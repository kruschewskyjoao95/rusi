# Project: Rusi Phase 3 — Native Toolset and Security Gatekeeper

## Architecture
Rusi is an autonomous agent orchestrator written in Rust. In Phase 3, native tools and safety mechanisms are introduced to enable the agent to interact safely and efficiently with the host operating system.

### Module Boundaries & Data Flow
```
                  ┌──────────────────────┐
                  │    Agent / LLM       │
                  │   (src/llm/types)    │
                  └──────────┬───────────┘
                             │ ToolCall (name, arguments)
                             ▼
                  ┌──────────────────────┐
                  │   Permissions Gate   │
                  │(src/cortex/permission│
                  └──────────┬───────────┘
           Approved? (Plan / Auto / AcceptEdits)
                             │
       ┌─────────────────────┼─────────────────────┐
       ▼                     ▼                     ▼
┌──────────────┐     ┌───────────────┐     ┌───────────────┐
│  fs tools    │     │  process tool │     │  search tool  │
│(src/tools/fs)│     │(src/tools/proc│     │(src/tools/sea)│
└──────────────┘     └───────────────┘     └───────────────┘
```

1. **LLM Tool Layer (`src/llm/types.rs`)**: Exposes `ToolDefinition` schemas for all 5 native tools (`file_read`, `file_write`, `file_edit`, `bash_run`, `code_search`) to Gemini and future providers.
2. **Security Gatekeeper (`src/cortex/permissions.rs`)**: Intercepts tool calls before execution. Enforces `AgentMode` policies (`Plan` blocks all destructive operations, `AcceptEdits`/`Auto` prompt for confirmation or bypass if `dangerously_skip_permissions` is set).
3. **Native Toolset (`src/tools/`)**:
   - `fs.rs`: Line-numbered reading with slicing, atomic writing with parent directory creation and overwrite protection, unique contiguous block editing.
   - `process.rs`: Asynchronous bash execution with combined stdout/stderr capture and process-group SIGKILL termination on timeout.
   - `search.rs`: Fast `.gitignore`-aware regular expression workspace code search.

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | `file_read` | 1-indexed line reading with slice bounds (start_line, end_line), formatting lines as `"{i}: {line}"`. | M1 | Survey R1 |
| 2 | `file_write` | Atomic file write with parent directory auto-creation and overwrite guard (`overwrite=false` fails if file exists). | M1 | Survey R1 |
| 3 | `file_edit` | Exact unique target block replacement; returns descriptive errors for missing block or ambiguous matches (>1). | M1 | Survey R1 |
| 4 | `bash_run` | Asynchronous command execution using `tokio::process::Command`, combined stdout/stderr, working directory support. | M2 | Survey R2 |
| 5 | `bash_run` timeout kill | Strict timeout enforcement using Unix process groups (`SIGKILL` to `-pgid`) terminating in <1s (e.g. `sleep 10` with 500ms timeout). | M2 | Survey R2 |
| 6 | `code_search` | Ripgrep-style regex search respecting `.gitignore`, hidden files, and ignore files using `ignore::WalkBuilder` and `regex::Regex`. | M3 | Survey R3 |
| 7 | `SearchMatch` & formatting | Formatted search results with file paths, line numbers, and line snippets; 1000-match truncation safety. | M3 | Survey R3 |
| 8 | `is_destructive` classification | Categorize `file_read` and `code_search` as safe; `file_write`, `file_edit`, `bash_run` (and unknown) as destructive. | M4 | Survey R4 |
| 9 | `PermissionsGate::evaluate` | Pure policy evaluation based on `AgentMode` and `dangerously_skip_permissions`. | M4 | Survey R4 |
| 10 | `Plan` mode strict blocking | Blocks all destructive operations in `AgentMode::Plan` unconditionally (cannot be bypassed by skip flag). | M4 | Survey R4 |
| 11 | Interactive confirmation `[y/N]` | Prompts user `[y/N]` for confirmation with fail-closed defaults; abstracted via `ConfirmationPrompter` trait for deterministic testability. | M4 | Survey R4 |
| 12 | `ToolDefinition` schemas | Complete OpenAPI 3.0 / JSON Schema parameter declarations for all 5 native tools in `src/llm/types.rs`. | M5 | Survey R5 |
| 13 | `ToolDefinition::all_native_tools` | Convenience aggregator returning definitions for all native tools for provider registration. | M5 | Survey R5 |
| 14 | Module exports & unifying `ToolError` | `src/tools/mod.rs` unifying `FsError`, `ProcessError`, `SearchError` into `ToolError`, exported in `src/main.rs`. | M5 | Survey R1-R5 |
| 15 | E2E Integration & Roadmap/Log updates | Full integration tests, `ROADMAP.md` checkmarks, `IMPLEMENTATION_LOG.md` Phase 3 architectural documentation, zero clippy warnings. | M6 | Survey Criteria |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Filesystem Tools | `src/tools/fs.rs` (`file_read`, `file_write`, `file_edit`, `FsError`) + unit tests | none | DONE |
| M2 | Process Execution Tool | `src/tools/process.rs` (`bash_run`, `ProcessOutput`, `ProcessError`, process group kill) + unit tests | none | IN_PROGRESS |
| M3 | Workspace Search Tool | `src/tools/search.rs` (`code_search`, `SearchMatch`, `SearchError`, gitignore traversal) + unit tests | none | PLANNED |
| M4 | Security & Permissions Gatekeeper | `src/cortex/permissions.rs`, `src/cortex/mod.rs` (`PermissionsGate`, `ConfirmationPrompter`, `PermissionError`) + unit tests | none | PLANNED |
| M5 | Tool Declarations & Integration | `src/llm/types.rs` (`ToolDefinition` constructors), `src/tools/mod.rs`, `src/main.rs` wiring | M1, M2, M3, M4 | PLANNED |
| M6 | E2E Testing, Docs & Final Acceptance | `tests/`, `ROADMAP.md`, `IMPLEMENTATION_LOG.md`, full `cargo test` and `cargo clippy -- -D warnings` | M5 | PLANNED |

## Interface Contracts

### M1 `src/tools/fs.rs`
```rust
pub fn file_read(path: impl AsRef<Path>, start_line: Option<usize>, end_line: Option<usize>) -> Result<String, FsError>;
pub fn file_write(path: impl AsRef<Path>, content: &str, overwrite: bool) -> Result<(), FsError>;
pub fn file_edit(path: impl AsRef<Path>, target_block: &str, replacement_block: &str) -> Result<(), FsError>;
```

### M2 `src/tools/process.rs`
```rust
pub async fn bash_run(command: &str, cwd: Option<&Path>, timeout_ms: u64) -> Result<ProcessOutput, ProcessError>;

pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub combined: String,
    pub timed_out: bool,
}
```

### M3 `src/tools/search.rs`
```rust
pub fn code_search(pattern: &str, path: impl AsRef<Path>) -> Result<Vec<SearchMatch>, SearchError>;
pub fn format_search_results(matches: &[SearchMatch]) -> String;

pub struct SearchMatch {
    pub file: String,
    pub line_number: usize,
    pub line_content: String,
}
```

### M4 `src/cortex/permissions.rs`
```rust
pub struct PermissionsGate {
    pub mode: AgentMode,
    pub dangerously_skip_permissions: bool,
}

pub trait ConfirmationPrompter {
    fn confirm(&mut self, prompt: &str) -> std::io::Result<bool>;
}

impl PermissionsGate {
    pub fn new(mode: AgentMode, dangerously_skip_permissions: bool) -> Self;
    pub fn from_config(config: &crate::config::Config) -> Self;
    pub fn is_destructive(tool_name: &str) -> bool;
    pub fn evaluate(&self, tool_name: &str) -> GateDecision;
    pub fn authorize(&self, tool_name: &str, details: Option<&str>) -> Result<(), PermissionError>;
    pub fn authorize_with_prompter<P: ConfirmationPrompter>(&self, tool_name: &str, details: Option<&str>, prompter: &mut P) -> Result<(), PermissionError>;
}
```

### M5 `src/llm/types.rs` & `src/tools/mod.rs`
```rust
impl ToolDefinition {
    pub fn file_read() -> Self;
    pub fn file_write() -> Self;
    pub fn file_edit() -> Self;
    pub fn bash_run() -> Self;
    pub fn code_search() -> Self;
    pub fn all_native_tools() -> Vec<Self>;
}
```

## Code Layout
```
src/
├── main.rs                 # Declares pub mod cortex; pub mod tools;
├── cli.rs
├── config.rs               # AgentMode (Auto, Plan, AcceptEdits), Config
├── cortex/
│   ├── mod.rs              # Re-exports permissions
│   └── permissions.rs      # PermissionsGate, ConfirmationPrompter, GateDecision
├── tools/
│   ├── mod.rs              # Re-exports fs, process, search, ToolError
│   ├── fs.rs               # file_read, file_write, file_edit, FsError
│   ├── process.rs          # bash_run, ProcessOutput, ProcessError
│   └── search.rs           # code_search, SearchMatch, SearchError
└── llm/
    ├── types.rs            # ToolDefinition schema constructors
    ├── provider.rs
    ├── error.rs
    └── gemini/
```
