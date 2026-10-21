use std::future::Future;

use super::{AgentEvent, AgentSession, ApprovalRequest, SessionError, TokenUsage};
use crate::persistence::Database;

/// Resultado de um turno já gravado no banco.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnSummary {
    pub text: String,
    pub is_error: bool,
    pub interrupted: bool,
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    pub denied_tools: Vec<String>,
}

/// Aprovador padrão para quem não pode perguntar ao usuário: nega tudo.
pub async fn deny_all(_: &ApprovalRequest) -> bool {
    false
}

/// Executa um turno completo: grava o prompt, repassa cada evento a `on_event`,
/// persiste o histórico na thread e guarda o id de sessão do provedor para retomada.
///
/// Pedidos de permissão passam por `approve`. Se `cancel` completar durante o turno
/// (inclusive esperando `approve`), o pedido é negado, o agente é interrompido (uma vez)
/// e os eventos continuam sendo drenados até o fim do turno.
pub async fn run_turn<C>(
    session: &mut dyn AgentSession,
    db: &Database,
    thread_id: &str,
    prompt: &str,
    cancel: C,
    mut approve: impl AsyncFnMut(&ApprovalRequest) -> bool,
    mut on_event: impl FnMut(&AgentEvent),
) -> Result<TurnSummary, SessionError>
where
    C: Future<Output = ()>,
{
    use serde_json::json;
    tokio::pin!(cancel);

    db.append_event(thread_id, "user", &json!({ "text": prompt }))?;
    session.send(prompt).await?;

    let mut summary = TurnSummary::default();
    let mut cancel_pending = true;

    loop {
        let event = tokio::select! {
            ev = session.next_event() => ev,
            _ = &mut cancel, if cancel_pending => {
                cancel_pending = false;
                summary.interrupted = true;
                session.interrupt().await?;
                continue;
            }
        };
        let Some(event) = event else {
            db.append_event(thread_id, "session_closed", &json!({}))?;
            summary.is_error = true;
            return Ok(summary);
        };
        on_event(&event);

        match event {
            AgentEvent::SessionStarted { session_id, .. } => {
                db.set_thread_provider_session(thread_id, &session_id)?;
            }
            AgentEvent::TextDelta(t) => summary.text.push_str(&t),
            AgentEvent::ThinkingDelta(_) => {}
            AgentEvent::ToolUse { id, name, input } => {
                db.append_event(thread_id, "tool_use", &json!({ "id": id, "name": name, "input": input }))?;
            }
            AgentEvent::ToolResult { id, output, is_error } => {
                db.append_event(thread_id, "tool_result", &json!({ "id": id, "output": output, "is_error": is_error }))?;
            }
            AgentEvent::ApprovalRequested(request) => {
                let mut cancelled_now = false;
                let approved = tokio::select! {
                    approved = approve(&request) => approved,
                    _ = &mut cancel, if cancel_pending => {
                        cancel_pending = false;
                        cancelled_now = true;
                        summary.interrupted = true;
                        false
                    }
                };
                db.append_event(
                    thread_id,
                    "approval",
                    &json!({ "id": request.id, "tool": request.tool, "input": request.input, "approved": approved }),
                )?;
                if !approved && !summary.denied_tools.contains(&request.tool) {
                    summary.denied_tools.push(request.tool.clone());
                }
                session.respond_approval(&request.id, approved).await?;
                if cancelled_now {
                    session.interrupt().await?;
                }
            }
            AgentEvent::TurnComplete { text, is_error, usage, cost_usd, denied_tools, .. } => {
                if let Some(final_text) = text.filter(|t| !t.is_empty()) {
                    summary.text = final_text;
                }
                summary.is_error = is_error;
                summary.usage = usage;
                summary.cost_usd = cost_usd;
                for tool in denied_tools {
                    if !summary.denied_tools.contains(&tool) {
                        summary.denied_tools.push(tool);
                    }
                }
                db.append_event(
                    thread_id,
                    "assistant",
                    &json!({
                        "text": summary.text,
                        "is_error": is_error,
                        "interrupted": summary.interrupted,
                        "cost_usd": cost_usd,
                        "denied_tools": summary.denied_tools,
                    }),
                )?;
                return Ok(summary);
            }
            AgentEvent::Error(message) => {
                db.append_event(thread_id, "error", &json!({ "message": message }))?;
                summary.is_error = true;
                summary.text = message;
                return Ok(summary);
            }
        }
    }
}
