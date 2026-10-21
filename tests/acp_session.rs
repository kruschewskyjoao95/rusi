//! Sessão ACP (via driver do Grok) contra um agente ACP falso (script Python).

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{GrokDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, SessionError, run_turn};

const FAKE_ACP: &str = r#"#!/usr/bin/env python3
import sys, json, os

def out(m):
    m["jsonrpc"] = "2.0"
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    return json.loads(line) if line else None

def update(u):
    out({"method": "session/update", "params": {"sessionId": "s1", "update": u}})

authed = "loggedout" not in sys.argv[0]
while True:
    m = read()
    if m is None:
        break
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        out({"id": mid, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True},
                                   "authMethods": [{"id": "grok.com", "name": "Grok"}]}})
    elif method == "authenticate":
        authed = True
        out({"id": mid, "result": {}})
    elif method in ("session/new", "session/load"):
        if not authed:
            out({"id": mid, "error": {"code": -32000, "message": "Authentication required"}})
        elif method == "session/new":
            out({"id": mid, "result": {"sessionId": "s1", "models": {"currentModelId": "grok-x"},
                                       "modes": {"availableModes": [{"id": "plan"}, {"id": "default"}]}}})
        else:
            out({"id": mid, "result": {}})
    elif method == "session/set_mode":
        sys.stderr.write("mode=%s\n" % m["params"]["modeId"]); sys.stderr.flush()
        out({"id": mid, "result": {}})
    elif method == "session/prompt":
        text = m["params"]["prompt"][0]["text"]
        if "slow" in text:
            update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "pensando"}})
            while True:
                n = read()
                if n is None: sys.exit(0)
                if n.get("method") == "session/cancel":
                    out({"id": mid, "result": {"stopReason": "cancelled"}})
                    break
            continue
        update({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run ls", "kind": "execute", "rawInput": {"command": "ls"}})
        out({"id": 500, "method": "session/request_permission", "params": {"sessionId": "s1",
             "toolCall": {"toolCallId": "t1", "title": "Run ls", "kind": "execute"},
             "options": [{"optionId": "yes", "name": "Yes", "kind": "allow_once"}, {"optionId": "no", "name": "No", "kind": "reject_once"}]}})
        reply = read()
        picked = reply["result"]["outcome"].get("optionId", "cancelled")
        update({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" if picked == "yes" else "failed",
                "content": [{"type": "content", "content": {"type": "text", "text": "arquivos"}}]})
        update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "feito: "}})
        update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": picked}})
        out({"id": mid, "result": {"stopReason": "end_turn", "usage": {"inputTokens": 4, "outputTokens": 1}}})
"#;

fn setup() -> (tempfile::TempDir, GrokDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grok");
    std::fs::write(&path, FAKE_ACP).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "grok".into(), ..Default::default() }).unwrap();
    (dir, GrokDriver::new().with_binary(path), db, thread.id)
}

fn config(mode: AgentMode) -> SessionConfig {
    SessionConfig { mode, ..Default::default() }
}

#[tokio::test]
async fn test_rejected_permission_in_accept_edits() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::AcceptEdits)).await.unwrap();

    let mut seen = Vec::new();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |e| seen.push(e.clone()))
        .await
        .unwrap();
    session.close().await;

    // Todo o texto chega antes do fim do turno, mesmo com a resposta do prompt logo em seguida.
    assert_eq!(summary.text, "feito: no");
    assert_eq!(summary.denied_tools, ["Run ls"]);
    assert_eq!(summary.usage.unwrap().input_tokens, 4);
    assert!(matches!(&seen[0], AgentEvent::SessionStarted { session_id, model }
        if session_id == "s1" && model.as_deref() == Some("grok-x")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. })));
    assert_eq!(db.get_thread(&thread_id).unwrap().provider_session_id.as_deref(), Some("s1"));
}

#[tokio::test]
async fn test_auto_mode_allows_the_tool() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto)).await.unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    session.close().await;
    assert_eq!(summary.text, "feito: yes");
    assert!(summary.denied_tools.is_empty());
}

#[tokio::test]
async fn test_cancel_ends_the_turn_and_session_continues() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::Auto)).await.unwrap();
    let cancel = tokio::time::sleep(Duration::from_millis(300));
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, rusi::session::deny_all, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);

    let next = run_turn(session.as_mut(), &db, &thread_id, "liste", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    assert_eq!(next.text, "feito: yes");
    session.close().await;
}

#[tokio::test]
async fn test_resume_uses_session_load() {
    let (_dir, driver, _db, _t) = setup();
    let mut session = driver
        .start_session(SessionConfig { resume: Some("s-antiga".into()), ..Default::default() })
        .await
        .unwrap();
    let first = session.next_event().await;
    session.close().await;
    assert!(matches!(first, Some(AgentEvent::SessionStarted { session_id, .. }) if session_id == "s-antiga"));
}

#[tokio::test]
async fn test_logged_out_agent_reports_login_hint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grok-loggedout");
    std::fs::write(&path, FAKE_ACP).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = GrokDriver::new().with_binary(path);

    let err = driver.start_session(config(AgentMode::Auto)).await.err().expect("deveria falhar sem login");
    assert!(matches!(&err, SessionError::NotReady(m) if m.contains("grok login")), "got: {err}");
}

#[tokio::test]
async fn test_user_can_approve_the_tool_in_accept_edits() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(config(AgentMode::AcceptEdits)).await.unwrap();
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
    session.close().await;
    assert_eq!(asked, ["Run ls"]);
    assert_eq!(summary.text, "feito: yes");
    assert!(summary.denied_tools.is_empty());
}
