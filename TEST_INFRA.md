# Test Infrastructure Specification: Rusi Phase 3 E2E Test Suite

## 1. Overview & Objectives

This document establishes the architecture, test methodology, layout, and execution protocols for the end-to-end (E2E) opaque-box test suite for Rusi Phase 3 (Native Toolset and Security Gatekeeper).

The primary objectives are:
1. **Opaque-Box Verification**: Exercise tools exclusively through public API boundaries, CLI arguments, and standard library behaviors without coupling to internal private states.
2. **Comprehensive 4-Tier Coverage**: Systematically cover feature happy-paths, boundary/corner edge cases, cross-feature integrations, and realistic agent workflows.
3. **Progressive Testability**: Keep test suites partitioned by milestone and functional area so that individual test targets compile and pass cleanly as milestones are completed.
4. **Hermetic Test Isolation**: Guarantee that every test executes in a clean, isolated environment (temporary directories, mock prompters, isolated subprocess groups) without polluting the source tree or depending on test execution order.
5. **Zero Clippy / Build Warnings**: Ensure all test code conforms to strict Rust 2024 edition guidelines and zero compiler/clippy warnings.

---

## 2. 4-Tier Test Methodology

```
┌────────────────────────────────────────────────────────┐
│               Tier 4: Real-World Scenarios             │
│  (Multi-step refactoring, mode switches, recovery)     │
├────────────────────────────────────────────────────────┤
│          Tier 3: Cross-Feature Combinations            │
│ (Write->Read, Write->Edit->Read, FS->Search, Gate->FS) │
├────────────────────────────────────────────────────────┤
│          Tier 2: Boundary & Corner Cases (>=5)         │
│(Empty files, out-of-range lines, duplicate edits, timeouts)│
├────────────────────────────────────────────────────────┤
│            Tier 1: Feature Coverage (>=5)              │
│ (Happy path: read slice, write new, edit unique, bash) │
└────────────────────────────────────────────────────────┘
```

### Tier 1 — Feature Coverage (Isolated Happy Path)
Requires at least 5 distinct test cases per feature covering standard operational parameters:
- **`file_read`**:
  1. Full file read with line numbering format (`"{i}: {line}\n"`).
  2. Sub-slice read with explicit `start_line` and `end_line`.
  3. Prefix read from `start_line = 1` through arbitrary `end_line`.
  4. Suffix read with `start_line` to end of file (`end_line = None`).
  5. Multi-line text preserving indentation and whitespace.
- **`file_write`**:
  1. Write new file in an existing directory.
  2. Write new file with non-existent nested parent directories (`create_dir_all`).
  3. Overwrite existing file when `overwrite = true`.
  4. Write multi-line code content with special characters.
  5. Write binary/UTF-8 data fidelity check.
- **`file_edit`**:
  1. Replace unique single-line block in middle of file.
  2. Replace multi-line contiguous code block.
  3. Replace first line in file.
  4. Replace last line in file.
  5. Replace block containing special characters (quotes, regex metachars, newlines).
- **`bash_run`**:
  1. Execute basic stdout command (`echo "hello"`).
  2. Execute command with combined stdout and stderr.
  3. Execute command with custom working directory (`cwd`).
  4. Execute multi-step shell command sequence (`echo A && echo B`).
  5. Capture non-zero exit codes correctly (`exit 42`).
- **`code_search`**:
  1. Search exact literal word across multiple files.
  2. Search regular expression pattern (`fn [a-z_]+\(`).
  3. Case-sensitive pattern matching verification.
  4. Search in specific subdirectory vs workspace root.
  5. Result formatting with `format_search_results`.
- **`PermissionsGate` & Schemas**:
  1. `PermissionsGate` allows safe read operations in `AgentMode::Plan`.
  2. `PermissionsGate` allows safe search operations in `AgentMode::Plan`.
  3. `PermissionsGate` allows destructive operations when `dangerously_skip_permissions = true`.
  4. `PermissionsGate` allows destructive operation when user confirms `y`/`yes`.
  5. `ToolDefinition::all_native_tools()` returns all 5 tool schemas with valid OpenAPI parameter types.

### Tier 2 — Boundary & Corner Cases (Edge Conditions & Failure Modes)
Requires at least 5 distinct test cases per feature covering boundary limits and error paths:
- **`file_read`**:
  1. Read empty file (returns `Ok("")`).
  2. `start_line` greater than total lines in file (returns `FsError::LineOutOfRange`).
  3. `start_line > end_line` (returns `FsError::InvalidLineRange`).
  4. `start_line = 0` (normalized to line 1).
  5. Non-existent file path (returns `FsError::FileNotFound`).
- **`file_write`**:
  1. Write to existing file with `overwrite = false` (returns `FsError::FileAlreadyExists`).
  2. Write to read-only directory / permission failure.
  3. Write empty content string.
  4. Write with trailing slash path or dot paths (`./file.txt`).
  5. Overwrite idempotent check (content matches exact new data, old erased).
- **`file_edit`**:
  1. Empty `target_block` parameter (returns `FsError::EmptyTargetBlock`).
  2. Target block does not exist in file (returns `FsError::TargetBlockNotFound`).
  3. Target block exists multiple times (>1) in file (returns `FsError::AmbiguousTargetBlock`).
  4. Target block is entire file content.
  5. Replacement block is empty (deletion of target block).
- **`bash_run`**:
  1. Timeout triggers process kill (`sleep 10` with 200ms timeout returns within 1000ms).
  2. Process group termination ensures grandchildren processes are killed without orphans.
  3. Non-existent command / binary (returns spawn or shell failure).
  4. Non-existent working directory `cwd` (returns `ProcessError::InvalidCwd`).
  5. Process producing high volume stdout/stderr without deadlocking pipes.
- **`code_search`**:
  1. Pattern matching files excluded by `.gitignore` (ignored files omitted from results).
  2. Pattern matching files in hidden directories (e.g. `.git/` or `.secret/` ignored).
  3. Search on non-existent path (returns `SearchError::PathNotFound`).
  4. Search with invalid regex syntax (returns `SearchError::InvalidRegex`).
  5. Empty search results returns empty vector (no match found).
- **`PermissionsGate`**:
  1. Destructive operations blocked in `AgentMode::Plan` even if `dangerously_skip_permissions = true`.
  2. User rejects confirmation prompt (`n`, `no`, arbitrary text returns `PermissionError::UserDenied`).
  3. Empty response to prompt (default Enter / EOF returns `PermissionError::UserDenied`).
  4. Unknown / custom tool names default to destructive behavior.
  5. Safe tools (`file_read`, `code_search`) bypass prompt in `AcceptEdits` mode.

### Tier 3 — Cross-Feature Combinations (Pairwise Subsystem Interactions)
Exercises multi-subsystem data pipelines and state transitions:
1. **FS Lifecycle**: `file_write` creates new nested file -> `file_read` verifies line numbering -> `file_edit` updates section -> `file_read` verifies replaced content.
2. **FS + Process**: `file_write` creates shell script or Cargo file -> `bash_run` executes it -> `file_read` checks output file generated by the process.
3. **FS + Search**: `file_write` creates files in normal directory and gitignored directory -> `code_search` scans for pattern -> verifies only non-ignored file is discovered.
4. **Search + Edit**: `code_search` locates pattern across project -> `file_edit` applies fix to discovered file at exact location -> `code_search` confirms pattern no longer present.
5. **Gate + FS**: `PermissionsGate` in `Plan` mode blocks `file_write` and `file_edit` -> switch gate to `Auto` with skip permissions -> `file_write` and `file_edit` execute successfully.
6. **Gate + Bash**: `PermissionsGate` in `Plan` mode blocks `bash_run` -> switch gate to `AcceptEdits` with mock prompter approving -> `bash_run` executes command.

### Tier 4 — Real-World Agent Scenarios (End-to-End Workflows)
Emulates realistic agent orchestration workflows:
1. **Automated Codebase Refactoring**:
   Agent searches for deprecated function pattern across multiple source files (`code_search`), applies targeted block replacement to each (`file_edit`), runs verification test command via `bash_run`, and reads back results.
2. **Read-Only Planning Phase to Execution Phase**:
   Agent begins in `AgentMode::Plan`, inspects codebase via `code_search` and `file_read` without permission prompts, attempts an unauthorized write which is rejected, requests user escalation, transitions to `AcceptEdits`, and performs necessary edits upon user confirmation.
3. **Edit Ambiguity Recovery Loop**:
   Agent attempts `file_edit` with a short pattern that matches multiple times, receives `AmbiguousTargetBlock`, invokes `file_read` with slice range around the expected line to retrieve larger context, re-submits `file_edit` with unique multi-line block, and succeeds.

---

## 3. Directory Layout & Test Module Organization

The integration test suite is located in `/home/joao/Projects/rusi/tests/`:

```
tests/
├── common/
│   └── mod.rs             # Test fixtures: TempDir guard, MockPrompter, sample project generator
├── e2e_fs.rs              # M1: Filesystem Tools (Tiers 1 & 2)
├── e2e_process.rs         # M2: Process Execution & Timeout Kill (Tiers 1 & 2)
├── e2e_search.rs          # M3: Gitignore & Regex Search (Tiers 1 & 2)
├── e2e_permissions.rs     # M4: Security Gate & LLM Schemas (Tiers 1 & 2)
└── e2e_integration.rs     # M5/M6: Cross-Feature (Tier 3) & Workflows (Tier 4)
```

Each test binary is self-contained. Test modules can be executed individually during milestone development or collectively upon project completion.

---

## 4. Test Isolation, Fixtures & Determinism

### 4.1 Hermetic Workspace Fixture (`TestEnv`)
Every test that performs filesystem operations, git repository interaction, or bash execution must use a self-contained temporary directory managed by a RAII guard:
- Created in the system temp directory or `target/test_tmp/`.
- Cleans up automatically on drop unless test debugging is enabled.
- Avoids modifying the project repository (`/home/joao/Projects/rusi`).

### 4.2 Non-Interactive Confirmation Mock (`MockPrompter`)
Interactive terminal prompting (`[y/N]`) must never block automated CI or `cargo test`.
The `MockPrompter` fixture implements `ConfirmationPrompter`:
- Configured with scripted sequence of responses: `vec![true]`, `vec![false]`, or custom input strings.
- Records all prompts presented to the user for assertion verification.

### 4.3 Subprocess Isolation & Safety
- Commands run via `bash_run` execute within the test's `TestEnv` working directory.
- Timeout tests use brief durations (e.g. 200ms–500ms) with `sleep 10` commands, verifying that termination occurs in < 1.0s and process groups leave zero dangling background jobs.

---

## 5. Execution & Verification Commands

### Run Full Test Suite
```bash
cargo test
```

### Run Feature-Specific Test Targets
```bash
# Filesystem Tools (M1)
cargo test --test e2e_fs

# Process Execution Tools (M2)
cargo test --test e2e_process

# Workspace Code Search Tools (M3)
cargo test --test e2e_search

# Security Gate & Tool Definitions (M4 & M5)
cargo test --test e2e_permissions

# Cross-Feature & Real-World Workflows (M6)
cargo test --test e2e_integration
```

### Run with Strict Linting
```bash
cargo clippy --tests -- -D warnings
```
