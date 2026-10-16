# Original User Request

## Initial Request — 2026-10-16T19:46:51Z

Implement the native toolset (Built-in Tools) and the security gatekeeper for Rusi, an autonomous agent orchestrator written in Rust, completing Phase 3 of the project roadmap.

Working directory: /home/joao/Projects/rusi
Integrity mode: demo

## Requirements

### R1. Filesystem Tools
Implement native filesystem tools in `src/tools/fs.rs`:
- `file_read(path, start_line, end_line)`: Reads lines from a file with slice range support and line numbering.
- `file_write(path, content, overwrite)`: Writes content to a file, creating parent directories as needed and guarding against accidental overwrite unless explicitly requested.
- `file_edit(path, target_block, replacement_block)`: Modifies a contiguous block in a file by locating the exact target block and replacing it.

### R2. Command Execution Tool
Implement process execution in `src/tools/process.rs`:
- `bash_run(command, cwd, timeout_ms)`: Executes shell commands asynchronously using `tokio::process`, captures combined stdout and stderr, enforces strict execution timeouts with clean child process termination, and returns the exit status code.

### R3. Fast Workspace Search
Implement workspace code search in `src/tools/search.rs`:
- `code_search(pattern, path)`: Performs high-speed regex search over files respecting `.gitignore` rules and hidden files using the `ignore` and `regex` crates, returning file matches with line numbers and matched content snippets.

### R4. Security & Permissions Gatekeeper
Implement the security gate in `src/cortex/permissions.rs` (or `src/tools/gate.rs`):
- Intercept destructive commands (e.g. file writes, file edits, bash execution) before execution.
- If `--dangerously-skip-permissions` is enabled in configuration, allow execution without prompting.
- If the current agent mode is `Plan`, block any write or execution operations.
- In `AcceptEdits` or `Auto` mode when permissions are required, prompt the user for interactive confirmation `[y/N]`.

### R5. LLM Tool Declarations
Expose all native tools via `ToolDefinition` (from `src/llm/types.rs`), providing complete JSON schema parameters and descriptions so they can be dispatched by the LLM providers implemented in Phase 2.

## Acceptance Criteria

### Functionality & Tests
- [ ] `cargo test` passes with unit tests for each tool: filesystem read/write/edit, process execution with timeout, and gitignore-aware regex code search.
- [ ] Subprocess execution timeout in `bash_run` properly kills long-running processes (e.g. `sleep 10` with 500ms timeout terminates in under 1s).
- [ ] `code_search` correctly ignores files listed in `.gitignore` and matches regex patterns.
- [ ] `file_edit` returns a descriptive error when the `target_block` is not found or is ambiguous.
- [ ] Security gate blocks file writes and command executions when in `AgentMode::Plan`.
- [ ] `cargo clippy -- -D warnings` finishes with zero errors or warnings.
- [ ] The implementation log (`IMPLEMENTATION_LOG.md`) and `ROADMAP.md` are updated to reflect Phase 3 completion.
