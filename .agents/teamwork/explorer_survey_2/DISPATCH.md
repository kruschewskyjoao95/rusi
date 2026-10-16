# Survey Task: Tools Technical Requirements (R1, R2, R3)

## Working Directory
/home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2

## Authoritative Request
/home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md

## Objective
Analyze technical specifications, existing tool patterns, and implementation requirements for:
1. R1: Filesystem Tools in `src/tools/fs.rs`:
   - `file_read(path, start_line, end_line)` (1-indexed vs 0-indexed, slice range, line numbers, error cases like non-existent file).
   - `file_write(path, content, overwrite)` (creating parent dirs, overwrite protection, error cases).
   - `file_edit(path, target_block, replacement_block)` (finding exact unique target block, ambiguous match error, not found error).
2. R2: Process Execution Tool in `src/tools/process.rs`:
   - `bash_run(command, cwd, timeout_ms)`: tokio::process::Command, combined stdout/stderr capture, strict timeout enforcement with process termination/kill, exit status code.
3. R3: Fast Workspace Search in `src/tools/search.rs`:
   - `code_search(pattern, path)`: regex search respecting `.gitignore` rules and hidden files using `ignore` and `regex` crates, formatted output with line numbers and snippets.
4. Acceptance criteria:
   - Specific unit tests required (timeout test `sleep 10` with 500ms timeout terminating in <1s, gitignore test, file_edit ambiguous error test).
5. Produce a comprehensive report in `handoff.md` in your working directory.

## 2026-10-16T19:50:16Z
[Message] timestamp=2026-10-16T19:50:16Z sender=96fbb631-8e43-4049-8fe9-1d581ea7d4c7 priority=MESSAGE_PRIORITY_HIGH content=You are explorer_survey_2. Your working directory is /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2.
You MUST read /home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md and /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2/DISPATCH.md.
Investigate the requirements and design for R1, R2, R3:
- R1: Filesystem Tools (file_read, file_write, file_edit in src/tools/fs.rs). Note line numbering, slice ranges, overwrite guards, exact unique block matching, ambiguous match error handling.
- R2: Process Execution Tool (bash_run in src/tools/process.rs). Note tokio::process usage, timeout handling (killing process group/child), combined stdout/stderr capture, exit code.
- R3: Workspace Code Search Tool (code_search in src/tools/search.rs). Note regex search respecting .gitignore and hidden files using ignore and regex crates.
- Check existing tool traits or modules in src/tools/.
Write a structured handoff report to /home/joao/Projects/rusi/.agents/teamwork/explorer_survey_2/handoff.md. Update progress.md with your liveness heartbeat. When finished, send a message to the caller with your findings summary and confirmation of the report path.
