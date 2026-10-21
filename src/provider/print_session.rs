//! Sessão com um CLI sem modo servidor: cada turno é um processo `<cli> … --output-format stream-json`,
//! e a conversa continua pelo id que o próprio CLI informa (`--resume` no Cursor, `--conversation` no `agy`).
//! Só muda, por CLI, a montagem dos argumentos e a tradução das linhas para `AgentEvent`.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::session::{AgentEvent, AgentSession, SessionConfig, SessionError};
use crate::tools::process::kill_process_group;

/// Traduz uma linha de stdout do CLI em eventos.
pub(super) trait Translate: Send {
    fn handle(&mut self, line: &str) -> Vec<AgentEvent>;
}

/// Argumentos de um turno: configuração, id de retomada e prompt.
pub(super) type ArgsFn = fn(&SessionConfig, Option<&str>, &str) -> Vec<String>;

pub struct PrintSession {
    /// Nome do CLI, para a mensagem de erro quando ele morre sem resultado.
    name: &'static str,
    build_args: ArgsFn,
    translator: fn() -> Box<dyn Translate>,
    binary: PathBuf,
    config: SessionConfig,
    chat_id: Arc<Mutex<Option<String>>>,
    running_pid: Arc<AtomicU32>,
    interrupted: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
}

impl PrintSession {
    pub(super) fn new(name: &'static str, binary: &Path, config: &SessionConfig, build_args: ArgsFn, translator: fn() -> Box<dyn Translate>) -> Self {
        let (tx, events) = mpsc::unbounded_channel();
        Self {
            name,
            build_args,
            translator,
            binary: binary.to_path_buf(),
            config: config.clone(),
            chat_id: Arc::new(Mutex::new(config.resume.clone())),
            running_pid: Arc::new(AtomicU32::new(0)),
            interrupted: Arc::new(AtomicBool::new(false)),
            tx,
            events,
        }
    }
}

#[async_trait]
impl AgentSession for PrintSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        if self.running_pid.load(Ordering::SeqCst) != 0 {
            return Err(SessionError::NotReady("já existe um turno em andamento".into()));
        }
        let chat_id = self.chat_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let mut cmd = Command::new(&self.binary);
        cmd.args((self.build_args)(&self.config, chat_id.as_deref(), prompt))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(cwd) = &self.config.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(SessionError::Spawn)?;
        self.running_pid.store(child.id().unwrap_or(0), Ordering::SeqCst);
        self.interrupted.store(false, Ordering::SeqCst);

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let tx = self.tx.clone();
        let chat = self.chat_id.clone();
        let running = self.running_pid.clone();
        let interrupted = self.interrupted.clone();
        let name = self.name;
        let mut translator = (self.translator)();

        tokio::spawn(async move {
            let stderr_task = tokio::spawn(async move {
                let mut text = String::new();
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if text.len() < 2000 {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
                text
            });

            let mut completed = false;
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                for event in translator.handle(&line) {
                    if let AgentEvent::SessionStarted { session_id, .. } = &event {
                        *chat.lock().unwrap_or_else(|p| p.into_inner()) = Some(session_id.clone());
                    }
                    if matches!(event, AgentEvent::TurnComplete { .. }) {
                        completed = true;
                        // O turno acabou para quem consome: já pode começar o próximo, mesmo
                        // que o processo ainda esteja encerrando.
                        running.store(0, Ordering::SeqCst);
                    }
                    let _ = tx.send(event);
                }
            }
            let status = child.wait().await.ok();
            let stderr_text = stderr_task.await.unwrap_or_default();

            if completed {
                return;
            }
            running.store(0, Ordering::SeqCst);
            if interrupted.load(Ordering::SeqCst) {
                let _ = tx.send(AgentEvent::TurnComplete {
                    text: None,
                    is_error: true,
                    usage: None,
                    cost_usd: None,
                    duration_ms: None,
                    denied_tools: Vec::new(),
                });
            } else {
                let detail = stderr_text.trim();
                let message = if detail.is_empty() {
                    format!("o {name} terminou sem resultado (status {:?})", status.and_then(|s| s.code()))
                } else {
                    detail.to_string()
                };
                let _ = tx.send(AgentEvent::Error(message));
            }
        });
        Ok(())
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        let pid = self.running_pid.load(Ordering::SeqCst);
        if pid != 0 {
            self.interrupted.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        }
        Ok(())
    }

    async fn close(&mut self) {
        let pid = self.running_pid.swap(0, Ordering::SeqCst);
        if pid != 0 {
            self.interrupted.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        }
    }
}
