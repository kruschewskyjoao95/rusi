//! Sessão do Pi contra um `pi --mode rpc` falso (script Python).

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{PiDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, run_turn};

const FAKE_PI: &str = r#"#!/usr/bin/env python3
import sys, json

def out(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

sys.stderr.write("args=" + " ".join(sys.argv[1:]) + "\n"); sys.stderr.flush()

for line in sys.stdin:
    m = json.loads(line)
    t, mid = m.get("type"), m.get("id")
    if t == "get_state":
        out({"type": "response", "id": mid, "command": "get_state", "success": True,
             "data": {"sessionFile": "/s/fake.jsonl", "model": {"id": "m1", "provider": "p"}}})
    elif t == "prompt":
        text = m["message"]
        out({"type": "response", "id": mid, "command": "prompt", "success": True, "data": {"disposition": "started"}})
        out({"type": "agent_start"})
        if "slow" in text:
            out({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "pensando"}})
            continue
        out({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "read", "args": {"path": "a"}})
        out({"type": "tool_execution_end", "toolCallId": "c1", "isError": False, "result": {"content": [{"type": "text", "text": "conteudo"}]}})
        out({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "ola "}})
        out({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "mundo"}})
        out({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": "ola mundo"}], "stopReason": "stop",
             "usage": {"input": 5, "output": 2, "cacheRead": 0, "cacheWrite": 0, "cost": {"total": 0.5}}}})
        out({"type": "agent_end"})
        out({"type": "agent_settled", "aborted": False})
    elif t == "abort":
        out({"type": "response", "id": mid, "command": "abort", "success": True})
        out({"type": "agent_settled", "aborted": True})
"#;

fn setup() -> (tempfile::TempDir, PiDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pi");
    std::fs::write(&path, FAKE_PI).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "pi".into(), ..Default::default() }).unwrap();
    (dir, PiDriver::new().with_binary(path), db, thread.id)
}

#[tokio::test]
async fn test_turn_streams_tools_and_persists_session_file() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver
        .start_session(SessionConfig { mode: AgentMode::Auto, ..Default::default() })
        .await
        .unwrap();

    let mut seen = Vec::new();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), |e| seen.push(e.clone()))
        .await
        .unwrap();
    session.close().await;

    assert_eq!(summary.text, "ola mundo");
    assert_eq!(summary.cost_usd, Some(0.5));
    assert!(matches!(&seen[0], AgentEvent::SessionStarted { session_id, model }
        if session_id == "/s/fake.jsonl" && model.as_deref() == Some("p/m1")));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::ToolResult { output, .. } if output == "conteudo")));
    assert_eq!(db.get_thread(&thread_id).unwrap().provider_session_id.as_deref(), Some("/s/fake.jsonl"));
}

#[tokio::test]
async fn test_abort_settles_the_turn() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    let cancel = tokio::time::sleep(Duration::from_millis(300));
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);

    let next = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), |_| {})
        .await
        .unwrap();
    assert_eq!(next.text, "ola mundo");
    session.close().await;
}
