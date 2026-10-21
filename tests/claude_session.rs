//! Sessão do Claude contra um `claude` falso que fala o protocolo stream-json.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use rusi::persistence::{Database, NewThread};
use rusi::provider::{ClaudeDriver, CliProbeDriver, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, SessionError, run_turn};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *interrupt*)
      echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"interrompido"}' ;;
    *die*)
      echo "falha fatal simulada" >&2
      exit 3 ;;
    *slow*)
      echo '{"type":"system","subtype":"init","session_id":"sess-slow","model":"fake"}'
      echo '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"pensando..."}}}' ;;
    *)
      echo '{"type":"system","subtype":"init","session_id":"sess-1","model":"fake"}'
      echo '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"ola "}}}'
      echo '{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"mundo"}}}'
      echo '{"type":"assistant","message":{"content":[{"type":"text","text":"ola mundo"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}}]}}'
      echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"conteudo"}]}}'
      echo '{"type":"result","subtype":"success","is_error":false,"result":"ola mundo","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":2}}' ;;
  esac
done
"#;

fn fake_driver(dir: &tempfile::TempDir) -> ClaudeDriver {
    let path: PathBuf = dir.path().join("claude");
    std::fs::write(&path, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    ClaudeDriver::new().with_binary(path)
}

fn setup() -> (tempfile::TempDir, ClaudeDriver, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let driver = fake_driver(&dir);
    let db = Database::open_in_memory().unwrap();
    let thread = db
        .create_thread(NewThread { provider: "claude".into(), ..Default::default() })
        .unwrap();
    (dir, driver, db, thread.id)
}

#[tokio::test]
async fn test_full_turn_is_streamed_and_persisted() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();

    let mut seen = Vec::new();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), rusi::session::deny_all, |e| {
        seen.push(e.clone())
    })
    .await
    .unwrap();
    session.close().await;

    assert_eq!(summary.text, "ola mundo");
    assert!(!summary.is_error && !summary.interrupted);
    assert_eq!(summary.usage.unwrap().output_tokens, 2);
    assert!(matches!(seen[0], AgentEvent::SessionStarted { .. }));
    assert!(seen.iter().any(|e| matches!(e, AgentEvent::TextDelta(t) if t == "ola ")));

    let thread = db.get_thread(&thread_id).unwrap();
    assert_eq!(thread.provider_session_id.as_deref(), Some("sess-1"));
    let kinds: Vec<_> = db.thread_events(&thread_id, 0).unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, ["user", "tool_use", "tool_result", "assistant"]);
}

#[tokio::test]
async fn test_second_turn_reuses_the_same_process() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    for _ in 0..2 {
        let s = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), rusi::session::deny_all, |_| {})
            .await
            .unwrap();
        assert_eq!(s.text, "ola mundo");
    }
    session.close().await;
}

#[tokio::test]
async fn test_interrupt_ends_the_turn_but_keeps_session() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();

    let cancel = tokio::time::sleep(Duration::from_millis(200));
    let summary = run_turn(session.as_mut(), &db, &thread_id, "slow", cancel, rusi::session::deny_all, |_| {}).await.unwrap();
    assert!(summary.interrupted && summary.is_error);

    // A sessão continua utilizável depois do interrupt.
    let next = run_turn(session.as_mut(), &db, &thread_id, "oi", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    assert_eq!(next.text, "ola mundo");
    session.close().await;
}

#[tokio::test]
async fn test_process_death_surfaces_stderr() {
    let (_dir, driver, db, thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread_id, "die", std::future::pending(), rusi::session::deny_all, |_| {})
        .await
        .unwrap();
    assert!(summary.is_error);
    assert!(summary.text.contains("falha fatal simulada"), "got: {}", summary.text);

    let err = session.send("de novo").await.unwrap_err();
    assert!(matches!(err, SessionError::Closed(_)));
}

#[tokio::test]
async fn test_close_terminates_a_busy_process() {
    let (_dir, driver, _db, _thread_id) = setup();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();
    session.send("slow").await.unwrap();
    let started = std::time::Instant::now();
    session.close().await;
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn test_provider_without_session_support_is_unsupported() {
    let driver = CliProbeDriver::new("muse", "Muse", &["muse"]);
    let err = driver.start_session(SessionConfig::default()).await.err().unwrap();
    assert!(matches!(err, SessionError::Unsupported(id) if id == "muse"));
}

const FAKE_CLAUDE_ASKS: &str = r#"#!/bin/sh
while IFS= read -r line; do
  echo '{"type":"system","subtype":"init","session_id":"sess-ask","model":"fake"}'
  echo '{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"t1"}}'
  IFS= read -r answer
  case "$answer" in
    *'"request_id":"req-1"'*'"behavior":"allow"'*'"command":"ls"'*) verdict=permitido ;;
    *'"behavior":"deny"'*) verdict=negado ;;
    *) verdict="resposta inesperada: $answer" ;;
  esac
  echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"$verdict\"}"
done
"#;

async fn ask_turn(approve: bool) -> (rusi::session::TurnSummary, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude");
    std::fs::write(&path, FAKE_CLAUDE_ASKS).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = ClaudeDriver::new().with_binary(path);
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "claude".into(), ..Default::default() }).unwrap();
    let mut session = driver.start_session(SessionConfig::default()).await.unwrap();

    let mut asked = Vec::new();
    let summary = run_turn(
        session.as_mut(),
        &db,
        &thread.id,
        "rode ls",
        std::future::pending(),
        async |request: &rusi::session::ApprovalRequest| {
            asked.push((request.tool.clone(), request.input.clone()));
            approve
        },
        |_| {},
    )
    .await
    .unwrap();
    session.close().await;
    assert_eq!(asked, vec![("Bash".to_string(), serde_json::json!({"command": "ls"}))]);
    (summary, db, thread.id)
}

#[tokio::test]
async fn test_approval_is_forwarded_and_recorded() {
    let (summary, db, thread_id) = ask_turn(true).await;
    assert_eq!(summary.text, "permitido");
    let events = db.thread_events(&thread_id, 0).unwrap();
    assert!(events.iter().any(|e| e.kind == "approval" && e.payload["approved"] == true), "{events:?}");
}

#[tokio::test]
async fn test_denied_approval_reaches_the_agent() {
    let (summary, ..) = ask_turn(false).await;
    assert_eq!(summary.text, "negado");
}
