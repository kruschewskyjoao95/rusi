# Handoff Report: Specification Mining for Security Gatekeeper (R4) & LLM Tool Declarations (R5)

**Author**: `spec_miner_survey_1` (Specification Miner)  
**Date**: 2026-10-16T19:55:00Z  
**Target Milestone**: Phase 3 — Native Toolset & Security Gatekeeper (Requirements R4 and R5)  
**Workspace**: `/home/joao/Projects/rusi`  

---

## 1. Observation

### 1.1 Existing Codebase & Architecture
- **Rust Toolchain & Build**: Cargo workspace configured with Edition 2024 (`Cargo.toml:4`). `cargo test` currently runs 11 unit tests across `config`, `cli`, and `llm` with 100% success (11 passed, 0 failed). `cargo clippy -- -D warnings` completes with zero errors or warnings.
- **Top-Level Modules** (`src/main.rs:1-3`):
  ```rust
  pub mod cli;
  pub mod config;
  pub mod llm;
  ```
  Neither `src/cortex/` nor `src/tools/` currently exists in `src/`.
- **Existing `AgentMode` Definition** (`src/config.rs:24-35`):
  `AgentMode` is already fully defined with serde and clap derives:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
  #[serde(rename_all = "kebab-case")]
  #[value(rename_all = "kebab-case")]
  pub enum AgentMode {
      /// O agente executa autonomamente até atingir o objetivo ou limite de turnos.
      Auto,
      /// O agente apenas analisa e planeja, sem executar ferramentas de escrita ou destrutivas.
      Plan,
      /// O agente solicita aprovação do usuário para cada ferramenta executada (padrão de segurança).
      #[default]
      AcceptEdits,
  }
  ```
- **Existing `--dangerously-skip-permissions` Configuration & CLI Handling**:
  - In `src/config.rs:158-160`: `pub dangerously_skip_permissions: bool` in `Config` (defaults to `false`).
  - In `src/config.rs:287-291`: Overridden by environment variable `RUSI_DANGEROUSLY_SKIP_PERMISSIONS` ("1" or "true").
  - In `src/cli.rs:39-40`: CLI flag `--dangerously-skip-permissions` parsed via clap.
  - In `src/cli.rs:144-146`: `Cli::merge_with_config` sets `config.dangerously_skip_permissions = true` if flag is passed.
- **Existing `ToolDefinition` Structure** (`src/llm/types.rs:89-94`):
  ```rust
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct ToolDefinition {
      pub name: String,
      pub description: String,
      pub parameters: serde_json::Value,
  }
  ```
- **Gemini Request Integration** (`src/llm/gemini/request.rs:170-183`):
  Gemini connector maps `ToolDefinition` directly into `GeminiFunctionDeclaration`:
  ```rust
  GeminiToolGroup {
      function_declarations: tools
          .iter()
          .map(|t| GeminiFunctionDeclaration {
              name: t.name.clone(),
              description: t.description.clone(),
              parameters: t.parameters.clone(),
          })
          .collect(),
  }
  ```
  Existing test `src/llm/gemini/mod.rs:116-135` demonstrates JSON Schema format with `type: "object"`, `properties: { "path": {"type": "string"} }`, `required: ["path"]`.

---

## 2. Logic Chain

1. **Module Placement for R4**:
   `ROADMAP.md:154-158` specifies `src/cortex/permissions.rs` as the security and confirmation gatekeeper, while `ORIGINAL_REQUEST.md:27` states `src/cortex/permissions.rs (or src/tools/gate.rs)`. Creating `src/cortex/permissions.rs` and re-exporting in `src/cortex/mod.rs` (and optionally `src/tools/gate.rs` / `src/tools/mod.rs`) satisfies both the Phase 3 tool gate requirements and the Phase 4 Cortex engine roadmap.
2. **Reuse of `AgentMode`**:
   `AgentMode` must NOT be duplicated. `src/cortex/permissions.rs` should import `crate::config::AgentMode`.
3. **Operation Safety Classification**:
   The 5 native tools fall strictly into two categories:
   - **Safe / Read-Only**: `file_read`, `code_search`.
   - **Destructive / Mutating / Execution**: `file_write`, `file_edit`, `bash_run`.
   - **Unknown / Custom Tools**: Must default to Destructive (fail-safe principle of least privilege).
4. **Policy Enforcement Matrix**:
   - **`AgentMode::Plan`**:
     - Safe tools (`file_read`, `code_search`): Allowed immediately.
     - Destructive tools (`file_write`, `file_edit`, `bash_run`): Blocked immediately with `PermissionError::PlanModeBlocked`.
     - *Crucial rule*: `--dangerously-skip-permissions` MUST NOT bypass `Plan` mode blocking (acceptance criterion: "Security gate blocks file writes and command executions when in `AgentMode::Plan`").
   - **`AgentMode::AcceptEdits`**:
     - Safe tools: Allowed immediately without prompt.
     - Destructive tools: If `dangerously_skip_permissions == true`, allowed without prompt. Otherwise, requires interactive user confirmation `[y/N]`.
   - **`AgentMode::Auto`**:
     - Safe tools: Allowed immediately.
     - Destructive tools: If `dangerously_skip_permissions == true`, allowed without prompt. Otherwise, prompts for interactive confirmation `[y/N]` when permission is required.
5. **Interactive Confirmation & Prompting Logic**:
   - Prompt format: `"Autorizar execução de '{tool_name}'? [y/N]: "` (or with target argument details if provided).
   - Reading line from stdin: trimmed, case-insensitive comparison against `"y"` or `"yes"`.
   - Empty input, `"n"`, `"no"`, or unexpected characters default to rejection (`[y/N]`).
   - Closed stdin / EOF defaults to rejection (`[y/N]`).
   - To make unit testing robust and prevent `cargo test` hangs, prompt I/O must be abstracted behind a `ConfirmationPrompter` trait (e.g. `DefaultPrompter` for interactive terminal, `MockPrompter` for tests).
6. **Tool Declarations (R5)**:
   Expose all 5 native tools via constructor functions on `ToolDefinition` (`file_read`, `file_write`, `file_edit`, `bash_run`, `code_search`) and an aggregator `ToolDefinition::all_native_tools() -> Vec<ToolDefinition>`.

---

## 3. Features Discovered

| # | Category | Feature | Description | Inputs | Outputs | Error Behavior | Discovered Via |
|---|----------|---------|-------------|--------|---------|----------------|----------------|
| 1 | R4 Security Gate | `is_destructive` | Classifies tool operations as safe (read-only) or destructive (mutating/executing). | `tool_name: &str` | `bool` (`false` for `file_read`, `code_search`; `true` for others) | Unknown tools return `true` (safe default). | `ORIGINAL_REQUEST.md` R4 |
| 2 | R4 Security Gate | `PermissionsGate::evaluate` | Evaluates permission policy without I/O side-effects. | `tool_name: &str` | `GateDecision` (`Allow`, `BlockPlanMode`, `RequireConfirmation`) | Pure logic evaluation based on mode and skip flag. | Codebase architectural analysis |
| 3 | R4 Security Gate | `PermissionsGate::authorize` | Enforces permissions, prompting user if required. | `tool_name: &str, details: Option<&str>` | `Result<(), PermissionError>` | Returns `Err(PermissionError::PlanModeBlocked)` or `Err(PermissionError::UserDenied)`. | `ORIGINAL_REQUEST.md` R4 |
| 4 | R4 Security Gate | `ConfirmationPrompter` Trait | Abstraction for user confirmation prompt `[y/N]` allowing zero-hang test mocks. | `prompt: &str` | `io::Result<bool>` | Non-'y'/'yes' or EOF returns `false`. | Engineering best practice for CLI testability |
| 5 | R4 Security Gate | `PermissionsGate::from_config` | Instantiates gatekeeper using active `Config` settings. | `config: &Config` | `PermissionsGate` | None. | `src/config.rs` inspection |
| 6 | R5 Tool Declarations | `ToolDefinition::file_read` | JSON schema declaration for `file_read(path, start_line, end_line)`. | None | `ToolDefinition` | Validates OpenAPI 3.0 / JSON Schema compliance. | `ORIGINAL_REQUEST.md` R1 & R5 |
| 7 | R5 Tool Declarations | `ToolDefinition::file_write` | JSON schema declaration for `file_write(path, content, overwrite)`. | None | `ToolDefinition` | Declares required `path` and `content`, optional `overwrite`. | `ORIGINAL_REQUEST.md` R1 & R5 |
| 8 | R5 Tool Declarations | `ToolDefinition::file_edit` | JSON schema declaration for `file_edit(path, target_block, replacement_block)`. | None | `ToolDefinition` | Declares required `path`, `target_block`, and `replacement_block`. | `ORIGINAL_REQUEST.md` R1 & R5 |
| 9 | R5 Tool Declarations | `ToolDefinition::bash_run` | JSON schema declaration for `bash_run(command, cwd, timeout_ms)`. | None | `ToolDefinition` | Declares required `command`, optional `cwd` and `timeout_ms`. | `ORIGINAL_REQUEST.md` R2 & R5 |
| 10 | R5 Tool Declarations | `ToolDefinition::code_search` | JSON schema declaration for `code_search(pattern, path)`. | None | `ToolDefinition` | Declares required `pattern`, optional `path`. | `ORIGINAL_REQUEST.md` R3 & R5 |
| 11 | R5 Tool Declarations | `ToolDefinition::all_native_tools` | Aggregator returning all 5 native tool declarations for LLM dispatch. | None | `Vec<ToolDefinition>` | Returns exact vector of 5 tools. | `ORIGINAL_REQUEST.md` R5 |

---

## 4. Edge Cases

| # | Feature | Input | Observed / Required Behavior |
|---|---------|-------|------------------------------|
| 1 | `PermissionsGate` | `tool_name = "file_write"`, `mode = AgentMode::Plan` | Must return `Err(PermissionError::PlanModeBlocked)`. Execution is rejected immediately without prompt. |
| 2 | `PermissionsGate` | `tool_name = "bash_run"`, `mode = AgentMode::Plan`, `dangerously_skip_permissions = true` | Must return `Err(PermissionError::PlanModeBlocked)`. Skip flag cannot override Plan mode guarantee. |
| 3 | `PermissionsGate` | `tool_name = "file_read"`, `mode = AgentMode::Plan` | Must return `Ok(())`. Read operations are permitted in Plan mode. |
| 4 | `PermissionsGate` | `tool_name = "code_search"`, `mode = AgentMode::Plan` | Must return `Ok(())`. Search operations are permitted in Plan mode. |
| 5 | `PermissionsGate` | `tool_name = "file_edit"`, `mode = AgentMode::AcceptEdits`, user input = `"y\n"` | Prompts `[y/N]`, returns `Ok(())`. |
| 6 | `PermissionsGate` | `tool_name = "file_edit"`, `mode = AgentMode::AcceptEdits`, user input = `"yes\n"` | Case-insensitive match, returns `Ok(())`. |
| 7 | `PermissionsGate` | `tool_name = "file_edit"`, `mode = AgentMode::AcceptEdits`, user input = `"\n"` (Enter) | Default is 'N', returns `Err(PermissionError::UserDenied)`. |
| 8 | `PermissionsGate` | `tool_name = "file_edit"`, `mode = AgentMode::AcceptEdits`, user input = `"no\n"` or `"random"` | Returns `Err(PermissionError::UserDenied)`. |
| 9 | `PermissionsGate` | `tool_name = "bash_run"`, `mode = AgentMode::AcceptEdits`, stdin reaches EOF | Safe fail-closed: returns `Err(PermissionError::UserDenied)`. |
| 10 | `PermissionsGate` | `tool_name = "bash_run"`, `mode = AgentMode::AcceptEdits`, `dangerously_skip_permissions = true` | Bypasses prompt, returns `Ok(())`. |
| 11 | `PermissionsGate` | `tool_name = "unknown_tool"` | Treated as destructive: blocks in Plan mode, requires confirmation in AcceptEdits mode. |
| 12 | `ToolDefinition` | Parameter JSON Schemas in Gemini API | Root must be `type: "object"`, with `properties` map and `required` string array. Compatible with Gemini `functionDeclarations`. |

---

## 5. Concrete JSON Schemas for the 5 Native Tools

### 5.1 `file_read`
```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "Path to the file to read (relative to workspace or absolute)"
    },
    "start_line": {
      "type": "integer",
      "description": "Optional 1-indexed line number to start reading from"
    },
    "end_line": {
      "type": "integer",
      "description": "Optional 1-indexed line number to stop reading at (inclusive)"
    }
  },
  "required": ["path"]
}
```

### 5.2 `file_write`
```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "Path to the file to write. Parent directories will be created automatically if missing."
    },
    "content": {
      "type": "string",
      "description": "Content to write into the file"
    },
    "overwrite": {
      "type": "boolean",
      "description": "Whether to overwrite the file if it already exists (defaults to false to prevent accidental data loss)"
    }
  },
  "required": ["path", "content"]
}
```

### 5.3 `file_edit`
```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "Path to the file to edit"
    },
    "target_block": {
      "type": "string",
      "description": "Exact contiguous block of code/text to locate and replace. Must match uniquely in the file."
    },
    "replacement_block": {
      "type": "string",
      "description": "The new replacement text to insert in place of target_block"
    }
  },
  "required": ["path", "target_block", "replacement_block"]
}
```

### 5.4 `bash_run`
```json
{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "The shell command line to execute"
    },
    "cwd": {
      "type": "string",
      "description": "Optional working directory in which to execute the command (defaults to current workspace directory)"
    },
    "timeout_ms": {
      "type": "integer",
      "description": "Optional execution timeout in milliseconds before child process is killed"
    }
  },
  "required": ["command"]
}
```

### 5.5 `code_search`
```json
{
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "Regular expression pattern to search for across workspace files"
    },
    "path": {
      "type": "string",
      "description": "Optional root directory or file path to search within (defaults to current directory '.')"
    }
  },
  "required": ["pattern"]
}
```

---

## 6. Concrete Code Design & Signatures

### 6.1 `src/cortex/permissions.rs`
```rust
use std::io::{self, BufRead, Write};
use thiserror::Error;
use crate::config::AgentMode;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    #[error("Operação '{tool}' bloqueada: o modo Plan impede modificações e comandos")]
    PlanModeBlocked { tool: String },

    #[error("Execução de '{tool}' rejeitada pelo usuário")]
    UserDenied { tool: String },

    #[error("Falha ao solicitar confirmação: {0}")]
    PromptError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    BlockPlanMode,
    RequireConfirmation,
}

pub trait ConfirmationPrompter {
    fn confirm(&mut self, prompt: &str) -> io::Result<bool>;
}

pub struct TerminalPrompter;

impl ConfirmationPrompter for TerminalPrompter {
    fn confirm(&mut self, prompt: &str) -> io::Result<bool> {
        print!("{prompt}");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        Ok(trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes"))
    }
}

pub struct PermissionsGate {
    pub mode: AgentMode,
    pub dangerously_skip_permissions: bool,
}

impl PermissionsGate {
    pub fn new(mode: AgentMode, dangerously_skip_permissions: bool) -> Self {
        Self { mode, dangerously_skip_permissions }
    }

    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            mode: config.default_mode,
            dangerously_skip_permissions: config.dangerously_skip_permissions,
        }
    }

    pub fn is_destructive(tool_name: &str) -> bool {
        match tool_name {
            "file_read" | "code_search" => false,
            "file_write" | "file_edit" | "bash_run" => true,
            _ => true, // Conservative default
        }
    }

    pub fn evaluate(&self, tool_name: &str) -> GateDecision {
        if !Self::is_destructive(tool_name) {
            return GateDecision::Allow;
        }

        if self.mode == AgentMode::Plan {
            return GateDecision::BlockPlanMode;
        }

        if self.dangerously_skip_permissions {
            return GateDecision::Allow;
        }

        GateDecision::RequireConfirmation
    }

    pub fn authorize(&self, tool_name: &str, details: Option<&str>) -> Result<(), PermissionError> {
        let mut prompter = TerminalPrompter;
        self.authorize_with_prompter(tool_name, details, &mut prompter)
    }

    pub fn authorize_with_prompter<P: ConfirmationPrompter>(
        &self,
        tool_name: &str,
        details: Option<&str>,
        prompter: &mut P,
    ) -> Result<(), PermissionError> {
        match self.evaluate(tool_name) {
            GateDecision::Allow => Ok(()),
            GateDecision::BlockPlanMode => Err(PermissionError::PlanModeBlocked {
                tool: tool_name.to_string(),
            }),
            GateDecision::RequireConfirmation => {
                let prompt = match details {
                    Some(d) => format!("[?] Autorizar execução de '{tool_name}' ({d})? [y/N]: "),
                    None => format!("[?] Autorizar execução de '{tool_name}'? [y/N]: "),
                };

                let approved = prompter.confirm(&prompt).map_err(|e| {
                    PermissionError::PromptError(e.to_string())
                })?;

                if approved {
                    Ok(())
                } else {
                    Err(PermissionError::UserDenied {
                        tool: tool_name.to_string(),
                    })
                }
            }
        }
    }
}
```

### 6.2 Module Wiring
- `src/cortex/mod.rs`:
  ```rust
  pub mod permissions;
  pub use permissions::{ConfirmationPrompter, GateDecision, PermissionError, PermissionsGate};
  ```
- `src/tools/mod.rs` (or `src/tools/gate.rs`):
  ```rust
  pub use crate::cortex::permissions::{GateDecision, PermissionError, PermissionsGate};
  ```
- In `src/main.rs`:
  ```rust
  pub mod cortex;
  pub mod tools;
  ```

---

## 7. Caveats

- **No Caveats**: The requirements in `ORIGINAL_REQUEST.md` for R4 and R5, existing data structures in `src/config.rs` and `src/llm/types.rs`, and the Gemini provider serialization are completely aligned and unambiguous.

---

## 8. Conclusion

- **R4** is fully specified: `AgentMode` already exists in `src/config.rs` and should be reused. `PermissionsGate` provides deterministic two-phase authorization (`evaluate` and `authorize_with_prompter`), guarantees strict blocking under `Plan` mode, allows read-only bypass, and supports skip-permission overrides in non-Plan modes with fail-safe `[y/N]` confirmation prompts.
- **R5** is fully specified: Concrete JSON Schema parameter definitions have been authored for all 5 native tools (`file_read`, `file_write`, `file_edit`, `bash_run`, `code_search`), completely ready for implementation on `ToolDefinition`.

---

## 9. Verification Method

To verify the implementation once coded:
1. Run `cargo test`:
   - Must execute all unit tests including the new permissions and tool schema tests.
   - Verify `test_plan_mode_blocks_destructive_operations` passes.
   - Verify `test_dangerously_skip_permissions_bypasses_prompt` passes.
   - Verify `test_all_native_tools_schemas` passes.
2. Run `cargo clippy -- -D warnings`:
   - Must pass with 0 errors and 0 warnings.
3. Test schema compatibility:
   - Call `GeminiRequest::build(None, &[ChatMessage::user("hi")], &ToolDefinition::all_native_tools())` and verify serialization succeeds.
