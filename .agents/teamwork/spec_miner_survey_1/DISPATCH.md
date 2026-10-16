# Survey Task: Security Gatekeeper (R4) and LLM Tool Declarations (R5)

## Working Directory
/home/joao/Projects/rusi/.agents/teamwork/spec_miner_survey_1

## Authoritative Request
/home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md

## Objective
Analyze technical specifications, existing patterns, and implementation requirements for:
1. R4: Security & Permissions Gatekeeper in `src/cortex/permissions.rs` (or `src/tools/gate.rs`):
   - What is the existing structure of `src/cortex/` and configuration? Is there an `AgentMode` enum (`Plan`, `AcceptEdits`, `Auto`) already defined, or where should it be?
   - How `--dangerously-skip-permissions` is structured in config / CLI flags.
   - Classification of destructive operations (write, edit, bash execution vs read, search).
   - Prompting logic for user confirmation `[y/N]` in `AcceptEdits` or `Auto` mode when permissions are required.
   - Blocking write/exec in `Plan` mode.
2. R5: LLM Tool Declarations in `src/llm/types.rs`:
   - Inspect `src/llm/types.rs`: how `ToolDefinition` is defined, what fields it has (name, description, parameters JSON schema).
   - How native tools (`file_read`, `file_write`, `file_edit`, `bash_run`, `code_search`) should be exposed as `ToolDefinition`s.
   - Provide concrete JSON schemas for each tool's arguments to ensure complete LLM compatibility.
3. Acceptance criteria & testing requirements for R4 and R5.
4. Produce a comprehensive report in `handoff.md` in your working directory.


## 2026-10-16T19:50:16Z
You are spec_miner_survey_1. Your working directory is /home/joao/Projects/rusi/.agents/teamwork/spec_miner_survey_1.
You MUST read /home/joao/Projects/rusi/.agents/teamwork/ORIGINAL_REQUEST.md and /home/joao/Projects/rusi/.agents/teamwork/spec_miner_survey_1/DISPATCH.md.
Investigate the requirements and design for R4 and R5:
- R4: Security & Permissions Gatekeeper in src/cortex/permissions.rs (or src/tools/gate.rs). Examine how AgentMode (Plan, AcceptEdits, Auto) is defined or where it should be, --dangerously-skip-permissions config handling, destructive operation classification, and confirmation prompt [y/N] logic.
- R5: LLM Tool Declarations in src/llm/types.rs. Examine ToolDefinition, JSON schema structures, and how all 5 native tools (file_read, file_write, file_edit, bash_run, code_search) must declare their parameters.
- Provide concrete JSON schemas for each tool's parameters.
Write a structured handoff report to /home/joao/Projects/rusi/.agents/teamwork/spec_miner_survey_1/handoff.md. Update progress.md with your liveness heartbeat. When finished, send a message to the caller with your findings summary and confirmation of the report path.
