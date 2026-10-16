# Dispatch: Milestone 2 — Process Execution Tool (R2)

## Working Directory
/home/joao/Projects/rusi/.agents/teamwork/worker_m2

## Authoritative User Request
/home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md

## Scope & Design Inputs
- /home/joao/Projects/rusi/PROJECT.md (Milestone 2, Interface Contracts)
- /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2/handoff.md (Section 2.2: Process Execution Tool Design & Empirical Process Group Termination)
- /home/joao/Projects/rusi/tests/e2e_process.rs (E2E test suite for Milestone 2)

## Exclusive File Ownership
You exclusively own and may create/modify:
- `src/tools/process.rs`
- `src/tools/mod.rs` (export `pub mod process; pub use process::*;`)

## Mandatory Integrity Warning
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

## Requirements
Implement process execution in `src/tools/process.rs`:
1. `pub async fn bash_run(command: &str, cwd: Option<&Path>, timeout_ms: u64) -> Result<ProcessOutput, ProcessError>`:
   - Uses `tokio::process::Command::new("bash")`.
   - Arguments: `-c`, `command`.
   - Validates `cwd`: if `Some(dir)` is provided and does not exist or is not a directory, returns `Err(ProcessError::InvalidCwd(dir.to_path_buf()))`.
   - Uses `tokio::process::CommandExt::process_group(0)` on Unix to create a dedicated process group.
   - Piped stdout and stderr, read concurrently using `tokio::io::AsyncReadExt::read_to_end` to prevent buffer deadlocks.
   - Wraps execution in `tokio::time::timeout(Duration::from_millis(timeout_ms))`.
   - On timeout:
     - On Unix, kills the entire process group with `kill(-(pid as i32), 9)` (SIGKILL) so child processes (like `sleep 10`) are not orphaned.
     - Calls `child.kill().await` and `child.wait().await` to reap the zombie.
     - Returns either `Ok(ProcessOutput { exit_code: None, stdout, stderr, combined, timed_out: true })` or `Err(ProcessError::Timeout)`. Note: `tests/e2e_process.rs` tests `res.unwrap()` with `res.timed_out == true` or `Err(ProcessError::Timeout)`. Check `tests/e2e_process.rs` for the exact signature and behavior expected.
   - On success:
     - Returns `ProcessOutput { exit_code: status.code(), stdout, stderr, combined, timed_out: false }`.
     - `combined` format: if stdout and stderr both non-empty, `format!("{stdout}\n{stderr}")` or if one is empty, the other.
2. Structs and Enums:
   - `ProcessOutput { exit_code: Option<i32>, stdout: String, stderr: String, combined: String, timed_out: bool }` (deriving Debug, Clone, PartialEq, Eq, Serialize, Deserialize).
   - `ProcessError` deriving `thiserror::Error` with variants `Timeout`, `Spawn(std::io::Error)`, `Io(std::io::Error)`, `InvalidCwd(PathBuf)`.
3. Unit tests within `src/tools/process.rs`:
   - Echo command stdout capture.
   - Stderr capture and combined stream.
   - Non-zero exit codes.
   - Timeout killing `sleep 10` in <1000ms.
   - Invalid cwd error.
   - Large stream without buffer deadlock.
4. Verify with `cargo test`, `RUSTFLAGS="--cfg milestone_2" cargo test --test e2e_process`, and `cargo clippy -- -D warnings`.
5. Write your handoff report to `handoff.md` in your working directory and notify the caller.

## 2026-10-16T20:10:29Z
You are worker_m2. Your working directory is /home/joao/Projects/rusi/.agents/teamwork/worker_m2.
You MUST read /home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md and /home/joao/Projects/rusi/.agents/teamwork/worker_m2/DISPATCH.md.
Also inspect /home/joao/Projects/rusi/PROJECT.md, /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2/handoff.md, and /home/joao/Projects/rusi/tests/e2e_process.rs.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Your exclusive file ownership:
- src/tools/process.rs
- src/tools/mod.rs (declare pub mod process; pub use process::*; and pub mod fs; pub use fs::*; plus ToolError)

Implement Milestone 2 (R2 Process Execution Tool):
- bash_run(command, cwd, timeout_ms) -> Result<ProcessOutput, ProcessError>
- ProcessOutput { exit_code: Option<i32>, stdout: String, stderr: String, combined: String, timed_out: bool }
- ProcessError (thiserror: Timeout, Spawn, Io, InvalidCwd)
- Dedicated Unix process group creation via CommandExt::process_group(0)
- Process group SIGKILL (-pgid) on timeout to clean up child processes (e.g. sleep 10) in <1000ms
- Concurrent stdout/stderr streaming via tokio::io::AsyncReadExt::read_to_end
- Comprehensive unit tests inside src/tools/process.rs
- Verify that `cargo test`, `RUSTFLAGS="--cfg milestone_2" cargo test --test e2e_process`, and `cargo clippy -- -D warnings` all pass with 0 errors and 0 warnings.

Write handoff.md in your working directory and notify the caller when done.
