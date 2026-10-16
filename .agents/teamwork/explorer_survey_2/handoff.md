# Handoff Report: Tools Technical Requirements Survey (R1, R2, R3)

**Author**: `explorer_survey_2`  
**Milestone**: Phase 3 Survey & Architectural Design (R1, R2, R3)  
**Date**: 2026-10-16  

---

## Executive Summary
This survey establishes the complete technical specification, architectural design, and unit testing strategy for Rusi's native built-in tools: Filesystem Tools (`src/tools/fs.rs`), Process Execution (`src/tools/process.rs`), and Fast Workspace Code Search (`src/tools/search.rs`). All required dependencies (`tokio`, `ignore`, `regex`, `serde`, `serde_json`, `thiserror`) are already configured in `Cargo.toml` without requiring new external crates. A critical empirical finding was uncovered regarding subprocess termination: killing the parent bash PID alone leaves grandchild processes orphaned, requiring process group signaling (`process_group(0)` + `kill(-pgid, SIGKILL)`) to satisfy the sub-second timeout requirement.

---

## 1. Observation

### 1.1 Existing Dependencies (`Cargo.toml`)
Inspecting `/home/joao/Projects/rusi/Cargo.toml` (lines 8–38):
- Runtime: `tokio = { version = "1.43", features = ["full"] }` (provides `tokio::process`, `tokio::time`, `tokio::io`)
- Serialization: `serde = { version = "1.0", features = ["derive"] }`, `serde_json = "1.0"`
- Search & Walk: `ignore = "0.4"`, `regex = "1.11"`
- Error management: `thiserror = "2.0.21"`
- Edition: `2024`

### 1.2 Codebase Baseline & Module Layout
- `src/main.rs`: lines 1–3 declare `pub mod cli;`, `pub mod config;`, `pub mod llm;`. The `src/tools/` directory does not yet exist.
- `src/llm/types.rs`: lines 90–94 define `ToolDefinition`:
  ```rust
  pub struct ToolDefinition {
      pub name: String,
      pub description: String,
      pub parameters: serde_json::Value,
  }
  ```
  Lines 16–28 define `ToolCall` and `ToolResult`.
- Current test status: `cargo test` executes 11 passed tests in 0.00s. `cargo clippy -- -D warnings` finishes with 0 warnings. `cargo check` builds in 0.72s.

### 1.3 Empirical Process Termination Verification
When running `bash -c "sleep 10"` via standard child process execution:
- Executing `child.kill()` on the parent `bash` process terminates the bash PID, but leaves the spawned `sleep 10` process running in the background as an orphan (observed remaining process PID: 1298608).
- Executing with a dedicated process group (`setpgid(0, 0)`) and sending `SIGKILL` to the negative process group ID (`-pgid`) cleanly terminates both `bash` and the grandchild `sleep 10` process in under 0.50s without any lingering orphan processes.

---

## 2. Logic Chain

### 2.1 R1: Filesystem Tools Design (`src/tools/fs.rs`)

#### A. `file_read(path, start_line, end_line)`
- **Line Indexing**: 1-indexed. Line 1 is the first line of the file.
- **Input Parameters**:
  - `path: impl AsRef<Path>`
  - `start_line: Option<usize>` (defaults to 1; if `Some(0)` passed, clamp to 1)
  - `end_line: Option<usize>` (defaults to total line count)
- **Slice & Bounds Semantics**:
  - If file does not exist: return `Err(FsError::FileNotFound(path))`.
  - Read content using `std::fs::read_to_string`.
  - If file is empty: return `Ok(String::new())`.
  - Calculate `total_lines = content.lines().count()`.
  - If `start > total_lines`: return `Err(FsError::LineOutOfRange { requested: start, total: total_lines })`.
  - If `start > end`: return `Err(FsError::InvalidLineRange { start, end })`.
  - Clamp `end = end.min(total_lines)`.
- **Output Formatting**:
  - Each line is prefixed with its 1-indexed line number: `"{line_no}: {line}\n"`.
  - Example:
    ```text
    1: [package]
    2: name = "rusi"
    3: version = "0.1.0"
    ```

#### B. `file_write(path, content, overwrite)`
- **Input Parameters**:
  - `path: impl AsRef<Path>`
  - `content: &str`
  - `overwrite: bool`
- **Parent Directory Creation**:
  - Extract `path.parent()`. If non-empty and does not exist, call `std::fs::create_dir_all(parent)?`.
- **Overwrite Guard**:
  - If `path.exists() && !overwrite`: return `Err(FsError::FileAlreadyExists(path))`.
- **Write Operation**:
  - Call `std::fs::write(path, content)?`.
  - Return `Ok(())`.

#### C. `file_edit(path, target_block, replacement_block)`
- **Input Parameters**:
  - `path: impl AsRef<Path>`
  - `target_block: &str`
  - `replacement_block: &str`
- **Validation**:
  - If `target_block.is_empty()`: return `Err(FsError::EmptyTargetBlock)`.
- **Block Matching & Ambiguity Handling**:
  - Read content from file.
  - Count exact occurrences: `let count = content.matches(target_block).count();`
  - If `count == 0`: return `Err(FsError::TargetBlockNotFound(path))`.
  - If `count > 1`: return `Err(FsError::AmbiguousTargetBlock { path, count })`.
  - If `count == 1`:
    - Perform single replacement: `let updated = content.replacen(target_block, replacement_block, 1);`
    - Write `updated` back to `path` via `std::fs::write(path, updated)?`.
    - Return `Ok(())`.

#### D. Error Definitions (`FsError`)
```rust
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("File not found: {0}")]
    FileNotFound(std::path::PathBuf),

    #[error("File already exists: '{0}'. Pass overwrite=true to overwrite.")]
    FileAlreadyExists(std::path::PathBuf),

    #[error("Invalid line range: start_line ({start}) > end_line ({end})")]
    InvalidLineRange { start: usize, end: usize },

    #[error("Line out of range: requested line {requested}, but file only has {total} lines")]
    LineOutOfRange { requested: usize, total: usize },

    #[error("Target block not found in file: '{0}'")]
    TargetBlockNotFound(std::path::PathBuf),

    #[error("Ambiguous target block in file '{path}': found {count} occurrences")]
    AmbiguousTargetBlock { path: std::path::PathBuf, count: usize },

    #[error("Target block cannot be empty")]
    EmptyTargetBlock,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

### 2.2 R2: Process Execution Tool Design (`src/tools/process.rs`)

#### A. Function Signature & Return Types
```rust
pub async fn bash_run(
    command: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> Result<ProcessOutput, ProcessError>
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub combined: String,
    pub timed_out: bool,
}
```

#### B. Process Spawning & Working Directory
- Command: `tokio::process::Command::new("bash")`.
- Arguments: `.arg("-c").arg(command)`.
- Cwd handling: If `Some(dir)` is provided, verify `dir.is_dir()`. If invalid, return `Err(ProcessError::InvalidCwd(dir.to_path_buf()))`. Then apply `.current_dir(dir)`.
- Stdio configuration:
  - `cmd.stdout(std::process::Stdio::piped())`
  - `cmd.stderr(std::process::Stdio::piped())`

#### C. Process Group Setup & Timeout Termination Mechanics
- On Unix, call `cmd.process_group(0)` (`std::os::unix::process::CommandExt`) before spawn to create a new process group with PGID equal to the child PID.
- Spawn child: `let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;`
- Take stdout and stderr handles and stream them concurrently via `tokio::spawn` or `tokio::join!` with `tokio::io::AsyncReadExt::read_to_end` to prevent pipe buffer deadlocks.
- Wrap execution in `tokio::time::timeout(Duration::from_millis(timeout_ms), ...)`:
  - **Normal completion**:
    - Collect `stdout` string and `stderr` string.
    - Generate `combined`:
      - If `stderr` is empty: `stdout`
      - If `stdout` is empty: `stderr`
      - Otherwise: `format!("{stdout}\n{stderr}")`
    - Return `Ok(ProcessOutput { exit_code: status.code(), stdout, stderr, combined, timed_out: false })`.
  - **Timeout triggered**:
    - On Unix, kill the entire process group:
      ```rust
      #[cfg(unix)]
      if let Some(pid) = child.id() {
          unsafe {
              extern "C" {
                  fn kill(pid: i32, sig: i32) -> i32;
              }
              kill(-(pid as i32), 9); // SIGKILL to process group
          }
      }
      ```
    - Call `let _ = child.kill().await;` and `let _ = child.wait().await;` to reap the zombie child immediately.
    - Return `Ok(ProcessOutput { exit_code: None, stdout: partial_out, stderr: partial_err, combined: "Process timed out after ...ms", timed_out: true })` (or return `Err(ProcessError::Timeout { timeout_ms, stdout, stderr })`). Returning `ProcessOutput` with `timed_out: true` allows the ReAct loop and LLM to inspect any partial logs before the hang.

#### D. Error Definitions (`ProcessError`)
```rust
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Process timed out after {timeout_ms}ms")]
    Timeout {
        timeout_ms: u64,
        stdout: String,
        stderr: String,
    },

    #[error("Failed to spawn process: {0}")]
    Spawn(std::io::Error),

    #[error("Process I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Working directory does not exist or is not a directory: '{0}'")]
    InvalidCwd(std::path::PathBuf),
}
```

---

### 2.3 R3: Workspace Code Search Tool Design (`src/tools/search.rs`)

#### A. Function Signature & Return Types
```rust
pub fn code_search(
    pattern: &str,
    path: impl AsRef<Path>,
) -> Result<Vec<SearchMatch>, SearchError>
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub file: String,
    pub line_number: usize,
    pub line_content: String,
}
```

```rust
pub fn format_search_results(matches: &[SearchMatch]) -> String
```

#### B. Regex & Search Traversal
- Compile regex: `let regex = Regex::new(pattern).map_err(|e| SearchError::InvalidRegex { pattern: pattern.to_string(), source: e })?;`
- Validate search path: If `!path.as_ref().exists()`, return `Err(SearchError::PathNotFound(path.as_ref().to_path_buf()))`.
- Build directory walker using `ignore::WalkBuilder`:
  ```rust
  let walker = ignore::WalkBuilder::new(root)
      .hidden(true)       // Ignore hidden files and directories (.git, .env)
      .git_ignore(true)   // Respect .gitignore files in current and parent directories
      .git_global(true)   // Respect global gitignore
      .git_exclude(true)  // Respect .git/info/exclude
      .build();
  ```
- File iteration:
  - For each `DirEntry`: check `entry.path().is_file()`.
  - Open file with `std::fs::File::open`.
  - Read line-by-line using `std::io::BufReader::new(file).lines()`.
  - Skip non-UTF-8 / binary files gracefully (break on invalid UTF-8 read error without aborting search).
  - On regex match: push `SearchMatch { file: entry.path().display().to_string(), line_number: idx + 1, line_content: line }`.
  - Guard against unbounded output: enforce `const MAX_MATCHES: usize = 1000`.

#### C. Error Definitions (`SearchError`)
```rust
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("Invalid regular expression '{pattern}': {source}")]
    InvalidRegex {
        pattern: String,
        #[source]
        source: regex::Error,
    },

    #[error("Path not found: '{0}'")]
    PathNotFound(std::path::PathBuf),

    #[error("I/O error during search: {0}")]
    Io(#[from] std::io::Error),
}
```

---

### 2.4 Unified Module Architecture (`src/tools/mod.rs`)

`src/tools/` structure:
```text
src/tools/
├── mod.rs
├── fs.rs
├── process.rs
└── search.rs
```

In `src/tools/mod.rs`:
```rust
pub mod fs;
pub mod process;
pub mod search;

pub use fs::{file_edit, file_read, file_write, FsError};
pub use process::{bash_run, ProcessError, ProcessOutput};
pub use search::{code_search, format_search_results, SearchError, SearchMatch};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Filesystem error: {0}")]
    Fs(#[from] FsError),
    #[error("Process execution error: {0}")]
    Process(#[from] ProcessError),
    #[error("Search error: {0}")]
    Search(#[from] SearchError),
}
```

In `src/main.rs`:
Add `pub mod tools;` alongside `cli`, `config`, and `llm`.

---

### 2.5 Acceptance Criteria & Unit Test Matrix

| Acceptance Criterion | Target Module | Unit Test Implementation |
|---|---|---|
| `file_read` slice & line numbers | `src/tools/fs.rs` | `test_file_read_slice_and_line_numbers`: verify lines 2–4 in a 5-line file are numbered `2: ...`, `3: ...`, `4: ...`; verify `start > total_lines` returns `LineOutOfRange`. |
| `file_write` overwrite protection | `src/tools/fs.rs` | `test_file_write_overwrite_guard`: write file once; second write with `overwrite=false` fails with `FileAlreadyExists`; third write with `overwrite=true` succeeds; non-existent parent dir created automatically. |
| `file_edit` ambiguous & not-found errors | `src/tools/fs.rs` | `test_file_edit_unique_and_ambiguous`: file with duplicate words returns `AmbiguousTargetBlock`; missing block returns `TargetBlockNotFound`; unique block replaces accurately. |
| Subprocess timeout kills long-running processes | `src/tools/process.rs` | `test_bash_run_timeout_kills_process`: run `sleep 10` with `timeout_ms = 500`. Assert elapsed time < 1000ms. Assert child/grandchildren processes are fully killed (no lingering PID). |
| Command execution stdout/stderr & exit code | `src/tools/process.rs` | `test_bash_run_success_and_exit_code`: execute `echo hello && echo error >&2`. Assert exit code `Some(0)`, combined output contains both streams. Execute `exit 42`, assert exit code `Some(42)`. |
| Gitignore-aware regex code search | `src/tools/search.rs` | `test_code_search_respects_gitignore`: create dir with `.gitignore` ignoring `ignored.txt` and `node_modules/`. Verify pattern match only finds `visible.txt`. Verify invalid regex returns `InvalidRegex`. |
| `cargo clippy -- -D warnings` | All modules | Clippy run passes with 0 warnings. |

---

## 3. Caveats
- **Windows Subprocess Semantics**: On Windows, Unix process groups (`std::os::unix::process::CommandExt`) are unavailable. On Windows, `tokio::process::Child::kill()` terminates the immediate process, but killing descendant trees requires Windows Job Objects. Since the primary target OS is Linux, Unix process groups provide 100% reliability with zero external dependencies via standard `extern "C" { fn kill(...) }`.
- **Shell Availability**: `bash_run` relies on `/bin/bash` or `bash` in the system `PATH`. On standard Linux environments, bash is always present.
- **Large Result Truncation**: When running regex search on a large codebase, returning hundreds of thousands of lines could overwhelm memory or token contexts. A `MAX_MATCHES = 1000` cap is established to safeguard execution.

---

## 4. Conclusion
The requirements for R1, R2, and R3 are fully resolved, architecturally sound, and ready for immediate implementation:
1. `src/tools/fs.rs`: Implements `file_read`, `file_write`, and `file_edit` with strict line-indexing, directory auto-creation, overwrite protection, and occurrence-based ambiguity detection.
2. `src/tools/process.rs`: Implements `bash_run` with concurrent stdout/stderr streaming and process-group SIGKILL termination, guaranteeing compliance with the sub-second timeout acceptance criterion.
3. `src/tools/search.rs`: Implements `code_search` using `ignore::WalkBuilder` and `regex::Regex` to achieve ripgrep-grade gitignore-compliant workspace search.
4. All needed crates (`tokio`, `ignore`, `regex`, `thiserror`, `serde`) are already verified in `Cargo.toml`.

---

## 5. Verification Method

### 5.1 Independent Codebase & Compilation Verification
1. Run `cargo check` to confirm clean compilation:
   ```bash
   cargo check
   ```
2. Run `cargo test` to confirm all 11 existing unit tests continue to pass:
   ```bash
   cargo test
   ```
3. Run `cargo clippy` to confirm strict lint compliance:
   ```bash
   cargo clippy -- -D warnings
   ```

### 5.2 Test Implementation Invalidation Conditions
- If `bash_run("sleep 10", None, 500)` takes >= 1000ms or leaves background `sleep` processes running, the implementation fails acceptance criterion 2.
- If `code_search` returns matches from `.git/` or files declared in `.gitignore`, the implementation fails acceptance criterion 3.
- If `file_edit` does not error when a target block matches multiple times, the implementation fails acceptance criterion 4.
