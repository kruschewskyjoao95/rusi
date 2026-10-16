#![allow(unexpected_cfgs)]
#![cfg(milestone_6)]

mod common;

use common::TestEnv;
use rusi::config::AgentMode;
use rusi::cortex::permissions::{ConfirmationPrompter, PermissionsGate};
use rusi::tools::fs::{file_edit, file_read, file_write, FsError};
use rusi::tools::process::bash_run;
use rusi::tools::search::code_search;

struct MockPrompter {
    approved: bool,
}

impl ConfirmationPrompter for MockPrompter {
    fn confirm(&mut self, _prompt: &str) -> std::io::Result<bool> {
        Ok(self.approved)
    }
}

// ============================================================================
// Tier 3: Cross-Feature Combinations (Pairwise Subsystem Interactions)
// ============================================================================

#[test]
fn test_tier3_fs_lifecycle_write_read_edit_read() {
    let env = TestEnv::new();
    let file = env.file_path("pipeline.txt");

    // 1. Write file
    let initial_content = "header: config\ntimeout = 30\nretries = 3\nfooter: end\n";
    file_write(&file, initial_content, false).expect("file_write failed");

    // 2. Read with slice
    let slice = file_read(&file, Some(2), Some(3)).expect("file_read slice failed");
    assert_eq!(slice, "2: timeout = 30\n3: retries = 3\n");

    // 3. Edit unique block
    file_edit(&file, "timeout = 30", "timeout = 60").expect("file_edit failed");

    // 4. Read entire file and verify full state
    let full = file_read(&file, None, None).expect("file_read full failed");
    let expected = "1: header: config\n2: timeout = 60\n3: retries = 3\n4: footer: end\n";
    assert_eq!(full, expected);
}

#[tokio::test]
async fn test_tier3_fs_and_process_script_execution() {
    let env = TestEnv::new();
    let script_file = env.file_path("generate.sh");
    let script = "#!/bin/bash\necho 'GENERATED_DATA_789' > generated.txt\n";

    file_write(&script_file, script, false).expect("Failed to write shell script");

    // Execute script via bash_run in the test environment directory
    let cmd = format!("bash {}", script_file.to_string_lossy());
    let output = bash_run(&cmd, Some(env.path()), 5000)
        .await
        .expect("bash_run failed");

    assert_eq!(output.exit_code, Some(0));

    // Verify generated file via file_read
    let gen_path = env.file_path("generated.txt");
    let content = file_read(&gen_path, None, None).expect("Failed to read generated file");
    assert!(content.contains("GENERATED_DATA_789"));
}

#[test]
fn test_tier3_fs_and_search_with_gitignore() {
    let env = TestEnv::new();

    // 1. Write .gitignore
    env.create_file(".gitignore", "*.tmp\nbuild/\n");

    // 2. Write matching files in various locations
    env.create_file("src/main.rs", "const MARKER: &str = \"RUSI_FIND_ME\";\n");
    env.create_file("scratch.tmp", "const MARKER: &str = \"RUSI_FIND_ME\";\n");
    env.create_file("build/artifact.rs", "const MARKER: &str = \"RUSI_FIND_ME\";\n");

    // 3. Code search must find ONLY src/main.rs
    let matches = code_search("RUSI_FIND_ME", env.path()).expect("code_search failed");
    assert_eq!(matches.len(), 1);
    assert!(matches[0].file.ends_with("src/main.rs"));
}

#[test]
fn test_tier3_search_match_leading_to_targeted_file_edit() {
    let env = TestEnv::new();
    env.create_file(
        "src/api.rs",
        "pub fn legacy_call() {\n    deprecate_me();\n}\n",
    );

    // 1. Search to find target file and content
    let matches = code_search("deprecate_me\\(\\);", env.path()).expect("Search failed");
    assert_eq!(matches.len(), 1);

    let match_item = &matches[0];
    assert!(match_item.file.ends_with("src/api.rs"));
    assert_eq!(match_item.line_number, 2);

    // 2. Perform targeted edit on the discovered file
    file_edit(
        &match_item.file,
        "    deprecate_me();",
        "    modern_replacement();",
    )
    .expect("Edit failed");

    // 3. Search again to confirm zero occurrences
    let second_search = code_search("deprecate_me\\(\\);", env.path()).expect("Second search failed");
    assert!(second_search.is_empty());

    // 4. Confirm new pattern exists
    let modern_search =
        code_search("modern_replacement\\(\\);", env.path()).expect("Modern search failed");
    assert_eq!(modern_search.len(), 1);
}

#[test]
fn test_tier3_permissions_gate_guarding_fs_operations() {
    let env = TestEnv::new();
    let file = env.file_path("protected.txt");

    // Phase A: Plan mode blocks destructive write
    let plan_gate = PermissionsGate::new(AgentMode::Plan, false);
    assert!(plan_gate.authorize("file_write", None).is_err());
    assert!(!env.exists("protected.txt"));

    // Phase B: AcceptEdits mode with approved prompter authorizes and writes
    let accept_gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = MockPrompter { approved: true };

    let auth = accept_gate.authorize_with_prompter("file_write", None, &mut prompter);
    assert!(auth.is_ok());

    file_write(&file, "authorized data", false).expect("file_write failed");
    assert_eq!(env.read_file("protected.txt"), "authorized data");
}

#[tokio::test]
async fn test_tier3_permissions_gate_guarding_bash_operations() {
    // Phase A: Plan mode blocks execution
    let plan_gate = PermissionsGate::new(AgentMode::Plan, false);
    assert!(plan_gate.authorize("bash_run", None).is_err());

    // Phase B: Auto mode with skip permissions allows execution
    let auto_gate = PermissionsGate::new(AgentMode::Auto, true);
    assert!(auto_gate.authorize("bash_run", None).is_ok());

    let output = bash_run("echo 'gate_authorized'", None, 5000)
        .await
        .expect("bash_run failed");
    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.contains("gate_authorized"));
}

// ============================================================================
// Tier 4: Real-World Scenarios (Autonomous Agent Workflows)
// ============================================================================

#[tokio::test]
async fn test_tier4_scenario_automated_codebase_refactoring() {
    let env = TestEnv::new();

    // Create a mini project with 3 modules containing an outdated constant
    env.create_file("src/net.rs", "pub const MAX_RETRIES: u32 = 3;\n");
    env.create_file("src/db.rs", "pub const MAX_RETRIES: u32 = 3;\n");
    env.create_file("src/worker.rs", "pub const MAX_RETRIES: u32 = 3;\n");
    env.create_file("Cargo.toml", "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n");

    // Step 1: Agent searches for all occurrences of the constant
    let matches = code_search("pub const MAX_RETRIES: u32 = 3;", env.path())
        .expect("Failed searching for deprecated constant");
    assert_eq!(matches.len(), 3);

    // Step 2: Agent iterates through matches and edits each file
    for m in &matches {
        file_edit(
            &m.file,
            "pub const MAX_RETRIES: u32 = 3;",
            "pub const MAX_RETRIES: u32 = 5;",
        )
        .expect("Failed to apply refactoring edit");
    }

    // Step 3: Agent runs bash command to verify zero old constants remain
    let check_cmd = format!("grep -rn 'MAX_RETRIES: u32 = 3' {}", env.path().display());
    let check_output = bash_run(&check_cmd, None, 5000).await.expect("bash check failed");
    assert_ne!(
        check_output.exit_code,
        Some(0),
        "grep should find zero matches and return non-zero"
    );

    // Step 4: Agent reads back files and verifies updated state
    for m in &matches {
        let content = file_read(&m.file, None, None).expect("Read back failed");
        assert!(content.contains("pub const MAX_RETRIES: u32 = 5;"));
    }
}

#[test]
fn test_tier4_scenario_read_only_planning_transition_to_execution() {
    let env = TestEnv::new();
    let spec_file = env.create_file("spec.md", "# Project Spec\nFeature A is required.\n");

    // Phase 1: Planning (Read-Only)
    let plan_gate = PermissionsGate::new(AgentMode::Plan, false);

    // Read and search operations are completely allowed without asking user
    let read_res = plan_gate.authorize("file_read", Some("spec.md"));
    assert!(read_res.is_ok());
    let spec_content = file_read(&spec_file, None, None).unwrap();
    assert!(spec_content.contains("Feature A"));

    let search_res = plan_gate.authorize("code_search", Some("Feature A"));
    assert!(search_res.is_ok());

    // Agent attempts to write implementation file in Plan mode: blocked
    let impl_file = env.file_path("feature_a.rs");
    let write_auth = plan_gate.authorize("file_write", Some("feature_a.rs"));
    assert!(write_auth.is_err(), "Must be blocked in Plan mode");

    // Phase 2: User approves plan -> transition to AcceptEdits
    let exec_gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = MockPrompter { approved: true };

    let exec_auth = exec_gate.authorize_with_prompter("file_write", Some("feature_a.rs"), &mut prompter);
    assert!(exec_auth.is_ok(), "Authorized after user prompt approval");

    file_write(&impl_file, "pub fn feature_a() -> bool { true }\n", false).unwrap();
    assert!(env.exists("feature_a.rs"));
}

#[test]
fn test_tier4_scenario_edit_ambiguity_recovery_workflow() {
    let env = TestEnv::new();
    let code = "fn alpha() {\n    let count = 0;\n    count += 1;\n}\n\nfn beta() {\n    let count = 0;\n    count += 2;\n}\n";
    let file = env.create_file("ambiguous_workflow.rs", code);

    // 1. Agent attempts an ambiguous replacement
    let ambiguous_attempt = file_edit(&file, "    let count = 0;", "    let count = 100;");
    assert!(ambiguous_attempt.is_err());
    match ambiguous_attempt.unwrap_err() {
        FsError::AmbiguousTargetBlock { count, .. } => {
            assert_eq!(count, 2);
        }
        other => panic!("Expected AmbiguousTargetBlock, got: {other:?}"),
    }

    // 2. Recovery: Agent reads lines around the target area to gain wider context
    let context = file_read(&file, Some(1), Some(4)).expect("Recovery read failed");
    assert!(context.contains("fn alpha()"));
    assert!(context.contains("let count = 0;"));

    // 3. Agent formulates unique multi-line replacement using function context
    let unique_target = "fn alpha() {\n    let count = 0;";
    let unique_replacement = "fn alpha() {\n    let count = 100;";

    let recovery_edit = file_edit(&file, unique_target, unique_replacement);
    assert!(recovery_edit.is_ok(), "Recovery edit with widened context must succeed");

    // 4. Verify only alpha() was modified, beta() remains unchanged
    let updated = file_read(&file, None, None).unwrap();
    assert!(updated.contains("fn alpha() {\n2:     let count = 100;"));
    assert!(updated.contains("fn beta() {\n7:     let count = 0;"));
}
