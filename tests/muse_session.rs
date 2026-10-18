//! Sessão do Muse contra um `muse serve` falso (MSP, JSON-RPC 2.0, script Python).

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{MuseDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, run_turn};

const FAKE_MUSE: &str = r#"#!/usr/bin/env python3
import sys, json

def out(m):
    m["jsonrpc"] = "2.0"; sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    return json.loads(line) if line else None

if sys.argv[1:2] != ["serve"]:
    sys.exit(64)

sid = None
while True:
    m = read()
    if m is None:
        break
    method, mid, p = m.get("method"), m.get("id"), m.get("params", {})
    if method == "initialize":
        out({"id": mid, "result": {"schema": {"version": 1}}})
    elif method in ("session/start", "session/resume"):
        sid = p["sessionId"]
        out({"id": mid, "result": {"session": {"sessionId": sid, "modelId": "muse-fake"}, "viewCursor": "v"}})
    elif method == "session/setApprovalMode":
        out({"id": mid, "result": {}})
    elif method == "turn/start":
        text = p["input"][0]["text"]
        turn = p["commandId"]
        out({"id": mid, "result": {"commandId": turn, "turnId": turn, "status": "accepted", "startedNewTurn": True, "disposition": "started"}})
        out({"method": "turn/started", "params": {"sessionId": sid, "turnId": turn}})
        out({"method": "item/started", "params": {"sessionId": sid, "item": {"itemId": "m1", "kind": "agentMessage", "status": "inProgress", "text": ""}}})
        if "slow" in text:
            out({"method": "item/delta", "params": {"sessionId": sid, "itemId": "m1", "field": "text", "delta": "pensando"}})
            continue
        out({"method": "item/started", "params": {"sessionId": sid, "item": {"itemId": "t1", "kind": "toolCall", "tool": "bash", "args": "{\"command\":\"ls\"}", "status": "inProgress"}}})
        out({"id": 700, "method": "approval/request", "params": {"approvalId": "ap1"}})
        ack = read()
        out({"method": "approval/requested", "params": {"sessionId": sid, "approvalId": "ap1", "toolName": "bash",
             "currentRequirementId": {"approvalId": "ap1", "sourceIndex": 0},
             "availableChoices": [
                 {"choiceId": "yes", "decision": "approved", "scope": "once", "label": "Yes"},
                 {"choiceId": "no", "decision": "abort", "scope": "once", "label": "Reject"}]}})
        decide = read()
        choice = decide["params"]["choiceId"]
        ok = choice == "yes"
        out({"id": decide["id"], "result": {}})
        out({"method": "item/completed", "params": {"sessionId": sid, "item": {"itemId": "t1", "kind": "toolCall", "tool": "bash",
             "status": "completed" if ok else "rejected", "exitCode": 0 if ok else None, "visibleOutput": "arquivos" if ok else "rejeitado"}}})
        out({"method": "item/delta", "params": {"sessionId": sid, "itemId": "m1", "field": "text", "delta": "feito: " + choice}})
        out({"method": "item/completed", "params": {"sessionId": sid, "item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": "feito: " + choice}}})
        out({"method": "session/tokenUsage", "params": {"sessionId": sid, "usage": {"inputTokens": 6, "outputTokens": 2, "cachedTokens": 1}}})
        out({"method": "turn/completed", "params": {"sessionId": sid, "turnId": turn, "terminal": "completed", "durationMs": 9}})
    elif method == "turn/interrupt":
        out({"id": mid, "result": {}})
        out({"method": "turn/completed", "params": {"sessionId": sid, "turnId": p["turnId"], "terminal": "cancelled"}})
"#;

fn setup() -> (tempfile::TempDir, MuseDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("muse");
    std::fs::write(&path, FAKE_MUSE).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "muse".into(), ..Default::default() }).unwrap();
    (dir, MuseDriver::new().with_binary(path), db, thread.id)
}

fn config(mode: AgentMode) -> SessionConfig {
    SessionConfig { mode, ..Default::default() }
}

#[tokio::test]
async fn test_approval_is_rejected_in_accept_edits() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::AcceptEdits)).await.unwrap();

    let mut seen = Vec::new();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), |e| seen.push(e.clone()))
        .await
        .unwrap();
    session.close().await;

    assert_eq!(summary.text, "feito: no");
    assert_eq!(summary.denied_tools, ["bash"]);
    assert_eq!(summary.usage.unwrap().cache_read_tokens, 1);
    assert!(matches!(&seen[0], AgentEvent::SessionStarted { model, .. } if model.as_deref() == Some("muse-fake")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "bash")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. })));
    assert!(db.get_thread(&thread_id).unwrap().provider_session_id.is_some());
}

#[tokio::test]
async fn test_auto_mode_approves() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto)).await.unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), |_| {})
        .await
        .unwrap();
    session.close().await;
    assert_eq!(summary.text, "feito: yes");
    assert!(summary.denied_tools.is_empty());
}

#[tokio::test]
async fn test_interrupt_then_continue_and_resume_keeps_session_id() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto)).await.unwrap();
    let cancel = tokio::time::sleep(Duration::from_millis(300));
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);
    session.close().await;

    let stored = db.get_thread(&thread_id).unwrap().provider_session_id.unwrap();
    let mut resumed = driver
        .start_session(SessionConfig { resume: Some(stored.clone()), mode: AgentMode::Auto, ..Default::default() })
        .await
        .unwrap();
    let first = resumed.next_event().await;
    resumed.close().await;
    assert!(matches!(first, Some(AgentEvent::SessionStarted { session_id, .. }) if session_id == stored));
}
