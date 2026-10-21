//! Sessão do Codex contra um `codex app-server` falso (script Python) que fala o protocolo real.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{CodexDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, run_turn};

const FAKE_CODEX: &str = r#"#!/usr/bin/env python3
import sys, json

def out(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    return json.loads(line) if line else None

if len(sys.argv) < 2 or sys.argv[1] != "app-server":
    sys.exit(64)

while True:
    m = read()
    if m is None:
        break
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        out({"id": mid, "result": {"userAgent": "fake", "codexHome": "/tmp"}})
    elif method == "thread/start" or method == "thread/resume":
        sys.stderr.write("policy=%s/%s\n" % (m["params"].get("approvalPolicy"), m["params"].get("sandbox")))
        sys.stderr.flush()
        tid = m["params"].get("threadId", "thr-1")
        out({"id": mid, "result": {"thread": {"id": tid}, "model": "fake-model"}})
    elif method == "account/read":
        out({"id": mid, "result": {"account": None, "requiresOpenaiAuth": True}})
    elif method == "turn/start":
        text = m["params"]["input"][0]["text"]
        out({"id": mid, "result": {"turn": {"id": "turn-1", "status": "inProgress", "items": []}}})
        out({"method": "turn/started", "params": {"threadId": "thr-1", "turn": {"id": "turn-1"}}})
        if "slow" in text:
            out({"method": "item/agentMessage/delta", "params": {"delta": "pensando"}})
            continue
        out({"method": "item/started", "params": {"item": {"type": "commandExecution", "id": "c1", "command": "ls", "cwd": "/w", "status": "inProgress"}}})
        out({"id": 100, "method": "item/commandExecution/requestApproval", "params": {"itemId": "c1"}})
        reply = read()
        decision = reply["result"]["decision"]
        out({"method": "item/completed", "params": {"item": {"type": "commandExecution", "id": "c1", "aggregatedOutput": "arquivos", "exitCode": 0 if decision == "accept" else 1, "status": "completed" if decision == "accept" else "declined"}}})
        out({"method": "item/agentMessage/delta", "params": {"delta": "feito: "}})
        out({"method": "item/agentMessage/delta", "params": {"delta": decision}})
        out({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "a1", "text": "feito: " + decision}}})
        out({"method": "thread/tokenUsage/updated", "params": {"tokenUsage": {"last": {"inputTokens": 7, "outputTokens": 3, "cachedInputTokens": 1}}}})
        out({"method": "turn/completed", "params": {"threadId": "thr-1", "turn": {"id": "turn-1", "status": "completed", "durationMs": 12}}})
    elif method == "turn/interrupt":
        out({"id": mid, "result": {}})
        out({"method": "turn/completed", "params": {"threadId": "thr-1", "turn": {"id": "turn-1", "status": "interrupted"}}})
"#;

fn setup() -> (tempfile::TempDir, CodexDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex");
    std::fs::write(&path, FAKE_CODEX).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "codex".into(), ..Default::default() }).unwrap();
    (dir, CodexDriver::new().with_binary(path), db, thread.id)
}

fn config(mode: AgentMode, skip: bool) -> SessionConfig {
    SessionConfig { mode, dangerously_skip_permissions: skip, ..Default::default() }
}

#[tokio::test]
async fn test_turn_with_denied_command_in_accept_edits() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::AcceptEdits, false)).await.unwrap();

    let mut seen = Vec::new();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |e| seen.push(e.clone()))
        .await
        .unwrap();
    session.close().await;

    assert_eq!(summary.text, "feito: decline");
    assert_eq!(summary.denied_tools, ["command"]);
    assert_eq!(summary.usage.unwrap().input_tokens, 7);
    assert!(matches!(&seen[0], AgentEvent::SessionStarted { session_id, model }
        if session_id == "thr-1" && model.as_deref() == Some("fake-model")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "command")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. })));

    let thread = db.get_thread(&thread_id).unwrap();
    assert_eq!(thread.provider_session_id.as_deref(), Some("thr-1"));
}

#[tokio::test]
async fn test_auto_mode_accepts_the_command() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto, false)).await.unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    session.close().await;
    assert_eq!(summary.text, "feito: accept");
    assert!(summary.denied_tools.is_empty());
}

#[tokio::test]
async fn test_interrupt_ends_turn_and_session_stays_usable() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto, false)).await.unwrap();

    let cancel = tokio::time::sleep(Duration::from_millis(300));
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, rusi::session::deny_all, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);

    let next = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    assert_eq!(next.text, "feito: accept");
    session.close().await;
}

#[tokio::test]
async fn test_resume_uses_thread_resume_with_given_id() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver
        .start_session(SessionConfig { resume: Some("thr-antiga".into()), ..Default::default() })
        .await
        .unwrap();
    let mut started = None;
    // Basta o primeiro evento: SessionStarted com o id retomado.
    if let Some(AgentEvent::SessionStarted { session_id, .. }) = session.next_event().await {
        started = Some(session_id);
    }
    session.close().await;
    assert_eq!(started.as_deref(), Some("thr-antiga"));
    let _ = (db, thread_id);
}

#[tokio::test]
async fn test_snapshot_reports_unauthenticated() {
    let (_dir, driver, _db, _t) = setup();
    let snap = driver.snapshot().await;
    assert!(snap.installed);
    assert_eq!(snap.auth, rusi::provider::AuthState::Unauthenticated);
}

#[tokio::test]
async fn test_user_can_approve_a_command_in_accept_edits() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::AcceptEdits, false)).await.unwrap();

    let mut asked = Vec::new();
    let summary = run_turn(
        session.as_mut(),
        &db,
        &thread_id,
        "liste",
        std::future::pending(),
        async |request: &rusi::session::ApprovalRequest| {
            asked.push(request.tool.clone());
            true
        },
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(asked, ["command"]);
    assert_eq!(summary.text, "feito: accept");
    assert!(summary.denied_tools.is_empty());
    session.close().await;
}
