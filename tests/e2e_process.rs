#![allow(unexpected_cfgs)]
#![cfg(milestone_2)]

mod common;

use common::TestEnv;
use rusi::tools::process::{bash_run, ProcessError};
use std::time::Instant;

// ============================================================================
// Tier 1: Feature Coverage (Representative Happy-Path Inputs)
// ============================================================================

#[tokio::test]
async fn test_tier1_bash_run_stdout_capture() {
    let output = bash_run("echo 'rusi_test_stdout'", None, 5000)
        .await
        .expect("bash_run should succeed");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.contains("rusi_test_stdout"));
    assert!(output.stderr.is_empty());
    assert!(!output.timed_out);
}

#[tokio::test]
async fn test_tier1_bash_run_stderr_capture() {
    let output = bash_run("echo 'rusi_test_stderr' >&2", None, 5000)
        .await
        .expect("bash_run should succeed");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("rusi_test_stderr"));
    assert!(output.combined.contains("rusi_test_stderr"));
    assert!(!output.timed_out);
}

#[tokio::test]
async fn test_tier1_bash_run_combined_streams() {
    let output = bash_run("echo 'stdout line' && echo 'stderr line' >&2", None, 5000)
        .await
        .expect("bash_run should succeed");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.contains("stdout line"));
    assert!(output.stderr.contains("stderr line"));
    assert!(output.combined.contains("stdout line"));
    assert!(output.combined.contains("stderr line"));
    assert!(!output.timed_out);
}

#[tokio::test]
async fn test_tier1_bash_run_custom_working_directory() {
    let env = TestEnv::new();
    let sub = env.file_path("nested_dir");
    std::fs::create_dir_all(&sub).expect("Failed to create sub dir");

    let output = bash_run("pwd -P", Some(&sub), 5000)
        .await
        .expect("bash_run in custom cwd should succeed");

    assert_eq!(output.exit_code, Some(0));
    let reported = output.stdout.trim();
    let canonical = std::fs::canonicalize(&sub).expect("canonicalize failed");
    assert_eq!(reported, canonical.to_string_lossy());
}

#[tokio::test]
async fn test_tier1_bash_run_exit_status_codes() {
    let output = bash_run("exit 7", None, 5000)
        .await
        .expect("bash_run should return ProcessOutput even on non-zero exit");

    assert_eq!(output.exit_code, Some(7));
    assert!(!output.timed_out);
}

#[tokio::test]
async fn test_tier1_bash_run_multistep_piped_command() {
    let script = "printf 'cherry\\napple\\nbanana\\n' | sort";
    let output = bash_run(script, None, 5000)
        .await
        .expect("piped command execution should succeed");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.stdout, "apple\nbanana\ncherry\n");
}

// ============================================================================
// Tier 2: Boundary & Corner Cases (Edge Conditions & Failure Modes)
// ============================================================================

#[tokio::test]
async fn test_tier2_bash_run_timeout_kills_long_running_process() {
    let start = Instant::now();
    let timeout_ms = 300;

    let res = bash_run("sleep 10", None, timeout_ms).await;
    let elapsed = start.elapsed();

    // Must enforce sub-second termination (strictly under 1000ms)
    assert!(
        elapsed.as_millis() < 1000,
        "Subprocess execution timeout must terminate in under 1s, took: {}ms",
        elapsed.as_millis()
    );

    match res {
        Ok(out) => {
            assert!(out.timed_out, "timed_out must be true on timeout");
            assert_eq!(out.exit_code, None);
        }
        Err(ProcessError::Timeout { .. }) => {
            // Also acceptable if modeled as Err(ProcessError::Timeout)
        }
        Err(other) => panic!("Expected timeout output or error, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_tier2_bash_run_timeout_kills_grandchild_process_group() {
    let start = Instant::now();
    let timeout_ms = 300;

    // bash -c spawns a child process which spawns grandchild sleep 10
    let res = bash_run("bash -c 'sleep 10'", None, timeout_ms).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 1000,
        "Process group kill must terminate child and grandchildren in under 1s, took: {}ms",
        elapsed.as_millis()
    );

    match res {
        Ok(out) => {
            assert!(out.timed_out);
        }
        Err(ProcessError::Timeout { .. }) => {}
        Err(other) => panic!("Unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn test_tier2_bash_run_invalid_cwd_returns_error() {
    let non_existent = std::path::Path::new("/non/existent/path/for/rusi/test/execution");

    let res = bash_run("echo hi", Some(non_existent), 5000).await;
    assert!(res.is_err(), "bash_run with invalid cwd must return an error");

    match res.unwrap_err() {
        ProcessError::InvalidCwd(p) => {
            assert_eq!(p, non_existent);
        }
        ProcessError::Spawn(_) | ProcessError::Io(_) => {
            // Also acceptable if IO error is returned
        }
        other => panic!("Expected InvalidCwd or Spawn error, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_tier2_bash_run_empty_command() {
    let output = bash_run("", None, 5000)
        .await
        .expect("Empty command should execute without error");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn test_tier2_bash_run_syntax_error_returns_nonzero_exit() {
    let output = bash_run("if then fi", None, 5000)
        .await
        .expect("Command with syntax error should return ProcessOutput");

    assert_ne!(output.exit_code, Some(0));
    assert!(!output.stderr.is_empty(), "Syntax error must produce stderr output");
}

#[tokio::test]
async fn test_tier2_bash_run_large_output_stream_no_deadlock() {
    let script = "for i in $(seq 1 5000); do echo \"line $i\"; done";
    let output = bash_run(script, None, 10000)
        .await
        .expect("High volume stdout streaming must not deadlock pipe buffers");

    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.contains("line 1\n"));
    assert!(output.stdout.contains("line 5000\n"));
}
