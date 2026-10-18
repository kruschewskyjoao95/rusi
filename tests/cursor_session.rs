//! Sessão do Cursor contra um `cursor-agent` falso: um processo por turno.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{CursorDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, run_turn};

/// O prompt é o último argumento; `--resume <id>` aparece a partir do segundo turno.
const FAKE_CURSOR: &str = r#"#!/usr/bin/env python3
import sys, json, time

def out(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

args = sys.argv[1:]
prompt = args[-1]
resumed = args[args.index("--resume") + 1] if "--resume" in args else "nenhum"
out({"type": "system", "subtype": "init", "session_id": "chat-1", "model": "fake"})
if "slow" in prompt:
    out({"type": "assistant", "timestamp_ms": 1, "message": {"content": [{"type": "text", "text": "pensando"}]}})
    time.sleep(30)
if "die" in prompt:
    sys.stderr.write("falha simulada\n"); sys.exit(3)
out({"type": "tool_call", "subtype": "started", "call_id": "k1", "tool_call": {"readToolCall": {"args": {"path": "a"}}}})
out({"type": "tool_call", "subtype": "completed", "call_id": "k1", "tool_call": {"readToolCall": {"args": {}, "result": {"success": {"content": "x"}}}}})
out({"type": "assistant", "timestamp_ms": 2, "message": {"content": [{"type": "text", "text": "resumo=" + resumed}]}})
out({"type": "assistant", "message": {"content": [{"type": "text", "text": "resumo=" + resumed}]}})
out({"type": "result", "subtype": "success", "is_error": False, "duration_ms": 7})
"#;

fn setup() -> (tempfile::TempDir, CursorDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor-agent");
    std::fs::write(&path, FAKE_CURSOR).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "cursor".into(), ..Default::default() }).unwrap();
    (dir, CursorDriver::new().with_binary(path), db, thread.id)
}

#[tokio::test]
async fn test_two_turns_share_the_chat_through_resume() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig { mode: AgentMode::Auto, ..Default::default() }).await.unwrap();

    let mut seen = Vec::new();
    let first = run_turn(session.as_mut(), &db, &thread_id, "um", std::future::pending(), |e| seen.push(e.clone()))
        .await
        .unwrap();
    assert_eq!(first.text, "resumo=nenhum");
    assert!(matches!(&seen[0], AgentEvent::SessionStarted { session_id, .. } if session_id == "chat-1"));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "read")));
    // O texto parcial não é duplicado pela mensagem consolidada.
    let deltas: String = seen.iter().filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
    assert_eq!(deltas, "resumo=nenhum");

    let second = run_turn(session.as_mut(), &db, &thread_id, "dois", std::future::pending(), |_| {})
        .await
        .unwrap();
    assert_eq!(second.text, "resumo=chat-1", "o segundo turno usa --resume com o chat do primeiro");
    session.close().await;
    assert_eq!(db.get_thread(&thread_id).unwrap().provider_session_id.as_deref(), Some("chat-1"));
}

#[tokio::test]
async fn test_resume_from_previous_run() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver
        .start_session(SessionConfig { resume: Some("chat-antigo".into()), ..Default::default() })
        .await
        .unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), |_| {}).await.unwrap();
    session.close().await;
    assert_eq!(summary.text, "resumo=chat-antigo");
}

#[tokio::test]
async fn test_interrupt_kills_the_turn_and_next_turn_works() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    let cancel = tokio::time::sleep(Duration::from_millis(500));
    let started = std::time::Instant::now();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);
    assert!(started.elapsed() < Duration::from_secs(10), "o processo foi derrubado, não esperou o sleep");

    let next = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), |_| {}).await.unwrap();
    assert_eq!(next.text, "resumo=chat-1");
    session.close().await;
}

#[tokio::test]
async fn test_process_failure_surfaces_stderr() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "die", std::future::pending(), |_| {}).await.unwrap();
    session.close().await;
    assert!(summary.is_error);
    assert!(summary.text.contains("falha simulada"), "got: {}", summary.text);
}
