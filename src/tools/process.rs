use std::path::{Path, PathBuf};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[cfg(unix)]
mod sys {
    unsafe extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

/// Output of a completed or timed-out process execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutput {
    /// Exit status code if the process exited normally, or `None` if terminated by a signal or timeout.
    pub exit_code: Option<i32>,
    /// Standard output captured from the process.
    pub stdout: String,
    /// Standard error captured from the process.
    pub stderr: String,
    /// Formatted combination of standard output and standard error.
    pub combined: String,
    /// Whether the execution timed out before normal completion.
    pub timed_out: bool,
}

/// Errors that can occur during process execution.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// Process execution exceeded the specified timeout.
    #[error("Process timed out after {timeout_ms}ms")]
    Timeout {
        timeout_ms: u64,
        stdout: String,
        stderr: String,
    },

    /// Failed to spawn the process.
    #[error("Failed to spawn process: {0}")]
    Spawn(#[source] std::io::Error),

    /// Standard I/O error during process operations.
    #[error("Process I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Working directory is invalid or does not exist.
    #[error("Working directory does not exist or is not a directory: '{0}'")]
    InvalidCwd(PathBuf),
}

/// Sends `SIGKILL` to the process group associated with `pid` on Unix systems.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    if pid > 1 {
        // Send SIGKILL (signal 9) to the process group (-pid)
        unsafe {
            let _ = sys::kill(-(pid as i32), 9);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_process_group(_pid: u32) {}

/// Merges stdout and stderr into a combined string.
fn combine_output(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => {
            if stdout.ends_with('\n') {
                format!("{stdout}{stderr}")
            } else {
                format!("{stdout}\n{stderr}")
            }
        }
    }
}

/// Executes a shell command asynchronously using `bash -c`.
///
/// Features:
/// - Executes the command in a dedicated Unix process group (`process_group(0)`) so that
///   all child and grandchild processes are tracked together.
/// - Reads stdout and stderr streams concurrently using `AsyncReadExt::read_to_end` to prevent
///   pipe buffer deadlocks.
/// - Enforces a strict execution timeout. On timeout, sends `SIGKILL` to the entire process group
///   (`-pgid`) ensuring sub-second termination of long-running processes (e.g. `sleep 10`)
///   without leaving orphaned background processes.
/// - Returns `ProcessOutput` with `timed_out: true` and `exit_code: None` upon timeout,
///   preserving any partial stdout/stderr produced prior to termination.
///
/// # Arguments
/// * `command` - The shell command string to execute.
/// * `cwd` - Optional working directory. If provided and invalid/non-existent, returns `ProcessError::InvalidCwd`.
/// * `timeout_ms` - Maximum execution duration in milliseconds.
pub async fn bash_run(
    command: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> Result<ProcessOutput, ProcessError> {
    if let Some(dir) = cwd
        && !dir.is_dir()
    {
        return Err(ProcessError::InvalidCwd(dir.to_path_buf()));
    }

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(command);

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;

    let child_id = child.id();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    // Stream stdout concurrently to avoid pipe buffer deadlocks
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stream) = stdout_pipe {
            let _ = stream.read_to_end(&mut buf).await;
        }
        buf
    });

    // Stream stderr concurrently to avoid pipe buffer deadlocks
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut stream) = stderr_pipe {
            let _ = stream.read_to_end(&mut buf).await;
        }
        buf
    });

    let timeout_duration = Duration::from_millis(timeout_ms);
    let wait_res = tokio::time::timeout(timeout_duration, child.wait()).await;

    match wait_res {
        Ok(status_res) => {
            let status = status_res.map_err(ProcessError::Io)?;

            let stdout_bytes = tokio::time::timeout(Duration::from_millis(2000), stdout_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            let stderr_bytes = tokio::time::timeout(Duration::from_millis(2000), stderr_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
            let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
            let combined = combine_output(&stdout, &stderr);

            Ok(ProcessOutput {
                exit_code: status.code(),
                stdout,
                stderr,
                combined,
                timed_out: false,
            })
        }
        Err(_) => {
            // Process timed out: kill the entire process group
            #[cfg(unix)]
            if let Some(pid) = child_id {
                kill_process_group(pid);
            }

            // Ensure child is reaped to avoid zombies
            let _ = child.kill().await;
            let _ = child.wait().await;

            let stdout_bytes = tokio::time::timeout(Duration::from_millis(500), stdout_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            let stderr_bytes = tokio::time::timeout(Duration::from_millis(500), stderr_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
            let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
            let combined = if stdout.is_empty() && stderr.is_empty() {
                format!("Process timed out after {timeout_ms}ms")
            } else {
                combine_output(&stdout, &stderr)
            };

            Ok(ProcessOutput {
                exit_code: None,
                stdout,
                stderr,
                combined,
                timed_out: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_bash_run_stdout() {
        let res = bash_run("echo 'hello rusi'", None, 5000)
            .await
            .expect("bash_run should succeed");

        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.contains("hello rusi"));
        assert!(res.stderr.is_empty());
        assert!(!res.timed_out);
        assert!(res.combined.contains("hello rusi"));
    }

    #[tokio::test]
    async fn test_bash_run_stderr() {
        let res = bash_run("echo 'an error message' >&2", None, 5000)
            .await
            .expect("bash_run should succeed");

        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.is_empty());
        assert!(res.stderr.contains("an error message"));
        assert!(res.combined.contains("an error message"));
        assert!(!res.timed_out);
    }

    #[tokio::test]
    async fn test_bash_run_combined_streams() {
        let res = bash_run("echo 'line 1' && echo 'err 2' >&2", None, 5000)
            .await
            .expect("bash_run should succeed");

        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.contains("line 1"));
        assert!(res.stderr.contains("err 2"));
        assert!(res.combined.contains("line 1"));
        assert!(res.combined.contains("err 2"));
        assert!(!res.timed_out);
    }

    #[tokio::test]
    async fn test_bash_run_exit_code_nonzero() {
        let res = bash_run("exit 42", None, 5000)
            .await
            .expect("bash_run returns ProcessOutput on nonzero exit");

        assert_eq!(res.exit_code, Some(42));
        assert!(!res.timed_out);
    }

    #[tokio::test]
    async fn test_bash_run_cwd_success() {
        let temp_dir = std::env::temp_dir();
        let res = bash_run("pwd -P", Some(&temp_dir), 5000)
            .await
            .expect("bash_run with valid cwd should succeed");

        assert_eq!(res.exit_code, Some(0));
        let reported = res.stdout.trim();
        let canonical = std::fs::canonicalize(&temp_dir).expect("canonicalize temp dir");
        assert_eq!(reported, canonical.to_string_lossy());
    }

    #[tokio::test]
    async fn test_bash_run_invalid_cwd() {
        let non_existent = Path::new("/non/existent/path/for/unit/test/process_run");
        let res = bash_run("echo hi", Some(non_existent), 5000).await;

        assert!(res.is_err());
        match res.unwrap_err() {
            ProcessError::InvalidCwd(p) => {
                assert_eq!(p, non_existent);
            }
            other => panic!("Expected InvalidCwd, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_bash_run_timeout_kills_process() {
        let start = Instant::now();
        let res = bash_run("sleep 10", None, 250)
            .await
            .expect("bash_run should return timed_out ProcessOutput");
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 1000,
            "Timeout must terminate in under 1s, took {}ms",
            elapsed.as_millis()
        );
        assert!(res.timed_out);
        assert_eq!(res.exit_code, None);
    }

    #[tokio::test]
    async fn test_bash_run_timeout_kills_child_processes() {
        let start = Instant::now();
        let res = bash_run("bash -c 'sleep 10'", None, 250)
            .await
            .expect("bash_run should return timed_out ProcessOutput");
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 1000,
            "Process group kill must terminate child and grandchildren in under 1s, took {}ms",
            elapsed.as_millis()
        );
        assert!(res.timed_out);
        assert_eq!(res.exit_code, None);
    }

    #[tokio::test]
    async fn test_bash_run_empty_command() {
        let res = bash_run("", None, 5000)
            .await
            .expect("empty command succeeds");

        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.is_empty());
        assert!(res.stderr.is_empty());
        assert!(!res.timed_out);
    }

    #[tokio::test]
    async fn test_bash_run_syntax_error() {
        let res = bash_run("if then fi", None, 5000)
            .await
            .expect("syntax error returns ProcessOutput with nonzero exit code");

        assert_ne!(res.exit_code, Some(0));
        assert!(!res.stderr.is_empty());
    }

    #[tokio::test]
    async fn test_bash_run_large_stream_no_deadlock() {
        let script = "for i in $(seq 1 4000); do echo \"line $i\"; done";
        let res = bash_run(script, None, 10000)
            .await
            .expect("large stream should not deadlock");

        assert_eq!(res.exit_code, Some(0));
        assert!(res.stdout.contains("line 1\n"));
        assert!(res.stdout.contains("line 4000\n"));
    }

    #[test]
    fn test_process_output_serde() {
        let out = ProcessOutput {
            exit_code: Some(0),
            stdout: "hello".to_string(),
            stderr: String::new(),
            combined: "hello".to_string(),
            timed_out: false,
        };

        let json = serde_json::to_string(&out).expect("serialize");
        let deserialized: ProcessOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(out, deserialized);
    }

    #[test]
    fn test_process_error_display() {
        let err = ProcessError::InvalidCwd(PathBuf::from("/invalid/path"));
        assert!(err.to_string().contains("/invalid/path"));

        let timeout_err = ProcessError::Timeout {
            timeout_ms: 500,
            stdout: "out".to_string(),
            stderr: "err".to_string(),
        };
        assert!(timeout_err.to_string().contains("500ms"));
    }
}
