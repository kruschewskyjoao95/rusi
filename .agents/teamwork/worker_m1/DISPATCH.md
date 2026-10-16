# Dispatch: Milestone 1 — Filesystem Tools (R1)

## Working Directory
/home/joao/Projects/rusi/.agents/teamwork/worker_m1

## Authoritative User Request
/home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md

## Scope & Design Inputs
- /home/joao/Projects/rusi/PROJECT.md (Milestone 1, Interface Contracts)
- /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2/handoff.md (Section 2.1: Full specifications and error types for fs.rs)

## Exclusive File Ownership
You exclusively own and may create/modify:
- `src/tools/fs.rs`
- `src/tools/mod.rs` (declare `pub mod fs; pub use fs::*;`)
- `src/lib.rs` (create `src/lib.rs` declaring `pub mod tools; pub mod config; pub mod cli; pub mod llm;` so integration tests can link)
- `src/main.rs` (add `pub mod tools;` or use `rusi::*`)

## Mandatory Integrity Warning
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

## Requirements
Implement native filesystem tools in `src/tools/fs.rs`:
1. `file_read(path, start_line, end_line) -> Result<String, FsError>`:
   - Reads lines from a file with slice range support.
   - 1-indexed line numbers formatted as `"{line_no}: {line}\n"`.
   - Returns empty string if file is empty.
   - Returns descriptive error `FsError::LineOutOfRange` if `start_line > total_lines`.
   - Returns descriptive error `FsError::InvalidLineRange` if `start_line > end_line`.
   - Returns `FsError::FileNotFound` if path doesn't exist.
2. `file_write(path, content, overwrite) -> Result<(), FsError>`:
   - Writes content to a file.
   - Automatically creates parent directories if they do not exist (`std::fs::create_dir_all`).
   - Guards against accidental overwrite: returns `FsError::FileAlreadyExists` if file exists and `overwrite == false`.
   - Overwrites if `overwrite == true`.
3. `file_edit(path, target_block, replacement_block) -> Result<(), FsError>`:
   - Modifies a contiguous block in a file by locating the exact target block.
   - If `target_block` is empty, returns `FsError::EmptyTargetBlock`.
   - If `target_block` is not found, returns descriptive error `FsError::TargetBlockNotFound`.
   - If `target_block` appears more than once, returns descriptive error `FsError::AmbiguousTargetBlock { path, count }`.
   - If found exactly once, replaces it and writes back to file.
4. `FsError` enum deriving `thiserror::Error`.
5. Comprehensive unit tests within `src/tools/fs.rs` covering:
   - Line numbering & slice ranges (middle lines, clamping end line, out of range).
   - Empty file read.
   - Auto parent directory creation on write.
   - Overwrite protection (`overwrite=false` fails if file exists, `overwrite=true` succeeds).
   - Edit single unique match.
   - Edit not-found returns `TargetBlockNotFound`.
   - Edit duplicate/ambiguous matches returns `AmbiguousTargetBlock`.
   - Empty target block returns `EmptyTargetBlock`.

Verify your implementation with `cargo test` and `cargo clippy -- -D warnings`.
Report all results in `handoff.md` in your working directory.


## 2026-10-16T19:58:12Z
[Message] timestamp=2026-10-16T19:58:12Z sender=96fbb631-8e43-4049-8fe9-1d581ea7d4c7 priority=MESSAGE_PRIORITY_HIGH content=You are worker_m1. Your working directory is /home/joao/Projects/rusi/.agents/teamwork/worker_m1.
You MUST read /home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md and /home/joao/Projects/rusi/.agents/teamwork/worker_m1/DISPATCH.md.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A teamwork_preview_auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Your exclusive file ownership:
- src/tools/fs.rs
- src/tools/mod.rs (declare pub mod fs; pub use fs::*; )
- src/lib.rs (create src/lib.rs declaring pub mod tools; pub mod config; pub mod cli; pub mod llm; so integration tests link)
- src/main.rs (use rusi::* or declare pub mod tools;)

Implement Milestone 1 (R1 Filesystem Tools):
- file_read(path, start_line, end_line) with 1-indexed numbering "{i}: {line}\n", slice bounds, and FsError::LineOutOfRange / InvalidLineRange.
- file_write(path, content, overwrite) with parent directory auto-creation and FsError::FileAlreadyExists guard.
- file_edit(path, target_block, replacement_block) with exact unique block replacement, FsError::TargetBlockNotFound, and FsError::AmbiguousTargetBlock.
- FsError with thiserror.
- Comprehensive unit tests inside src/tools/fs.rs.

Run `cargo test` and `cargo clippy -- -D warnings`.
Write a structured handoff report to /home/joao/Projects/rusi/.agents/teamwork/worker_m1/handoff.md. Update progress.md with your liveness heartbeat. When done, send a message to caller with summary and report path.
