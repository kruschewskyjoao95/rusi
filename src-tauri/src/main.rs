//! Camada fina entre a UI e o núcleo `rusi`: comandos IPC e eventos. Nenhuma regra de negócio aqui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusi::config::AgentMode;
use rusi::git::{self, FileStatus};
use rusi::persistence::{Database, NewThread, Thread, ThreadEvent};
use rusi::provider::{ProviderSnapshot, built_in_drivers, driver_by_id, snapshot_all};
use rusi::session::{ApprovalRequest, SessionConfig, run_turn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{Notify, oneshot};

/// Eventos devolvidos ao abrir uma sessão; o resto fica no banco.
const MESSAGE_LIMIT: u32 = 300;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Default)]
struct AppState {
    db: Option<Database>,
    /// Turnos em andamento: o `Notify` interrompe o turno.
    running: Mutex<HashMap<String, Arc<Notify>>>,
    /// Aprovações aguardando o usuário, por `<thread>:<id do pedido>`.
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl AppState {
    fn db(&self) -> &Database {
        self.db.as_ref().expect("banco aberto na inicialização")
    }
}

type Shared<'a> = State<'a, Arc<AppState>>;

/// `lock()` que ignora envenenamento: um turno que deu panic não pode travar o app inteiro.
trait LockExt<T> {
    fn guard(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn guard(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Vive durante o turno: ao sair (inclusive por panic) libera a sessão e avisa a UI se ninguém avisou.
struct TurnGuard {
    app: AppHandle,
    state: Arc<AppState>,
    thread_id: String,
    finished: bool,
}

impl TurnGuard {
    /// Método (e não campo) para a closure capturar o guard inteiro e ele só cair no fim do turno.
    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.state.running.guard().remove(&self.thread_id);
        // Pedidos que ficaram sem resposta (turno interrompido) não vazam.
        let prefix = format!("{}:", self.thread_id);
        self.state.pending.guard().retain(|k, _| !k.starts_with(&prefix));
        if !self.finished {
            let _ = self.app.emit("turn-finished", serde_json::json!({ "threadId": self.thread_id, "isError": true, "text": "o turno falhou inesperadamente" }));
        }
        let _ = self.app.emit("sessions-changed", ());
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadEventPayload<'a, T: Serialize> {
    thread_id: &'a str,
    event: T,
}

#[tauri::command]
async fn list_providers() -> Vec<ProviderSnapshot> {
    snapshot_all(&built_in_drivers()).await
}

#[tauri::command(async)]
fn list_sessions(state: Shared<'_>) -> CmdResult<Vec<Thread>> {
    state.db().list_threads(200).map_err(err)
}

#[tauri::command(async)]
fn create_session(app: AppHandle, state: Shared<'_>, provider: String, cwd: Option<String>, title: Option<String>) -> CmdResult<Thread> {
    if driver_by_id(&provider).is_none() {
        return Err(format!("provedor desconhecido: {provider}"));
    }
    let thread = state.db().create_thread(NewThread { provider, model: None, title, cwd }).map_err(err)?;
    let _ = app.emit("sessions-changed", ());
    Ok(thread)
}

#[tauri::command(async)]
fn delete_session(app: AppHandle, state: Shared<'_>, thread_id: String) -> CmdResult<()> {
    if state.running.guard().contains_key(&thread_id) {
        return Err("a sessão está em andamento; pare-a antes".into());
    }
    state.db().delete_thread(&thread_id).map_err(err)?;
    let _ = app.emit("sessions-changed", ());
    Ok(())
}

#[tauri::command(async)]
fn get_messages(state: Shared<'_>, thread_id: String) -> CmdResult<Vec<ThreadEvent>> {
    state.db().recent_thread_events(&thread_id, MESSAGE_LIMIT).map_err(err)
}

/// Inicia um turno em segundo plano; o progresso chega pelos eventos `agent-event` e `turn-finished`.
/// `mode`: `accept-edits` (padrão), `auto`, `plan` ou `full` (sem permissões).
#[tauri::command(async)]
fn send_message(app: AppHandle, state: Shared<'_>, thread_id: String, prompt: String, mode: Option<String>) -> CmdResult<()> {
    let state = Arc::clone(&state);
    let thread = state.db().get_thread(&thread_id).map_err(err)?;
    let driver = driver_by_id(&thread.provider).ok_or_else(|| format!("provedor desconhecido: {}", thread.provider))?;
    let (agent_mode, skip) = match mode.as_deref() {
        None | Some("") => (AgentMode::default(), false),
        Some("full") => (AgentMode::Auto, true),
        Some(m) => (m.parse::<AgentMode>().map_err(err)?, false),
    };

    if thread.title.is_none() {
        let title = rusi::chat::title_from(&prompt);
        if !title.is_empty() {
            let _ = state.db().rename_thread(&thread_id, &title);
        }
    }

    let cancel = Arc::new(Notify::new());
    {
        let mut running = state.running.guard();
        if running.contains_key(&thread_id) {
            return Err("já existe um turno em andamento nesta sessão".into());
        }
        running.insert(thread_id.clone(), Arc::clone(&cancel));
    }

    // Valores próprios antes do spawn: referências a `thread` presas através de um await quebram o `Send`.
    let cwd = thread.cwd.map(PathBuf::from);
    let config = SessionConfig {
        cwd: cwd.clone(),
        model: thread.model,
        mode: agent_mode,
        dangerously_skip_permissions: skip,
        resume: thread.provider_session_id,
        ..Default::default()
    };
    // Uma thread própria por turno com runtime local: `run_turn` recebe um `AsyncFnMut` e o rustc
    // ainda não consegue provar `Send` para esse futuro dentro de um spawn multi-thread.
    let mut guard = TurnGuard { app: app.clone(), state: Arc::clone(&state), thread_id: thread_id.clone(), finished: false };
    std::thread::spawn(move || tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(async move {
        let outcome: Result<_, String> = async {
            let mut session = driver.start_session(config).await.map_err(err)?;
            let approve = async |request: &ApprovalRequest| {
                let (tx, rx) = oneshot::channel();
                state.pending.guard().insert(format!("{thread_id}:{}", request.id), tx);
                rx.await.unwrap_or(false)
            };
            let stopped = Arc::clone(&cancel);
            let summary = git::track_changes(
                state.db(),
                &thread_id,
                cwd.as_deref(),
                run_turn(
                    session.as_mut(),
                    state.db(),
                    &thread_id,
                    &prompt,
                    async move { stopped.notified().await },
                    approve,
                    |event| {
                        let _ = app.emit("agent-event", ThreadEventPayload { thread_id: &thread_id, event });
                    },
                ),
            )
            .await;
            session.close().await;
            summary.map_err(err)
        }
        .await;

        let (is_error, text) = match &outcome {
            Ok(s) => (s.is_error, s.text.clone()),
            Err(e) => (true, e.clone()),
        };
        guard.finish();
        let _ = app.emit("turn-finished", serde_json::json!({ "threadId": thread_id, "isError": is_error, "text": text }));
    }));
    Ok(())
}

#[tauri::command]
fn stop_session(state: Shared<'_>, thread_id: String) {
    if let Some(cancel) = state.running.guard().get(&thread_id) {
        cancel.notify_one();
    }
}

#[tauri::command]
fn respond_approval(state: Shared<'_>, thread_id: String, id: String, approved: bool) -> CmdResult<()> {
    let tx = state.pending.guard().remove(&format!("{thread_id}:{id}"));
    tx.ok_or("pedido de aprovação não encontrado (já respondido ou turno encerrado)")?
        .send(approved)
        .map_err(|_| "o turno já terminou".into())
}

#[tauri::command]
async fn git_status(cwd: String) -> CmdResult<Vec<FileStatus>> {
    git::status(&PathBuf::from(cwd)).await.map_err(err)
}

#[tauri::command]
async fn git_diff(cwd: String, staged: bool) -> CmdResult<String> {
    git::diff(&PathBuf::from(cwd), staged).await.map_err(err)
}

#[tauri::command]
async fn create_worktree(app: AppHandle, state: Shared<'_>, thread_id: String, branch: String) -> CmdResult<String> {
    if state.running.guard().contains_key(&thread_id) {
        return Err("a sessão está em andamento; pare-a antes de criar o worktree".into());
    }
    let thread = state.db().get_thread(&thread_id).map_err(err)?;
    let repo = thread.cwd.ok_or("a sessão não tem pasta de projeto")?;
    let path = state.db().attach_worktree(&thread_id, &PathBuf::from(repo), &branch).await.map_err(err)?;
    let _ = app.emit("sessions-changed", ());
    Ok(path.display().to_string())
}

fn main() {
    let db = rusi::cli::open_database().expect("não foi possível abrir o banco do Rusi");
    let state = Arc::new(AppState { db: Some(db), ..Default::default() });
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_providers,
            list_sessions,
            create_session,
            delete_session,
            get_messages,
            send_message,
            stop_session,
            respond_approval,
            git_status,
            git_diff,
            create_worktree,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao executar o Rusi");
}
