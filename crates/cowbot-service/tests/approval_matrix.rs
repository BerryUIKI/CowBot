//! Acceptance matrix for the five approval modes (ADR-0003, issue #93).
//!
//! Each row asserts the outcome for one tool class under one mode. Keep this
//! table in sync with the mode table in `docs/ADR/0003-risk-classification-and-approval.md`
//! and with `docs/USER_GUIDE.md`.

use cowbot_core::approval::{ApprovalLevel, BypassCritical};
use cowbot_core::risk::RiskLevel;
use cowbot_core::tool::ToolCall;
use cowbot_service::{decision_for_call, ApprovalDecision};
use cowbot_tools::StaticRiskClassifier;
use serde_json::json;

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call-matrix".to_string(),
        name: name.to_string(),
        arguments,
    }
}

fn outcome(level: ApprovalLevel, call: &ToolCall, bypass: BypassCritical) -> ApprovalDecision {
    let assessment = StaticRiskClassifier.classify(call);
    decision_for_call(
        level,
        assessment.level,
        &call.name,
        &assessment.rule_id,
        bypass,
    )
}

/// One representative call per tool class named in the issue.
fn tool_classes() -> Vec<(&'static str, ToolCall, RiskLevel)> {
    vec![
        (
            "read-only",
            call("read_file", json!({ "path": "src/main.rs" })),
            RiskLevel::ReadOnly,
        ),
        (
            "edit inside workspace",
            call(
                "write_file",
                json!({ "path": "src/main.rs", "content": "x" }),
            ),
            RiskLevel::Medium,
        ),
        (
            "edit outside workspace",
            call(
                "write_file",
                json!({ "path": "/etc/hosts", "content": "x" }),
            ),
            RiskLevel::High,
        ),
        (
            "shell low",
            call("execute_code", json!({ "command": "mkdir scratch" })),
            RiskLevel::Low,
        ),
        (
            "shell medium",
            call("execute_code", json!({ "command": "npm install" })),
            RiskLevel::Medium,
        ),
        (
            "shell high",
            call("execute_code", json!({ "command": "git push --force" })),
            RiskLevel::High,
        ),
        (
            "shell critical",
            call("execute_code", json!({ "command": "rm -rf /" })),
            RiskLevel::Critical,
        ),
        (
            "workspace create_channel",
            call("create_channel", json!({ "name": "ops" })),
            RiskLevel::Medium,
        ),
        (
            "workspace create_task",
            call(
                "create_task",
                json!({
                    "description": "x",
                    "expected_output": "y",
                    "assignee": "main"
                }),
            ),
            RiskLevel::Medium,
        ),
        (
            "workspace instantiate_team",
            call("instantiate_team", json!({ "template": "software-team" })),
            RiskLevel::High,
        ),
        (
            "workspace delete_channel",
            call("delete_channel", json!({ "channel": "ops" })),
            RiskLevel::High,
        ),
        (
            "unknown tool",
            call("mystery_tool", json!({ "input": "x" })),
            RiskLevel::High,
        ),
    ]
}

fn expected(
    level: ApprovalLevel,
    call: &ToolCall,
    risk: RiskLevel,
    bypass: BypassCritical,
) -> ApprovalDecision {
    // Destructive channel deletion always needs a human, in every mode except
    // Plan (which denies it). See issue #84.
    if call.name == "delete_channel" {
        return match level {
            ApprovalLevel::PlanOnly => ApprovalDecision::Deny,
            _ => ApprovalDecision::RequireApproval,
        };
    }
    match level {
        ApprovalLevel::Ask | ApprovalLevel::AcceptEdits => {
            let accept_edit = level == ApprovalLevel::AcceptEdits
                && call.name == "write_file"
                && StaticRiskClassifier.classify(call).rule_id == "write_file.workspace";
            if risk == RiskLevel::Critical {
                ApprovalDecision::Deny
            } else if risk == RiskLevel::ReadOnly || accept_edit {
                ApprovalDecision::AutoApprove
            } else {
                ApprovalDecision::RequireApproval
            }
        }
        ApprovalLevel::SafeAuto => match risk {
            RiskLevel::ReadOnly | RiskLevel::Low => ApprovalDecision::AutoApprove,
            RiskLevel::Medium | RiskLevel::High => ApprovalDecision::RequireApproval,
            RiskLevel::Critical => ApprovalDecision::Deny,
        },
        ApprovalLevel::PlanOnly => {
            if risk == RiskLevel::ReadOnly {
                ApprovalDecision::AutoApprove
            } else {
                ApprovalDecision::Deny
            }
        }
        ApprovalLevel::Trusted => match risk {
            RiskLevel::Critical => match bypass {
                BypassCritical::Allow => ApprovalDecision::AutoApprove,
                BypassCritical::Ask => ApprovalDecision::RequireApproval,
                BypassCritical::Block => ApprovalDecision::Deny,
            },
            _ => ApprovalDecision::AutoApprove,
        },
    }
}

#[test]
fn acceptance_matrix_covers_every_mode_and_tool_class() {
    let modes = [
        ApprovalLevel::SafeAuto,
        ApprovalLevel::Ask,
        ApprovalLevel::AcceptEdits,
        ApprovalLevel::PlanOnly,
    ];
    for (label, call, expected_risk) in tool_classes() {
        let assessment = StaticRiskClassifier.classify(&call);
        assert_eq!(
            assessment.level, expected_risk,
            "{label} must classify as {expected_risk:?}"
        );
        for mode in modes {
            assert_eq!(
                outcome(mode, &call, BypassCritical::Allow),
                expected(mode, &call, expected_risk, BypassCritical::Allow),
                "mode {mode:?}, tool class {label}"
            );
        }
    }
}

#[test]
fn bypass_allows_every_non_critical_class_and_follows_the_tri_state_for_critical() {
    for (label, call, risk) in tool_classes() {
        assert_eq!(
            outcome(ApprovalLevel::Trusted, &call, BypassCritical::Allow),
            expected(ApprovalLevel::Trusted, &call, risk, BypassCritical::Allow),
            "bypass allow, tool class {label}"
        );
    }

    let critical = call("execute_code", json!({ "command": "rm -rf /" }));
    assert_eq!(
        outcome(ApprovalLevel::Trusted, &critical, BypassCritical::Allow),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        outcome(ApprovalLevel::Trusted, &critical, BypassCritical::Ask),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        outcome(ApprovalLevel::Trusted, &critical, BypassCritical::Block),
        ApprovalDecision::Deny
    );

    for mode in [
        ApprovalLevel::SafeAuto,
        ApprovalLevel::Ask,
        ApprovalLevel::AcceptEdits,
        ApprovalLevel::PlanOnly,
    ] {
        for bypass in [
            BypassCritical::Allow,
            BypassCritical::Ask,
            BypassCritical::Block,
        ] {
            assert_eq!(
                outcome(mode, &critical, bypass),
                ApprovalDecision::Deny,
                "critical calls stay blocked in {mode:?} even when bypass is {bypass:?}"
            );
        }
    }
}
