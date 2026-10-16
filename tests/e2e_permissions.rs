#![allow(unexpected_cfgs)]
#![cfg(milestone_4)]

mod common;

use rusi::config::{AgentMode, Config};
use rusi::cortex::permissions::{
    ConfirmationPrompter, GateDecision, PermissionError, PermissionsGate,
};
use rusi::llm::types::ToolDefinition;

struct MockPrompter {
    responses: Vec<bool>,
    recorded_prompts: Vec<String>,
}

impl MockPrompter {
    fn new(responses: Vec<bool>) -> Self {
        Self {
            responses,
            recorded_prompts: Vec::new(),
        }
    }
}

impl ConfirmationPrompter for MockPrompter {
    fn confirm(&mut self, prompt: &str) -> std::io::Result<bool> {
        self.recorded_prompts.push(prompt.to_string());
        if self.responses.is_empty() {
            Ok(false) // Default fail-safe
        } else {
            Ok(self.responses.remove(0))
        }
    }
}

// ============================================================================
// Tier 1: Feature Coverage (Representative Happy-Path Inputs)
// ============================================================================

#[test]
fn test_tier1_permissions_gate_allows_read_only_tools_in_plan_mode() {
    let gate = PermissionsGate::new(AgentMode::Plan, false);

    assert_eq!(gate.evaluate("file_read"), GateDecision::Allow);
    assert_eq!(gate.evaluate("code_search"), GateDecision::Allow);

    assert!(gate.authorize("file_read", None).is_ok());
    assert!(gate.authorize("code_search", None).is_ok());
}

#[test]
fn test_tier1_permissions_gate_dangerously_skip_permissions_allows_destructive() {
    let gate = PermissionsGate::new(AgentMode::Auto, true);

    assert_eq!(gate.evaluate("file_write"), GateDecision::Allow);
    assert_eq!(gate.evaluate("file_edit"), GateDecision::Allow);
    assert_eq!(gate.evaluate("bash_run"), GateDecision::Allow);

    assert!(gate.authorize("file_write", None).is_ok());
    assert!(gate.authorize("file_edit", None).is_ok());
    assert!(gate.authorize("bash_run", None).is_ok());
}

#[test]
fn test_tier1_permissions_gate_confirmation_prompter_approves_destructive() {
    let gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = MockPrompter::new(vec![true]);

    let res = gate.authorize_with_prompter("file_edit", Some("file.rs"), &mut prompter);
    assert!(res.is_ok(), "Approved prompt must allow operation");
    assert_eq!(prompter.recorded_prompts.len(), 1);
}

#[test]
fn test_tier1_tool_definition_all_native_tools() {
    let tools = ToolDefinition::all_native_tools();
    assert_eq!(tools.len(), 5, "Expected exactly 5 native tool definitions");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"file_read"));
    assert!(names.contains(&"file_write"));
    assert!(names.contains(&"file_edit"));
    assert!(names.contains(&"bash_run"));
    assert!(names.contains(&"code_search"));
}

#[test]
fn test_tier1_tool_definition_json_schemas() {
    let tools = ToolDefinition::all_native_tools();

    for tool in tools {
        let params = &tool.parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"].is_object());
        assert!(params["required"].is_array());

        match tool.name.as_str() {
            "file_read" => {
                assert!(params["properties"]["path"].is_object());
                assert!(params["properties"]["start_line"].is_object());
                assert!(params["properties"]["end_line"].is_object());
            }
            "file_write" => {
                assert!(params["properties"]["path"].is_object());
                assert!(params["properties"]["content"].is_object());
            }
            "file_edit" => {
                assert!(params["properties"]["path"].is_object());
                assert!(params["properties"]["target_block"].is_object());
                assert!(params["properties"]["replacement_block"].is_object());
            }
            "bash_run" => {
                assert!(params["properties"]["command"].is_object());
            }
            "code_search" => {
                assert!(params["properties"]["pattern"].is_object());
            }
            _ => {}
        }
    }
}

#[test]
fn test_tier1_permissions_gate_from_config() {
    let mut config = Config::default();
    config.default_mode = AgentMode::Auto;
    config.dangerously_skip_permissions = true;

    let gate = PermissionsGate::from_config(&config);
    assert_eq!(gate.mode, AgentMode::Auto);
    assert!(gate.dangerously_skip_permissions);
}

// ============================================================================
// Tier 2: Boundary & Corner Cases (Edge Conditions & Failure Modes)
// ============================================================================

#[test]
fn test_tier2_permissions_gate_plan_mode_strictly_blocks_destructive_operations() {
    let gate = PermissionsGate::new(AgentMode::Plan, false);

    for tool in ["file_write", "file_edit", "bash_run"] {
        assert_eq!(gate.evaluate(tool), GateDecision::BlockPlanMode);
        let res = gate.authorize(tool, None);
        assert!(
            res.is_err(),
            "Plan mode must strictly block destructive tool: {tool}"
        );
        match res.unwrap_err() {
            PermissionError::PlanModeBlocked { tool: t } => {
                assert_eq!(t, tool);
            }
            other => panic!("Expected PlanModeBlocked for {tool}, got: {other:?}"),
        }
    }
}

#[test]
fn test_tier2_permissions_gate_plan_mode_blocks_even_if_dangerously_skip_is_true() {
    // CRITICAL: --dangerously-skip-permissions MUST NOT bypass Plan mode safety guarantee!
    let gate = PermissionsGate::new(AgentMode::Plan, true);

    assert_eq!(gate.evaluate("file_write"), GateDecision::BlockPlanMode);
    let res = gate.authorize("file_write", None);
    assert!(res.is_err(), "Plan mode cannot be bypassed by skip flag");
    match res.unwrap_err() {
        PermissionError::PlanModeBlocked { .. } => {}
        other => panic!("Expected PlanModeBlocked, got: {other:?}"),
    }
}

#[test]
fn test_tier2_permissions_gate_user_rejection_denies_execution() {
    let gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = MockPrompter::new(vec![false]);

    let res = gate.authorize_with_prompter("bash_run", Some("rm -rf /"), &mut prompter);
    assert!(res.is_err(), "User rejection must deny execution");
    match res.unwrap_err() {
        PermissionError::UserDenied { tool } => {
            assert_eq!(tool, "bash_run");
        }
        other => panic!("Expected UserDenied, got: {other:?}"),
    }
}

#[test]
fn test_tier2_permissions_gate_unknown_tool_defaults_to_destructive() {
    let gate = PermissionsGate::new(AgentMode::Plan, false);

    assert!(
        PermissionsGate::is_destructive("custom_dangerous_tool"),
        "Unknown tool must default to destructive"
    );
    assert_eq!(
        gate.evaluate("custom_dangerous_tool"),
        GateDecision::BlockPlanMode
    );
}

#[test]
fn test_tier2_permissions_gate_prompter_receives_details_string() {
    let gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = MockPrompter::new(vec![true]);

    let details = "file=/path/to/script.sh";
    let _ = gate.authorize_with_prompter("file_write", Some(details), &mut prompter);

    assert_eq!(prompter.recorded_prompts.len(), 1);
    let prompt = &prompter.recorded_prompts[0];
    assert!(prompt.contains("file_write"));
    assert!(prompt.contains(details));
}

#[test]
fn test_tier2_permissions_gate_prompt_io_error_handling() {
    struct FailingPrompter;
    impl ConfirmationPrompter for FailingPrompter {
        fn confirm(&mut self, _prompt: &str) -> std::io::Result<bool> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin closed",
            ))
        }
    }

    let gate = PermissionsGate::new(AgentMode::AcceptEdits, false);
    let mut prompter = FailingPrompter;

    let res = gate.authorize_with_prompter("file_edit", None, &mut prompter);
    assert!(res.is_err());
    match res.unwrap_err() {
        PermissionError::PromptError(msg) => {
            assert!(msg.contains("stdin closed"));
        }
        other => panic!("Expected PromptError, got: {other:?}"),
    }
}
