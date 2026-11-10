use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::{
    Approval, ApprovalLevel, ApprovalStatus, BypassCritical, WorkspacePolicy,
};
use cowbot_core::assignment::{
    Assignment, AssignmentMode, AssignmentOrigin, AssignmentRequester, ReplyMode,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::{Workspace, WorkspaceId};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::risk::RiskLevel;
use cowbot_core::task::Task;
use cowbot_core::tool::ToolAuthorization;
use cowbot_core::tool::ToolCall;
use cowbot_service::{
    bootstrap_local, decision_for, decision_for_call, ApprovalDecision, ApprovalService,
    PermissionDecision, PermissionDecisionKind, PermissionEvaluator, PermissionRequest,
    SessionDecisions,
};
use cowbot_store::Store;
use serde_json::json;

async fn setup(
    level: ApprovalLevel,
    timeout: Duration,
) -> (Store, ApprovalService, Assignment, AgentId, WorkspaceId) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let context = bootstrap_local(&store).await.unwrap();
    let agent = AgentId::new("agent-1");
    let task = cowbot_core::task::Task::new("Approval task", "An approved result")
        .assign_to(agent.clone());
    store
        .create_task(&context.workspace.id, None, &task)
        .await
        .unwrap();

    let assignment = Assignment::new_root(
        task.id.clone(),
        AssignmentOrigin::User,
        AssignmentRequester::User(context.user.id.0.clone()),
        agent.clone(),
        AssignmentMode::Work,
        ReplyMode::Resume,
        level,
    );
    store.create_assignment(&assignment).await.unwrap();

    let bus = EventBus::default_bus();
    let service = ApprovalService::new(store.clone(), bus, context.workspace.id.clone())
        .with_timeout(timeout);
    (
        store,
        service,
        assignment,
        agent,
        context.workspace.id.clone(),
    )
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call-1".to_string(),
        name: name.to_string(),
        arguments,
    }
}

#[test]
fn policy_decisions_are_distinct_per_mode() {
    assert_eq!(
        decision_for(
            ApprovalLevel::Ask,
            RiskLevel::ReadOnly,
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for(ApprovalLevel::Ask, RiskLevel::Low, BypassCritical::Allow),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::AcceptEdits,
            RiskLevel::Low,
            BypassCritical::Allow
        ),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for_call(
            ApprovalLevel::AcceptEdits,
            RiskLevel::Medium,
            "write_file",
            "write_file.workspace",
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for_call(
            ApprovalLevel::AcceptEdits,
            RiskLevel::High,
            "write_file",
            "write_file.outside_workspace",
            BypassCritical::Allow
        ),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for_call(
            ApprovalLevel::AcceptEdits,
            RiskLevel::Low,
            "execute_code",
            "execute_code.low_risk",
            BypassCritical::Allow
        ),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::AcceptEdits,
            RiskLevel::Critical,
            BypassCritical::Allow
        ),
        ApprovalDecision::Deny
    );
    assert_eq!(
        decision_for_call(
            ApprovalLevel::Trusted,
            RiskLevel::High,
            "delete_channel",
            "workspace.delete_channel",
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for_call(
            ApprovalLevel::PlanOnly,
            RiskLevel::High,
            "delete_channel",
            "workspace.delete_channel",
            BypassCritical::Allow
        ),
        ApprovalDecision::Deny
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::SafeAuto,
            RiskLevel::Low,
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::SafeAuto,
            RiskLevel::Medium,
            BypassCritical::Allow
        ),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::PlanOnly,
            RiskLevel::Low,
            BypassCritical::Allow
        ),
        ApprovalDecision::Deny
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::Trusted,
            RiskLevel::High,
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::Trusted,
            RiskLevel::Critical,
            BypassCritical::Allow
        ),
        ApprovalDecision::AutoApprove
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::Trusted,
            RiskLevel::Critical,
            BypassCritical::Ask
        ),
        ApprovalDecision::RequireApproval
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::Trusted,
            RiskLevel::Critical,
            BypassCritical::Block
        ),
        ApprovalDecision::Deny
    );
    assert_eq!(
        decision_for(
            ApprovalLevel::SafeAuto,
            RiskLevel::Critical,
            BypassCritical::Allow
        ),
        ApprovalDecision::Deny
    );
}

#[tokio::test]
async fn model_allow_auto_approves_a_mutating_call() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Allow,
        calls: calls.clone(),
    })));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call(
                "write_file",
                json!({"path": "src/main.rs", "content": "fn main() {}"}),
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        authorization,
        ToolAuthorization::Approved { approved_by: None }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.model_decision"));
}

#[tokio::test]
async fn model_deny_denies_the_call() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Deny,
        calls: Arc::new(AtomicUsize::new(0)),
    })));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call(
                "write_file",
                json!({"path": "src/main.rs", "content": "fn main() {}"}),
            ),
        )
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    let ToolAuthorization::Denied { reason } = authorization else {
        unreachable!()
    };
    assert!(reason.contains("model denied"));
}

#[tokio::test]
async fn critical_calls_skip_the_evaluator() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Allow,
        calls: calls.clone(),
    })));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({"command": "rm -rf /"})),
        )
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn evaluator_failure_falls_back_to_human_approval() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_millis(20)).await;
    let service = service.with_evaluator(Some(Arc::new(FailingEvaluator)));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call(
                "write_file",
                json!({"path": "src/main.rs", "content": "fn main() {}"}),
            ),
        )
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.model_fallback"));
}

#[tokio::test]
async fn delete_channel_never_uses_the_model() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_millis(20)).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Allow,
        calls: calls.clone(),
    })));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("delete_channel", json!({"channel": "ops"})),
        )
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn session_approval_skips_the_evaluator() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let decisions = SessionDecisions::new();
    let write = call(
        "write_file",
        json!({"path": "src/main.rs", "content": "fn main() {}"}),
    );
    decisions.remember(&write, ApprovalStatus::Approved);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service
        .with_session_decisions(Some(decisions))
        .with_evaluator(Some(Arc::new(ScriptedEvaluator {
            decision: PermissionDecisionKind::Deny,
            calls: calls.clone(),
        })));

    let authorization = service
        .authorize(&assignment.id, &agent, &write)
        .await
        .unwrap();

    assert_eq!(
        authorization,
        ToolAuthorization::Approved { approved_by: None }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.session_approved"));
}

#[tokio::test]
async fn session_rejection_denies_the_call() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let decisions = SessionDecisions::new();
    let write = call(
        "write_file",
        json!({"path": "src/main.rs", "content": "fn main() {}"}),
    );
    decisions.remember(&write, ApprovalStatus::Rejected);
    let service = service.with_session_decisions(Some(decisions));

    let authorization = service
        .authorize(&assignment.id, &agent, &write)
        .await
        .unwrap();

    let ToolAuthorization::Denied { reason } = authorization else {
        panic!("expected a denial");
    };
    assert!(reason.contains("rejected for this session"));
}

#[tokio::test]
async fn session_approval_cannot_override_a_hard_denial() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let decisions = SessionDecisions::new();
    let critical = call("execute_code", json!({"command": "rm -rf /"}));
    decisions.remember(&critical, ApprovalStatus::Approved);
    let service = service.with_session_decisions(Some(decisions));

    let authorization = service
        .authorize(&assignment.id, &agent, &critical)
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
}

struct ScriptedEvaluator {
    decision: PermissionDecisionKind,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl PermissionEvaluator for ScriptedEvaluator {
    async fn evaluate(&self, _request: &PermissionRequest) -> CowResult<PermissionDecision> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(PermissionDecision {
            decision: self.decision,
            reason: "scripted".to_string(),
            confidence: 0.95,
            model: "scripted-model".to_string(),
        })
    }
}

struct FailingEvaluator;

#[async_trait]
impl PermissionEvaluator for FailingEvaluator {
    async fn evaluate(&self, _request: &PermissionRequest) -> CowResult<PermissionDecision> {
        Err(CowError::Task("evaluator unavailable".to_string()))
    }
}

#[tokio::test]
async fn auto_approval_records_audit() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("read_file", json!({ "path": "src/main.rs" })),
        )
        .await
        .unwrap();
    assert_eq!(
        authorization,
        ToolAuthorization::Approved { approved_by: None }
    );

    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.auto_approved"));
}

#[tokio::test]
async fn plan_only_denies_writes_and_records_audit() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::PlanOnly, Duration::from_secs(1)).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call(
                "write_file",
                json!({ "path": "src/main.rs", "content": "x" }),
            ),
        )
        .await
        .unwrap();
    match authorization {
        ToolAuthorization::Denied { reason } => assert!(reason.contains("denied")),
        other => panic!("expected denied, got {other:?}"),
    }

    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    assert!(events.iter().any(|event| event.action == "tool.denied"));
}

#[tokio::test]
async fn trusted_allows_critical_by_default() {
    let (_store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::Trusted, Duration::from_secs(1)).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "rm -rf /" })),
        )
        .await
        .unwrap();
    assert_eq!(
        authorization,
        ToolAuthorization::Approved { approved_by: None }
    );
}

#[tokio::test]
async fn trusted_blocks_critical_when_the_policy_blocks() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Trusted, Duration::from_secs(1)).await;
    store
        .save_workspace_policy(&WorkspacePolicy {
            workspace_id: workspace_id.clone(),
            bypass_critical: BypassCritical::Block,
        })
        .await
        .unwrap();

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "rm -rf /" })),
        )
        .await
        .unwrap();
    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "tool.denied")
        .expect("denial audit");
    assert_eq!(event.source.as_deref(), Some("policy"));
}

#[tokio::test]
async fn trusted_asks_for_critical_when_the_policy_asks() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Trusted, Duration::from_millis(20)).await;
    store
        .save_workspace_policy(&WorkspacePolicy {
            workspace_id: workspace_id.clone(),
            bypass_critical: BypassCritical::Ask,
        })
        .await
        .unwrap();

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "rm -rf /" })),
        )
        .await
        .unwrap();
    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    let events = store.list_audit_events(&workspace_id, 30).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.approval_requested"));
    assert!(events
        .iter()
        .any(|event| event.action == "tool.approval_expired"));
}

#[tokio::test]
async fn expired_approval_is_persisted_and_denies_the_tool() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_millis(300)).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "echo hello" })),
        )
        .await
        .unwrap();
    match authorization {
        ToolAuthorization::Denied { reason } => assert!(reason.contains("expired")),
        other => panic!("expected denied, got {other:?}"),
    }

    let pending = store
        .list_pending_approvals(&assignment.task_id)
        .await
        .unwrap();
    assert!(
        pending.is_empty(),
        "expired approvals must not stay pending"
    );

    let events = store.list_audit_events(&workspace_id, 50).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.approval_expired"));
}

#[tokio::test]
async fn already_decided_approval_cannot_be_decided_again() {
    let (store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_secs(2)).await;

    let task_id = assignment.task_id.clone();
    let decider = service.clone();
    let decide_store = store.clone();
    let authorize = tokio::spawn(async move {
        service
            .authorize(
                &assignment.id,
                &agent,
                &call("execute_code", json!({ "command": "echo hello" })),
            )
            .await
    });
    let (first, second) = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pending = decide_store.list_pending_approvals(&task_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        decider
            .decide(&pending[0].id, ApprovalStatus::Approved, "user-1")
            .await
            .unwrap();
        let again = decider
            .decide(&pending[0].id, ApprovalStatus::Rejected, "user-2")
            .await;
        (pending[0].id.clone(), again.is_err())
    })
    .await
    .unwrap();

    let authorization = authorize.await.unwrap().unwrap();
    assert_eq!(
        authorization,
        ToolAuthorization::Approved {
            approved_by: Some("user-1".to_string())
        }
    );

    let stored = store.get_approval(&first).await.unwrap().unwrap();
    assert_eq!(stored.status, ApprovalStatus::Approved);
    assert_eq!(stored.decided_by.as_deref(), Some("user-1"));
    assert!(second, "a second decision on the same approval must fail");
}

#[tokio::test]
async fn pending_approval_is_approved_by_a_human() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_secs(2)).await;
    let task_id = assignment.task_id.clone();

    let decider = service.clone();
    let decide_store = store.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pending = decide_store.list_pending_approvals(&task_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        decider
            .decide(&pending[0].id, ApprovalStatus::Approved, "user-1")
            .await
            .unwrap();
    });

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "echo hello" })),
        )
        .await
        .unwrap();
    assert_eq!(
        authorization,
        ToolAuthorization::Approved {
            approved_by: Some("user-1".to_string())
        }
    );

    let pending = store
        .list_pending_approvals(&assignment.task_id)
        .await
        .unwrap();
    assert!(pending.is_empty());

    let events = store.list_audit_events(&workspace_id, 50).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.action == "tool.approval_requested"));
    assert!(events
        .iter()
        .any(|event| event.action == "tool.approval_approved"));
}

#[tokio::test]
async fn pending_approval_rejected_denies_the_tool() {
    let (store, service, assignment, agent, _workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_secs(2)).await;
    let task_id = assignment.task_id.clone();

    let decider = service.clone();
    let decide_store = store.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pending = decide_store.list_pending_approvals(&task_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        decider
            .decide(&pending[0].id, ApprovalStatus::Rejected, "user-1")
            .await
            .unwrap();
    });

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("execute_code", json!({ "command": "echo hello" })),
        )
        .await
        .unwrap();
    match authorization {
        ToolAuthorization::Denied { reason } => assert!(reason.contains("rejected")),
        other => panic!("expected denied, got {other:?}"),
    }
}

#[tokio::test]
async fn startup_reconciliation_expires_orphaned_pending_approvals() {
    let (store, service, assignment, _agent, workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_secs(2)).await;

    let orphan = Approval::new(assignment.task_id.clone(), "call-orphan");
    store.create_approval(&orphan).await.unwrap();

    let decided = Approval::new(assignment.task_id.clone(), "call-decided");
    store.create_approval(&decided).await.unwrap();
    service
        .decide(&decided.id, ApprovalStatus::Approved, "user-1")
        .await
        .unwrap();

    let other_workspace = Workspace::new("Other Workspace");
    store.create_workspace(&other_workspace).await.unwrap();
    let other_task = Task::new("Other task", "Other output");
    store
        .create_task(&other_workspace.id, None, &other_task)
        .await
        .unwrap();
    let other_approval = Approval::new(other_task.id.clone(), "call-other");
    store.create_approval(&other_approval).await.unwrap();

    let expired = service.reconcile_on_startup().await.unwrap();
    assert_eq!(expired, 1);

    let stored = store.get_approval(&orphan.id).await.unwrap().unwrap();
    assert_eq!(stored.status, ApprovalStatus::Expired);
    assert_eq!(stored.decided_by.as_deref(), Some("system"));
    assert!(store
        .list_pending_approvals(&assignment.task_id)
        .await
        .unwrap()
        .is_empty());

    let stored = store.get_approval(&decided.id).await.unwrap().unwrap();
    assert_eq!(stored.status, ApprovalStatus::Approved);

    let untouched = store
        .get_approval(&other_approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(untouched.status, ApprovalStatus::Pending);

    let events = store.list_audit_events(&workspace_id, 50).await.unwrap();
    assert!(events.iter().any(|event| {
        event.action == "tool.approval_expired" && event.target.contains("call-orphan")
    }));
}

async fn add_rule(
    store: &Store,
    workspace_id: &WorkspaceId,
    tool: &str,
    command: Option<&str>,
    decision: RuleDecision,
) -> PermissionRule {
    let rule = PermissionRule::new(
        workspace_id.clone(),
        tool,
        command.map(str::to_string),
        decision,
    );
    store.upsert_permission_rule(&rule).await.unwrap();
    rule
}

#[tokio::test]
async fn allow_rule_auto_approves_and_skips_the_evaluator() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let rule = add_rule(
        &store,
        &workspace_id,
        "write_file",
        None,
        RuleDecision::Allow,
    )
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Deny,
        calls: calls.clone(),
    })));

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call(
                "write_file",
                json!({"path": "src/main.rs", "content": "fn main() {}"}),
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        authorization,
        ToolAuthorization::Approved { approved_by: None }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "tool.auto_approved")
        .expect("auto approval audit");
    assert_eq!(event.source.as_deref(), Some("rule"));
    assert!(event.target.contains(&rule.id));
}

#[tokio::test]
async fn deny_rule_denies_read_only_calls() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_secs(1)).await;
    add_rule(&store, &workspace_id, "read_file", None, RuleDecision::Deny).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("read_file", json!({"path": "src/main.rs"})),
        )
        .await
        .unwrap();

    let ToolAuthorization::Denied { reason } = authorization else {
        panic!("expected a denial");
    };
    assert!(reason.contains("workspace rule"));
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "tool.denied")
        .expect("denial audit");
    assert_eq!(event.source.as_deref(), Some("rule"));
}

#[tokio::test]
async fn ask_rule_creates_an_approval_with_the_rule_source() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::Ask, Duration::from_millis(20)).await;
    let rule = add_rule(&store, &workspace_id, "write_file", None, RuleDecision::Ask).await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("write_file", json!({"path": "src/main.rs", "content": "x"})),
        )
        .await
        .unwrap();
    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));

    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "tool.approval_requested")
        .expect("approval request audit");
    assert_eq!(event.source.as_deref(), Some("rule"));
    assert!(event.target.contains(&rule.id));
}

#[tokio::test]
async fn plan_only_denial_is_not_softened_by_an_allow_rule() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::PlanOnly, Duration::from_secs(1)).await;
    add_rule(
        &store,
        &workspace_id,
        "write_file",
        None,
        RuleDecision::Allow,
    )
    .await;

    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("write_file", json!({"path": "src/main.rs", "content": "x"})),
        )
        .await
        .unwrap();

    let ToolAuthorization::Denied { reason } = authorization else {
        panic!("expected a denial");
    };
    assert!(reason.contains("approval policy denied"));
    let events = store.list_audit_events(&workspace_id, 20).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.action == "tool.denied")
        .expect("denial audit");
    assert_eq!(event.source.as_deref(), Some("policy"));
}

#[tokio::test]
async fn session_rejection_beats_an_allow_rule() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    add_rule(
        &store,
        &workspace_id,
        "write_file",
        None,
        RuleDecision::Allow,
    )
    .await;
    let decisions = SessionDecisions::new();
    let write = call("write_file", json!({"path": "src/main.rs", "content": "x"}));
    decisions.remember(&write, ApprovalStatus::Rejected);
    let service = service.with_session_decisions(Some(decisions));

    let authorization = service
        .authorize(&assignment.id, &agent, &write)
        .await
        .unwrap();
    let ToolAuthorization::Denied { reason } = authorization else {
        panic!("expected a denial");
    };
    assert!(reason.contains("rejected for this session"));
}

#[tokio::test]
async fn disabled_imported_rules_do_not_decide() {
    let (store, service, assignment, agent, workspace_id) =
        setup(ApprovalLevel::SafeAuto, Duration::from_secs(1)).await;
    let rule = PermissionRule::new(
        workspace_id.clone(),
        "write_file",
        None,
        RuleDecision::Allow,
    )
    .with_enabled(false)
    .with_source(cowbot_core::permission::RuleSource::Imported);
    store.upsert_permission_rule(&rule).await.unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let service = service.with_evaluator(Some(Arc::new(ScriptedEvaluator {
        decision: PermissionDecisionKind::Deny,
        calls: calls.clone(),
    })));
    let authorization = service
        .authorize(
            &assignment.id,
            &agent,
            &call("write_file", json!({"path": "src/main.rs", "content": "x"})),
        )
        .await
        .unwrap();

    assert!(matches!(authorization, ToolAuthorization::Denied { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
