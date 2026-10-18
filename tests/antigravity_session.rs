//! Driver do Antigravity contra um `agy_acp_server` falso (ACP em Python).

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use rusi::config::AgentMode;
use rusi::persistence::{Database, NewThread};
use rusi::provider::{AntigravityDriver, AuthState, ProviderDriver};
use rusi::session::{AgentEvent, SessionConfig, SessionError, run_turn};

/// Registra em `$GEMINI_HOME` o que o Rusi configurou: argumentos, modo e método de login.
const FAKE_AGY: &str = r#"#!/usr/bin/env python3
import sys, json, os

home = os.environ["GEMINI_HOME"]
def note(name, value):
    open(os.path.join(home, name), "a").write(value + "\n")

note("argv.txt", " ".join(sys.argv[1:]))
note("env.txt", "browser=%s harness=%s" % (os.environ.get("BROWSER"), os.environ.get("ANTIGRAVITY_HARNESS_PATH")))

def out(m):
    m["jsonrpc"] = "2.0"; sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()

for line in sys.stdin:
    m = json.loads(line)
    method, mid = m.get("method"), m.get("id")
    if method == "initialize":
        out({"id": mid, "result": {"protocolVersion": 1, "agentInfo": {"name": "antigravity-acp", "version": "1.3.0"},
             "agentCapabilities": {"loadSession": True, "sessionCapabilities": {"resume": {}}}}})
    elif method == "authenticate":
        note("auth.txt", m["params"]["methodId"])
        if os.path.exists(os.path.join(home, "deny-login")):
            out({"id": mid, "error": {"code": -32000, "message": "sign-in required"}})
        else:
            out({"id": mid, "result": {}})
    elif method in ("session/new", "session/resume", "session/load"):
        note("open.txt", method)
        out({"id": mid, "result": {"sessionId": "agy-1", "modes": {"availableModes": [{"id": "default"}, {"id": "yolo"}]}}})
    elif method == "session/set_mode":
        note("mode.txt", m["params"]["modeId"])
        out({"id": mid, "result": {}})
    elif method == "session/prompt":
        out({"method": "session/update", "params": {"sessionId": "agy-1", "update": {
            "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "ola do agy"}}}})
        out({"id": mid, "result": {"stopReason": "end_turn"}})
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    profile: PathBuf,
    driver: AntigravityDriver,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("install");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("agy_acp_server");
    std::fs::write(&exe, FAKE_AGY).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(install.join("localharness_external"), "").unwrap();
    let profile = dir.path().join("profile");
    let driver = AntigravityDriver::new().with_binary(exe).with_profile_dir(profile.clone());
    Fixture { _dir: dir, profile, driver }
}

fn read(profile: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(profile.join(name)).unwrap_or_default()
}

#[tokio::test]
async fn test_session_uses_private_profile_and_default_mode() {
    let f = fixture();
    let db = Database::open_in_memory().unwrap();
    let thread = db.create_thread(NewThread { provider: "antigravity".into(), ..Default::default() }).unwrap();

    let mut session = f
        .driver
        .start_session(SessionConfig { mode: AgentMode::Auto, ..Default::default() })
        .await
        .unwrap();
    let summary = run_turn(session.as_mut(), &db, &thread.id, "oi", std::future::pending(), |_| {})
        .await
        .unwrap();
    session.close().await;

    assert_eq!(summary.text, "ola do agy");
    assert_eq!(read(&f.profile, "auth.txt").trim(), "oauth-personal");
    assert_eq!(read(&f.profile, "open.txt").trim(), "session/new");
    assert_eq!(read(&f.profile, "mode.txt").trim(), "default");
    let env = read(&f.profile, "env.txt");
    assert!(env.contains("browser=/bin/false") && env.contains("localharness_external"), "got {env}");
    #[cfg(target_os = "linux")]
    assert_eq!(read(&f.profile, "argv.txt").trim(), "--uid=");

    let mode = f.profile.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "o perfil é privado");
    assert_eq!(db.get_thread(&thread.id).unwrap().provider_session_id.as_deref(), Some("agy-1"));
}

#[tokio::test]
async fn test_skip_permissions_selects_yolo_and_resume_prefers_resume() {
    let f = fixture();
    let mut session = f
        .driver
        .start_session(SessionConfig {
            dangerously_skip_permissions: true,
            resume: Some("agy-antiga".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = session.next_event().await;
    session.close().await;

    assert!(matches!(first, Some(AgentEvent::SessionStarted { session_id, .. }) if session_id == "agy-antiga"));
    assert_eq!(read(&f.profile, "mode.txt").trim(), "yolo");
    assert_eq!(read(&f.profile, "open.txt").trim(), "session/resume");
}

#[tokio::test]
async fn test_rejected_login_reports_sign_in_hint() {
    let f = fixture();
    std::fs::create_dir_all(&f.profile).unwrap();
    std::fs::write(f.profile.join("deny-login"), "").unwrap();
    let err = f.driver.start_session(SessionConfig::default()).await.err().expect("login recusado");
    assert!(matches!(&err, SessionError::NotReady(m) if m.contains("sem login")), "got: {err}");
}

#[tokio::test]
async fn test_missing_harness_is_reported() {
    let f = fixture();
    let harness = f._dir.path().join("install").join("localharness_external");
    std::fs::remove_file(harness).unwrap();
    let err = f.driver.start_session(SessionConfig::default()).await.err().unwrap();
    assert!(matches!(&err, SessionError::NotReady(m) if m.contains("localharness_external")), "got: {err}");
}

#[tokio::test]
async fn test_snapshot_reads_login_from_token_file() {
    let f = fixture();
    assert_eq!(f.driver.snapshot().await.auth, AuthState::Unauthenticated);

    let acp = f.profile.join("antigravity-acp");
    std::fs::create_dir_all(&acp).unwrap();
    std::fs::write(acp.join("acp_token.json"), r#"{"refresh_token":"r"}"#).unwrap();
    assert!(matches!(f.driver.snapshot().await.auth, AuthState::Authenticated { .. }));
}
