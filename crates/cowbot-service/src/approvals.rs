use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, ApprovalStatus};
use cowbot_core::assignment::AssignmentId;
use cowbot_core::audit::{AuditEvent, AuditSource};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::permission::{matching_rule, PermissionRule, PermissionRuleMatch, RuleDecision};
use cowbot_core::risk::RiskLevel;
use cowbot_core::task::TaskId;
use cowbot_core::tool::{ToolAuthorization, ToolCall, ToolGate};
use cowbot_store::Store;
use cowbot_tools::{RiskAssessment, StaticRiskClassifier};
use tokio::time::{sleep, Instant};

use crate::permissions::{
    PermissionDecision, PermissionDecisionKind, PermissionEvaluator, PermissionRequest,
};
use crate::session_decisions::{session_key, SessionDecisions};

/// Policy outcome before a tool call executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    AutoApprove,
    RequireApproval,
    Deny,
}

/// Apply the workspace/assignment approval level to a risk level.
pub fn decision_for(level: ApprovalLevel, risk: RiskLevel) -> ApprovalDecision {
    match level {
        ApprovalLevel::Ask | ApprovalLevel::AcceptEdits => match risk {
            RiskLevel::ReadOnly => ApprovalDecision::AutoApprove,
            RiskLevel::Low | RiskLevel::Medium | RiskLevel::High => {
                ApprovalDecision::RequireApproval
            }
            RiskLevel::Critical => ApprovalDecision::Deny,
        },
        ApprovalLevel::SafeAuto => match risk {
            RiskLevel::ReadOnly | RiskLevel::Low => ApprovalDecision::AutoApprove,
            RiskLevel::Medium | RiskLevel::High => ApprovalDecision::RequireApproval,
            RiskLevel::Critical => ApprovalDecision::Deny,
        },
        ApprovalLevel::PlanOnly => match risk {
            RiskLevel::ReadOnly => ApprovalDecision::AutoApprove,
            _ => ApprovalDecision::Deny,
        },
        ApprovalLevel::Trusted => match risk {
            RiskLevel::Critical => ApprovalDecision::Deny,
            _ => ApprovalDecision::AutoApprove,
        },
    }
}

/// Apply the approval level with the tool call in hand.
///
/// `AcceptEdits` auto-approves only `write_file` calls whose path is inside the
/// workspace; all other calls follow the Manual policy.
pub fn decision_for_call(
    level: ApprovalLevel,
    risk: RiskLevel,
    tool_name: &str,
    rule_id: &str,
) -> ApprovalDecision {
    if tool_name == "delete_channel" {
        return if level == ApprovalLevel::PlanOnly {
            ApprovalDecision::Deny
        } else {
            ApprovalDecision::RequireApproval
        };
    }
    if level == ApprovalLevel::AcceptEdits
        && tool_name == "write_file"
        && rule_id == "write_file.workspace"
    {
        return ApprovalDecision::AutoApprove;
    }
    decision_for(level, risk)
}

/// Approval service used by the local runtime and the agent tool gate.
#[derive(Clone)]
pub struct ApprovalService {
    store: Store,
    bus: EventBus,
    workspace_id: WorkspaceId,
    timeout: Duration,
    evaluator: Option<Arc<dyn PermissionEvaluator>>,
    session_decisions: Option<Arc<SessionDecisions>>,
}

impl ApprovalService {
    pub fn new(store: Store, bus: EventBus, workspace_id: WorkspaceId) -> Self {
        Self {
            store,
            bus,
            workspace_id,
            timeout: Duration::from_secs(300),
            evaluator: None,
            session_decisions: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_evaluator(mut self, evaluator: Option<Arc<dyn PermissionEvaluator>>) -> Self {
        self.evaluator = evaluator;
        self
    }

    pub fn with_session_decisions(mut self, decisions: Option<Arc<SessionDecisions>>) -> Self {
        self.session_decisions = decisions;
        self
    }

    pub async fn authorize(
        &self,
        assignment_id: &AssignmentId,
        agent_id: &AgentId,
        call: &ToolCall,
    ) -> CowResult<ToolAuthorization> {
        let assignment = self
            .store
            .get_assignment(assignment_id)
            .await?
            .ok_or_else(|| CowError::Task(format!("assignment '{assignment_id}' was not found")))?;
        let assessment = StaticRiskClassifier.classify(call);
        let base = decision_for_call(
            assignment.approval_level,
            assessment.level,
            &call.name,
            &assessment.rule_id,
        );
        // A hard policy denial (Plan mode mutations, Critical outside Bypass)
        // cannot be softened by a workspace rule or a remembered decision.
        let rules = self.store.list_permission_rules(&self.workspace_id).await?;
        let rule_match = if base == ApprovalDecision::Deny {
            None
        } else {
            matching_rule(&rules, call)
        };
        let session_decision = self
            .session_decisions
            .as_ref()
            .and_then(|decisions| decisions.lookup(call));

        let model_decision = if session_decision.is_none()
            && rule_match.is_none()
            && base == ApprovalDecision::RequireApproval
            && assignment.approval_level == ApprovalLevel::SafeAuto
            && assessment.level != RiskLevel::Critical
            && call.name != "delete_channel"
        {
            if let Some(evaluator) = &self.evaluator {
                let profile = self.store.get_agent(agent_id).await?;
                let recent = self
                    .recent_tool_calls(&assignment.task_id)
                    .await
                    .unwrap_or_default();
                let request = PermissionRequest {
                    agent_id: agent_id.clone(),
                    agent_role: profile
                        .as_ref()
                        .map(|profile| profile.role.clone())
                        .unwrap_or_default(),
                    agent_goal: profile
                        .as_ref()
                        .map(|profile| profile.goal.clone())
                        .unwrap_or_default(),
                    tool_name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    risk: assessment.level,
                    risk_reason: assessment.reason.clone(),
                    approval_level: assignment.approval_level,
                    recent_tool_calls: recent,
                };
                match evaluator.evaluate(&request).await {
                    Ok(decision) => {
                        self.record_model_decision(&agent_id.0, call, &decision)
                            .await?;
                        Some(decision)
                    }
                    Err(error) => {
                        self.record_model_fallback(&agent_id.0, call, &error.to_string())
                            .await?;
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Precedence: hard policy denial, then Deny rules, then a remembered
        // human decision, then Ask/Allow rules, then the model evaluator.
        let rule_decision = rule_match.map(|rule| rule.decision);
        let (decision, source) = if base == ApprovalDecision::Deny {
            (ApprovalDecision::Deny, AuditSource::Policy)
        } else if rule_decision == Some(RuleDecision::Deny) {
            (ApprovalDecision::Deny, AuditSource::Rule)
        } else if let Some(status) = session_decision {
            match status {
                ApprovalStatus::Approved => (ApprovalDecision::AutoApprove, AuditSource::Session),
                ApprovalStatus::Rejected => (ApprovalDecision::Deny, AuditSource::Session),
                _ => (base, AuditSource::Policy),
            }
        } else if rule_decision == Some(RuleDecision::Ask) {
            (ApprovalDecision::RequireApproval, AuditSource::Rule)
        } else if rule_decision == Some(RuleDecision::Allow) {
            (ApprovalDecision::AutoApprove, AuditSource::Rule)
        } else {
            match model_decision.as_ref().map(|decision| decision.decision) {
                Some(PermissionDecisionKind::Allow) => {
                    (ApprovalDecision::AutoApprove, AuditSource::Model)
                }
                Some(PermissionDecisionKind::Deny) => (ApprovalDecision::Deny, AuditSource::Model),
                Some(PermissionDecisionKind::Ask) => {
                    (ApprovalDecision::RequireApproval, AuditSource::Model)
                }
                None => (base, AuditSource::Policy),
            }
        };

        match decision {
            ApprovalDecision::AutoApprove => {
                if session_decision == Some(ApprovalStatus::Approved) {
                    self.record_session_audit(&agent_id.0, "tool.session_approved", call)
                        .await?;
                } else {
                    self.record_audit(
                        &agent_id.0,
                        "tool.auto_approved",
                        call,
                        &assessment,
                        source,
                        rule_match,
                    )
                    .await?;
                }
                Ok(ToolAuthorization::Approved { approved_by: None })
            }
            ApprovalDecision::Deny => {
                if session_decision == Some(ApprovalStatus::Rejected) {
                    self.record_session_audit(&agent_id.0, "tool.session_denied", call)
                        .await?;
                    return Ok(ToolAuthorization::Denied {
                        reason: format!(
                            "the tool call was rejected for this session ({})",
                            session_key(call)
                        ),
                    });
                }
                self.record_audit(
                    &agent_id.0,
                    "tool.denied",
                    call,
                    &assessment,
                    source,
                    rule_match,
                )
                .await?;
                let reason = match rule_match {
                    Some(rule) if source == AuditSource::Rule => {
                        format!("workspace rule '{}' denies the tool call", rule.describe())
                    }
                    _ => model_decision
                        .as_ref()
                        .map(|decision| format!("model denied the tool call: {}", decision.reason))
                        .unwrap_or_else(|| {
                            format!(
                                "approval policy denied {} risk operation: {}",
                                assessment.level.as_str(),
                                assessment.reason
                            )
                        }),
                };
                Ok(ToolAuthorization::Denied { reason })
            }
            ApprovalDecision::RequireApproval => {
                let approval = Approval::new(assignment.task_id.clone(), call.id.clone());
                self.store.create_approval(&approval).await?;
                if let Some(decisions) = &self.session_decisions {
                    decisions.record_pending(&approval.id, call);
                }
                let rule = rule_match.map(PermissionRuleMatch::from);
                let reason = match (&rule, &model_decision) {
                    (Some(rule), _) if source == AuditSource::Rule => {
                        format!("workspace rule '{}' requires approval", rule.description)
                    }
                    (_, Some(decision)) => {
                        format!(
                            "{} (model: {} confidence {:.2})",
                            decision.reason, decision.model, decision.confidence
                        )
                    }
                    _ => assessment.reason.clone(),
                };
                self.bus
                    .publish(Event::new(EventPayload::ApprovalRequested {
                        approval: approval.clone(),
                        risk: assessment.level,
                        reason,
                        rule,
                    }));
                self.record_audit(
                    &agent_id.0,
                    "tool.approval_requested",
                    call,
                    &assessment,
                    source,
                    rule_match,
                )
                .await?;

                let decided = self.wait_for_decision(&approval.id).await?;
                let decided = if decided.status == ApprovalStatus::Pending {
                    self.expire_or_load_decision(&approval.id).await?
                } else {
                    decided
                };
                match decided.status {
                    ApprovalStatus::Approved => {
                        let approved_by = decided.decided_by.clone();
                        self.record_audit(
                            approved_by.as_deref().unwrap_or("unknown"),
                            "tool.approval_approved",
                            call,
                            &assessment,
                            AuditSource::Human,
                            rule_match,
                        )
                        .await?;
                        Ok(ToolAuthorization::Approved { approved_by })
                    }
                    ApprovalStatus::Rejected => {
                        let decided_by =
                            decided.decided_by.unwrap_or_else(|| "unknown".to_string());
                        self.record_audit(
                            &decided_by,
                            "tool.approval_rejected",
                            call,
                            &assessment,
                            AuditSource::Human,
                            rule_match,
                        )
                        .await?;
                        Ok(ToolAuthorization::Denied {
                            reason: format!("approval rejected by {decided_by}"),
                        })
                    }
                    ApprovalStatus::Expired | ApprovalStatus::Pending => {
                        self.record_audit(
                            "system",
                            "tool.approval_expired",
                            call,
                            &assessment,
                            AuditSource::System,
                            rule_match,
                        )
                        .await?;
                        Ok(ToolAuthorization::Denied {
                            reason: "approval expired without a decision".to_string(),
                        })
                    }
                }
            }
        }
    }

    pub async fn decide(
        &self,
        approval_id: &ApprovalId,
        status: ApprovalStatus,
        decided_by: &str,
    ) -> CowResult<Approval> {
        if status != ApprovalStatus::Approved && status != ApprovalStatus::Rejected {
            return Err(CowError::Config(
                "an approval can only be approved or rejected".to_string(),
            ));
        }
        self.store
            .decide_approval(approval_id, status, decided_by)
            .await?;
        let approval = self
            .store
            .get_approval(approval_id)
            .await?
            .ok_or_else(|| CowError::Task(format!("approval '{approval_id}' was not found")))?;
        self.bus.publish(Event::new(EventPayload::ApprovalDecided {
            approval: approval.clone(),
        }));
        let action = if status == ApprovalStatus::Approved {
            "tool.approval_approved"
        } else {
            "tool.approval_rejected"
        };
        self.store
            .append_audit_event(
                &AuditEvent::new(
                    self.workspace_id.clone(),
                    decided_by,
                    action,
                    format!(
                        "approval {} for tool call {}",
                        approval.id, approval.tool_call_id
                    ),
                )
                .with_source(AuditSource::Human),
            )
            .await?;
        Ok(approval)
    }

    /// Decide an approval and remember the decision for the process lifetime.
    pub async fn decide_for_session(
        &self,
        approval_id: &ApprovalId,
        status: ApprovalStatus,
        decided_by: &str,
    ) -> CowResult<Approval> {
        if let Some(decisions) = &self.session_decisions {
            if let Some(call) = decisions.take_pending(approval_id) {
                decisions.remember(&call, status);
            }
        }
        self.decide(approval_id, status, decided_by).await
    }

    pub async fn pending_for_task(&self, task_id: &TaskId) -> CowResult<Vec<Approval>> {
        self.store.list_pending_approvals(task_id).await
    }

    pub async fn pending_for_workspace(&self) -> CowResult<Vec<Approval>> {
        self.store
            .list_pending_approvals_for_workspace(&self.workspace_id)
            .await
    }

    /// Expire approvals left pending by a previous process.
    ///
    /// The local runtime should call this once during startup: a pending row
    /// whose waiting agent died would otherwise stay actionable forever. Only
    /// rows still pending are expired, so a decision that lands concurrently is
    /// preserved. Returns the number of approvals expired.
    pub async fn reconcile_on_startup(&self) -> CowResult<usize> {
        let pending = self
            .store
            .list_pending_approvals_for_workspace(&self.workspace_id)
            .await?;
        let mut expired = 0;
        for approval in pending {
            let resolved = self.expire_or_load_decision(&approval.id).await?;
            if resolved.status != ApprovalStatus::Expired {
                continue;
            }
            expired += 1;
            self.store
                .append_audit_event(
                    &AuditEvent::new(
                        self.workspace_id.clone(),
                        "system",
                        "tool.approval_expired",
                        format!(
                            "approval {} for tool call {} expired on startup",
                            approval.id, approval.tool_call_id
                        ),
                    )
                    .with_source(AuditSource::System),
                )
                .await?;
        }
        Ok(expired)
    }

    async fn wait_for_decision(&self, approval_id: &ApprovalId) -> CowResult<Approval> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let approval =
                self.store.get_approval(approval_id).await?.ok_or_else(|| {
                    CowError::Task(format!("approval '{approval_id}' was not found"))
                })?;
            if approval.status != ApprovalStatus::Pending || Instant::now() >= deadline {
                return Ok(approval);
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    /// Expire a timed-out approval unless another decision landed first.
    ///
    /// `decide_approval` only updates pending rows, so a human decision that
    /// races the timeout makes the expiry update fail. In that case the stored
    /// decision is returned and honoured instead of failing the tool call.
    async fn expire_or_load_decision(&self, approval_id: &ApprovalId) -> CowResult<Approval> {
        match self
            .store
            .decide_approval(approval_id, ApprovalStatus::Expired, "system")
            .await
        {
            Ok(()) => {
                let expired = self.store.get_approval(approval_id).await?.ok_or_else(|| {
                    CowError::Task(format!("approval '{approval_id}' was not found"))
                })?;
                self.bus.publish(Event::new(EventPayload::ApprovalDecided {
                    approval: expired.clone(),
                }));
                Ok(expired)
            }
            Err(error) => match self.store.get_approval(approval_id).await? {
                Some(stored) if stored.status != ApprovalStatus::Pending => Ok(stored),
                _ => Err(error),
            },
        }
    }

    async fn recent_tool_calls(&self, task_id: &TaskId) -> CowResult<Vec<String>> {
        let runs = self.store.list_runs(task_id).await?;
        let Some(run) = runs.last() else {
            return Ok(Vec::new());
        };
        let calls = self.store.list_tool_calls(&run.id).await?;
        Ok(calls
            .iter()
            .rev()
            .take(5)
            .map(|call| format!("{} {}", call.name, call.arguments))
            .collect())
    }

    async fn record_model_decision(
        &self,
        actor: &str,
        call: &ToolCall,
        decision: &PermissionDecision,
    ) -> CowResult<()> {
        self.store
            .append_audit_event(
                &AuditEvent::new(
                    self.workspace_id.clone(),
                    actor,
                    "tool.model_decision",
                    format!(
                        "{} ({}) model={} decision={:?} confidence={:.2} reason={}",
                        call.name,
                        call.id,
                        decision.model,
                        decision.decision,
                        decision.confidence,
                        decision.reason
                    ),
                )
                .with_source(AuditSource::Model),
            )
            .await
    }

    async fn record_model_fallback(
        &self,
        actor: &str,
        call: &ToolCall,
        error: &str,
    ) -> CowResult<()> {
        self.store
            .append_audit_event(
                &AuditEvent::new(
                    self.workspace_id.clone(),
                    actor,
                    "tool.model_fallback",
                    format!(
                        "{} ({}) fell back to human approval: {error}",
                        call.name, call.id
                    ),
                )
                .with_source(AuditSource::Model),
            )
            .await
    }

    async fn record_session_audit(
        &self,
        actor: &str,
        action: &str,
        call: &ToolCall,
    ) -> CowResult<()> {
        self.store
            .append_audit_event(
                &AuditEvent::new(
                    self.workspace_id.clone(),
                    actor,
                    action,
                    format!(
                        "{} ({}) session_key={}",
                        call.name,
                        call.id,
                        session_key(call)
                    ),
                )
                .with_source(AuditSource::Session),
            )
            .await
    }

    async fn record_audit(
        &self,
        actor: &str,
        action: &str,
        call: &ToolCall,
        assessment: &RiskAssessment,
        source: AuditSource,
        rule: Option<&PermissionRule>,
    ) -> CowResult<()> {
        let rule_detail = rule
            .map(|rule| {
                format!(
                    " workspace_rule={} ({}) decision={}",
                    rule.id,
                    rule.describe(),
                    rule.decision
                )
            })
            .unwrap_or_default();
        self.store
            .append_audit_event(
                &AuditEvent::new(
                    self.workspace_id.clone(),
                    actor,
                    action,
                    format!(
                        "{} ({}) risk={} risk_rule={}{rule_detail} arguments={}",
                        call.name,
                        call.id,
                        assessment.level.as_str(),
                        assessment.rule_id,
                        call.arguments
                    ),
                )
                .with_source(source),
            )
            .await
    }
}

/// A tool gate scoped to one assignment.
#[derive(Clone)]
pub struct ScopedApprovalGate {
    service: ApprovalService,
    assignment_id: AssignmentId,
}

impl ScopedApprovalGate {
    pub fn new(service: ApprovalService, assignment_id: AssignmentId) -> Self {
        Self {
            service,
            assignment_id,
        }
    }
}

#[async_trait]
impl ToolGate for ScopedApprovalGate {
    async fn authorize(&self, agent_id: &AgentId, call: &ToolCall) -> CowResult<ToolAuthorization> {
        self.service
            .authorize(&self.assignment_id, agent_id, call)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::task::Task;

    #[tokio::test]
    async fn expiry_keeps_a_decision_that_raced_the_timeout() {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = crate::bootstrap_local(&store).await.unwrap();
        let task = Task::new("Race task", "Output");
        store
            .create_task(&context.workspace.id, None, &task)
            .await
            .unwrap();

        let service = ApprovalService::new(
            store.clone(),
            EventBus::default_bus(),
            context.workspace.id.clone(),
        );
        let approval = Approval::new(task.id.clone(), "call-1");
        store.create_approval(&approval).await.unwrap();
        service
            .decide(&approval.id, ApprovalStatus::Approved, "user-1")
            .await
            .unwrap();

        let resolved = service.expire_or_load_decision(&approval.id).await.unwrap();
        assert_eq!(resolved.status, ApprovalStatus::Approved);
        assert_eq!(resolved.decided_by.as_deref(), Some("user-1"));
    }
}
