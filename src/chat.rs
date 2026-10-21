//! Fluxo de conversa do terminal: abre/retoma uma thread, inicia a sessão do provedor
//! e faz o streaming dos turnos para a tela.

use colored::Colorize;
use std::io::{self, IsTerminal, Write};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::cli::open_database;
use crate::config::Config;
use crate::persistence::{Database, NewThread, Thread};
use crate::provider::driver_by_id;
use crate::session::{AgentEvent, AgentSession, ApprovalRequest, SessionConfig, TurnSummary, run_turn};

pub struct ChatOptions {
    pub prompt: Option<String>,
    /// Imprime só a resposta final (modo não interativo).
    pub print_only: bool,
    pub resume_latest: bool,
}

type BoxError = Box<dyn std::error::Error>;

pub async fn run(config: &Config, options: ChatOptions) -> Result<(), BoxError> {
    let db = open_database()?;
    let thread = open_thread(&db, config, &options)?;
    let driver = driver_by_id(&thread.provider)
        .ok_or_else(|| format!("provedor desconhecido: '{}'. Veja `rusi providers`", thread.provider))?;

    let mut session = driver
        .start_session(SessionConfig {
            cwd: thread_cwd(&thread),
            model: config.default_model.clone().or(thread.model.clone()),
            effort: config.default_effort,
            mode: config.default_mode,
            dangerously_skip_permissions: config.dangerously_skip_permissions,
            resume: thread.provider_session_id.clone(),
        })
        .await?;

    let outcome = drive(session.as_mut(), &db, &thread, &options).await;
    session.close().await;
    outcome
}

/// Checkout da tarefa (worktree, se houver); senão a pasta atual.
fn thread_cwd(thread: &Thread) -> Option<std::path::PathBuf> {
    thread.cwd.as_deref().map(Into::into).or_else(|| std::env::current_dir().ok())
}

fn open_thread(db: &Database, config: &Config, options: &ChatOptions) -> Result<Thread, BoxError> {
    if options.resume_latest {
        if let Some(thread) = db.latest_thread()? {
            eprintln!(
                "{} {} ({})",
                "Retomando sessão:".bold(),
                thread.id.cyan(),
                thread.title.as_deref().unwrap_or("sem título")
            );
            return Ok(thread);
        }
        eprintln!("{}", "Nenhuma sessão anterior; iniciando uma nova.".yellow());
    }
    Ok(db.create_thread(NewThread {
        provider: config.default_provider.clone(),
        model: config.default_model.clone(),
        title: options.prompt.as_deref().map(title_from),
        cwd: std::env::current_dir().ok().map(|p| p.display().to_string()),
    })?)
}

fn title_from(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    first_line.chars().take(60).collect()
}

async fn drive(
    session: &mut dyn AgentSession,
    db: &Database,
    thread: &Thread,
    options: &ChatOptions,
) -> Result<(), BoxError> {
    if let Some(prompt) = &options.prompt {
        let summary = turn(session, db, thread, prompt, options.print_only).await?;
        return match failure_reason(&summary) {
            Some(reason) => Err(reason.into()),
            None => Ok(()),
        };
    }

    eprintln!("{}", "Sessão interativa. Linha vazia ignora; /exit ou Ctrl-D sai.".dimmed());
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        eprint!("{} ", "›".cyan().bold());
        let _ = io::stderr().flush();
        let line = tokio::select! {
            line = lines.next_line() => line?,
            _ = tokio::signal::ctrl_c() => None,
        };
        let Some(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit") {
            break;
        }
        let summary = turn(session, db, thread, line, false).await?;
        if let Some(reason) = failure_reason(&summary) {
            eprintln!("{} {reason}", "Erro:".red().bold());
        }
    }
    Ok(())
}

async fn turn(
    session: &mut dyn AgentSession,
    db: &Database,
    thread: &Thread,
    prompt: &str,
    print_only: bool,
) -> Result<TurnSummary, BoxError> {
    let cancel = async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\n{}", "Interrompendo…".yellow());
    };
    // Sem terminal (pipe, -p) não há quem responda: o pedido é negado.
    let interactive = !print_only && io::stdin().is_terminal();
    let approve = async |request: &ApprovalRequest| interactive && ask_user(request).await;
    let cwd = thread_cwd(thread);
    let summary = crate::git::track_changes(
        db,
        &thread.id,
        cwd.as_deref(),
        run_turn(session, db, &thread.id, prompt, cancel, approve, |event| {
            if print_only {
                return;
            }
            render(event);
        }),
    )
    .await?;

    let failed = summary.is_error && !summary.interrupted;
    if print_only {
        if !failed {
            println!("{}", summary.text);
        }
    } else {
        println!();
        report(&summary);
    }
    Ok(summary)
}

async fn ask_user(request: &ApprovalRequest) -> bool {
    eprintln!(
        "\n{} {} {}",
        "Permitir?".yellow().bold(),
        request.tool.bold(),
        summarize_input(&request.input).dimmed()
    );
    eprint!("[s]im / [N]ão › ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let read = BufReader::new(tokio::io::stdin()).read_line(&mut answer).await;
    read.is_ok() && is_yes(&answer)
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "s" | "sim" | "y" | "yes")
}

/// Motivo da falha de um turno (não conta interrupção pelo usuário).
fn failure_reason(summary: &TurnSummary) -> Option<String> {
    if !summary.is_error || summary.interrupted {
        return None;
    }
    let first_line = summary.text.lines().next().filter(|l| !l.is_empty());
    Some(first_line.unwrap_or("o turno terminou com erro").to_string())
}

fn render(event: &AgentEvent) {
    match event {
        AgentEvent::TextDelta(text) => {
            print!("{text}");
            let _ = io::stdout().flush();
        }
        AgentEvent::ToolUse { name, input, .. } => {
            eprintln!("\n{} {}", "🔧".yellow(), format!("{name} {}", summarize_input(input)).dimmed());
        }
        AgentEvent::ToolResult { is_error: true, output, .. } => {
            eprintln!("{} {}", "✖".red(), output.lines().next().unwrap_or("").dimmed());
        }
        AgentEvent::Error(message) => eprintln!("\n{} {message}", "Erro:".red().bold()),
        _ => {}
    }
}

fn summarize_input(input: &serde_json::Value) -> String {
    let text = input.to_string();
    if text.chars().count() > 100 {
        format!("{}…", text.chars().take(100).collect::<String>())
    } else {
        text
    }
}

fn report(summary: &TurnSummary) {
    if !summary.denied_tools.is_empty() {
        eprintln!(
            "{} {} (modo atual não permite; use --mode auto ou --dangerously-skip-permissions)",
            "Negado por permissão:".yellow().bold(),
            summary.denied_tools.join(", ")
        );
    }
    if summary.interrupted {
        eprintln!("{}", "Turno interrompido.".yellow());
    }
    if let Some(usage) = &summary.usage {
        let cost = summary.cost_usd.map(|c| format!(", ~US$ {c:.4}")).unwrap_or_default();
        eprintln!(
            "{}",
            format!("tokens: {} entrada, {} saída{cost}", usage.input_tokens, usage.output_tokens).dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_reason() {
        let ok = TurnSummary::default();
        assert_eq!(failure_reason(&ok), None);
        let failed = TurnSummary { is_error: true, text: "sem créditos\nmais".into(), ..Default::default() };
        assert_eq!(failure_reason(&failed).as_deref(), Some("sem créditos"));
        let interrupted = TurnSummary { is_error: true, interrupted: true, ..Default::default() };
        assert_eq!(failure_reason(&interrupted), None);
        let silent = TurnSummary { is_error: true, ..Default::default() };
        assert_eq!(failure_reason(&silent).as_deref(), Some("o turno terminou com erro"));
    }

    #[test]
    fn test_is_yes() {
        assert!(is_yes("s\n") && is_yes(" SIM ") && is_yes("y"));
        assert!(!is_yes("") && !is_yes("n") && !is_yes("talvez"));
    }

    #[test]
    fn test_title_uses_first_line_truncated() {
        assert_eq!(title_from("primeira\nsegunda"), "primeira");
        assert_eq!(title_from(&"a".repeat(100)).chars().count(), 60);
    }
}
