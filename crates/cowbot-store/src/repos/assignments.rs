use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{
    Assignment, AssignmentBudgets, AssignmentEvent, AssignmentEventKind, AssignmentId,
    AssignmentMode, AssignmentOrigin, AssignmentOutcome, AssignmentRequester, AssignmentStatus,
    ReplyMode,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::task::TaskId;

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct AssignmentRow {
    id: String,
    root_id: String,
    parent_id: Option<String>,
    depth: i64,
    task_id: String,
    origin: String,
    requester: String,
    assignee: String,
    mode: String,
    reply: String,
    status: String,
    executions: i64,
    restarts: i64,
    wakes: i64,
    request_budget: i64,
    execution_budget: i64,
    wake_budget: i64,
    approval_level: String,
    outcome: Option<String>,
    stall_strikes: i64,
    repeated_errors: i64,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
}

impl AssignmentRow {
    fn into_domain(self) -> CowResult<Assignment> {
        Ok(Assignment {
            id: AssignmentId(self.id),
            root_id: AssignmentId(self.root_id),
            parent_id: self.parent_id.map(AssignmentId),
            depth: self.depth.max(0) as u32,
            task_id: TaskId(self.task_id),
            origin: origin_from_str(&self.origin)?,
            requester: parse_json::<AssignmentRequester>(&self.requester, "assignments.requester")?,
            assignee: AgentId(self.assignee),
            mode: mode_from_str(&self.mode)?,
            reply: reply_from_str(&self.reply)?,
            status: status_from_str(&self.status)?,
            executions: self.executions.max(0) as u32,
            restarts: self.restarts.max(0) as u32,
            wakes: self.wakes.max(0) as u32,
            budgets: AssignmentBudgets {
                requests: self.request_budget.max(0) as u32,
                executions: self.execution_budget.max(0) as u32,
                wakes: self.wake_budget.max(0) as u32,
            },
            approval_level: approval_level_from_str(&self.approval_level)?,
            outcome: self
                .outcome
                .as_deref()
                .map(|value| parse_json::<AssignmentOutcome>(value, "assignments.outcome"))
                .transpose()?,
            stall_strikes: self.stall_strikes.max(0) as u32,
            repeated_errors: self.repeated_errors.max(0) as u32,
            last_error: self.last_error,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct AssignmentEventRow {
    id: String,
    assignment_id: String,
    kind: String,
    detail: Option<String>,
    created_at: String,
}

impl AssignmentEventRow {
    fn into_domain(self) -> CowResult<AssignmentEvent> {
        Ok(AssignmentEvent {
            id: self.id,
            assignment_id: AssignmentId(self.assignment_id),
            kind: event_kind_from_str(&self.kind)?,
            detail: self.detail,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

fn origin_to_str(origin: AssignmentOrigin) -> &'static str {
    match origin {
        AssignmentOrigin::User => "user",
        AssignmentOrigin::Agent => "agent",
        AssignmentOrigin::System => "system",
        AssignmentOrigin::Routine => "routine",
        AssignmentOrigin::External => "external",
    }
}

fn origin_from_str(value: &str) -> CowResult<AssignmentOrigin> {
    match value {
        "user" => Ok(AssignmentOrigin::User),
        "agent" => Ok(AssignmentOrigin::Agent),
        "system" => Ok(AssignmentOrigin::System),
        "routine" => Ok(AssignmentOrigin::Routine),
        "external" => Ok(AssignmentOrigin::External),
        other => Err(CowError::Storage(format!(
            "unknown assignment origin '{other}'"
        ))),
    }
}

fn mode_to_str(mode: AssignmentMode) -> &'static str {
    match mode {
        AssignmentMode::Work => "work",
        AssignmentMode::Consult => "consult",
        AssignmentMode::Notify => "notify",
    }
}

fn mode_from_str(value: &str) -> CowResult<AssignmentMode> {
    match value {
        "work" => Ok(AssignmentMode::Work),
        "consult" => Ok(AssignmentMode::Consult),
        "notify" => Ok(AssignmentMode::Notify),
        other => Err(CowError::Storage(format!(
            "unknown assignment mode '{other}'"
        ))),
    }
}

fn reply_to_str(reply: ReplyMode) -> &'static str {
    match reply {
        ReplyMode::Resume => "resume",
        ReplyMode::Poll => "poll",
        ReplyMode::None => "none",
    }
}

fn reply_from_str(value: &str) -> CowResult<ReplyMode> {
    match value {
        "resume" => Ok(ReplyMode::Resume),
        "poll" => Ok(ReplyMode::Poll),
        "none" => Ok(ReplyMode::None),
        other => Err(CowError::Storage(format!("unknown reply mode '{other}'"))),
    }
}

fn status_to_str(status: AssignmentStatus) -> &'static str {
    match status {
        AssignmentStatus::Queued => "queued",
        AssignmentStatus::Running => "running",
        AssignmentStatus::Waiting => "waiting",
        AssignmentStatus::Resume => "resume",
        AssignmentStatus::NeedsInput => "needs_input",
        AssignmentStatus::Completed => "completed",
        AssignmentStatus::Failed => "failed",
        AssignmentStatus::Cancelled => "cancelled",
    }
}

fn status_from_str(value: &str) -> CowResult<AssignmentStatus> {
    match value {
        "queued" => Ok(AssignmentStatus::Queued),
        "running" => Ok(AssignmentStatus::Running),
        "waiting" => Ok(AssignmentStatus::Waiting),
        "resume" => Ok(AssignmentStatus::Resume),
        "needs_input" => Ok(AssignmentStatus::NeedsInput),
        "completed" => Ok(AssignmentStatus::Completed),
        "failed" => Ok(AssignmentStatus::Failed),
        "cancelled" => Ok(AssignmentStatus::Cancelled),
        other => Err(CowError::Storage(format!(
            "unknown assignment status '{other}'"
        ))),
    }
}

fn approval_level_to_str(level: ApprovalLevel) -> &'static str {
    match level {
        ApprovalLevel::Ask => "ask",
        ApprovalLevel::AcceptEdits => "accept_edits",
        ApprovalLevel::SafeAuto => "safe_auto",
        ApprovalLevel::PlanOnly => "plan_only",
        ApprovalLevel::Trusted => "trusted",
    }
}

fn approval_level_from_str(value: &str) -> CowResult<ApprovalLevel> {
    match value {
        "ask" => Ok(ApprovalLevel::Ask),
        "accept_edits" => Ok(ApprovalLevel::AcceptEdits),
        "safe_auto" => Ok(ApprovalLevel::SafeAuto),
        "plan_only" => Ok(ApprovalLevel::PlanOnly),
        "trusted" => Ok(ApprovalLevel::Trusted),
        other => Err(CowError::Storage(format!(
            "unknown approval level '{other}'"
        ))),
    }
}

fn event_kind_to_str(kind: AssignmentEventKind) -> &'static str {
    match kind {
        AssignmentEventKind::Enqueued => "enqueued",
        AssignmentEventKind::Started => "started",
        AssignmentEventKind::Waiting => "waiting",
        AssignmentEventKind::Resumed => "resumed",
        AssignmentEventKind::NeedsInput => "needs_input",
        AssignmentEventKind::Completed => "completed",
        AssignmentEventKind::Failed => "failed",
        AssignmentEventKind::Cancelled => "cancelled",
        AssignmentEventKind::Restarted => "restarted",
        AssignmentEventKind::Stalled => "stalled",
        AssignmentEventKind::BudgetExhausted => "budget_exhausted",
    }
}

fn event_kind_from_str(value: &str) -> CowResult<AssignmentEventKind> {
    match value {
        "enqueued" => Ok(AssignmentEventKind::Enqueued),
        "started" => Ok(AssignmentEventKind::Started),
        "waiting" => Ok(AssignmentEventKind::Waiting),
        "resumed" => Ok(AssignmentEventKind::Resumed),
        "needs_input" => Ok(AssignmentEventKind::NeedsInput),
        "completed" => Ok(AssignmentEventKind::Completed),
        "failed" => Ok(AssignmentEventKind::Failed),
        "cancelled" => Ok(AssignmentEventKind::Cancelled),
        "restarted" => Ok(AssignmentEventKind::Restarted),
        "stalled" => Ok(AssignmentEventKind::Stalled),
        "budget_exhausted" => Ok(AssignmentEventKind::BudgetExhausted),
        other => Err(CowError::Storage(format!(
            "unknown assignment event kind '{other}'"
        ))),
    }
}

impl Store {
    pub async fn create_assignment(&self, assignment: &Assignment) -> CowResult<()> {
        let mut tx = self.pool().begin().await.map_err(db)?;

        sqlx::query(
            "INSERT INTO assignments
                (id, root_id, parent_id, depth, task_id, origin, requester, assignee,
                 mode, reply, status, executions, restarts, wakes,
                 request_budget, execution_budget, wake_budget, approval_level,
                 outcome, stall_strikes, repeated_errors, last_error,
                 created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&assignment.id.0)
        .bind(&assignment.root_id.0)
        .bind(assignment.parent_id.as_ref().map(|id| id.0.clone()))
        .bind(assignment.depth as i64)
        .bind(&assignment.task_id.0)
        .bind(origin_to_str(assignment.origin))
        .bind(serde_json::to_string(&assignment.requester)?)
        .bind(&assignment.assignee.0)
        .bind(mode_to_str(assignment.mode))
        .bind(reply_to_str(assignment.reply))
        .bind(status_to_str(assignment.status))
        .bind(assignment.executions as i64)
        .bind(assignment.restarts as i64)
        .bind(assignment.wakes as i64)
        .bind(assignment.budgets.requests as i64)
        .bind(assignment.budgets.executions as i64)
        .bind(assignment.budgets.wakes as i64)
        .bind(approval_level_to_str(assignment.approval_level))
        .bind(
            assignment
                .outcome
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(assignment.stall_strikes as i64)
        .bind(assignment.repeated_errors as i64)
        .bind(&assignment.last_error)
        .bind(assignment.created_at.to_rfc3339())
        .bind(assignment.updated_at.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        let event =
            AssignmentEvent::new(assignment.id.clone(), AssignmentEventKind::Enqueued, None);
        sqlx::query(
            "INSERT INTO assignment_events (id, assignment_id, kind, detail, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.assignment_id.0)
        .bind(event_kind_to_str(event.kind))
        .bind(&event.detail)
        .bind(event.created_at.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn get_assignment(&self, id: &AssignmentId) -> CowResult<Option<Assignment>> {
        let row = sqlx::query_as::<_, AssignmentRow>(
            "SELECT id, root_id, parent_id, depth, task_id, origin, requester, assignee,
                    mode, reply, status, executions, restarts, wakes,
                    request_budget, execution_budget, wake_budget, approval_level,
                    outcome, stall_strikes, repeated_errors, last_error,
                    created_at, updated_at
             FROM assignments WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(AssignmentRow::into_domain).transpose()
    }

    pub async fn list_assignments_for_task(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>> {
        let rows = sqlx::query_as::<_, AssignmentRow>(
            "SELECT id, root_id, parent_id, depth, task_id, origin, requester, assignee,
                    mode, reply, status, executions, restarts, wakes,
                    request_budget, execution_budget, wake_budget, approval_level,
                    outcome, stall_strikes, repeated_errors, last_error,
                    created_at, updated_at
             FROM assignments WHERE task_id = ? ORDER BY created_at",
        )
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(AssignmentRow::into_domain).collect()
    }

    /// Every assignment in a run: the owning task's own assignments and the
    /// assignments of tasks whose `run_group` is that task.
    pub async fn list_assignments_for_run(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>> {
        let rows = sqlx::query_as::<_, AssignmentRow>(
            "SELECT a.id, a.root_id, a.parent_id, a.depth, a.task_id, a.origin, a.requester,
                    a.assignee, a.mode, a.reply, a.status, a.executions, a.restarts, a.wakes,
                    a.request_budget, a.execution_budget, a.wake_budget, a.approval_level,
                    a.outcome, a.stall_strikes, a.repeated_errors, a.last_error,
                    a.created_at, a.updated_at
             FROM assignments a
             JOIN tasks t ON t.id = a.task_id
             WHERE t.id = ? OR t.run_group = ?
             ORDER BY a.created_at",
        )
        .bind(&task_id.0)
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(AssignmentRow::into_domain).collect()
    }

    pub async fn list_child_assignments(
        &self,
        parent_id: &AssignmentId,
    ) -> CowResult<Vec<Assignment>> {
        let rows = sqlx::query_as::<_, AssignmentRow>(
            "SELECT id, root_id, parent_id, depth, task_id, origin, requester, assignee,
                    mode, reply, status, executions, restarts, wakes,
                    request_budget, execution_budget, wake_budget, approval_level,
                    outcome, stall_strikes, repeated_errors, last_error,
                    created_at, updated_at
             FROM assignments WHERE parent_id = ? ORDER BY created_at",
        )
        .bind(&parent_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(AssignmentRow::into_domain).collect()
    }

    pub async fn list_assignments_by_status(
        &self,
        status: AssignmentStatus,
    ) -> CowResult<Vec<Assignment>> {
        let rows = sqlx::query_as::<_, AssignmentRow>(
            "SELECT id, root_id, parent_id, depth, task_id, origin, requester, assignee,
                    mode, reply, status, executions, restarts, wakes,
                    request_budget, execution_budget, wake_budget, approval_level,
                    outcome, stall_strikes, repeated_errors, last_error,
                    created_at, updated_at
             FROM assignments WHERE status = ? ORDER BY created_at",
        )
        .bind(status_to_str(status))
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(AssignmentRow::into_domain).collect()
    }

    pub async fn save_assignment(
        &self,
        assignment: &Assignment,
        event_kind: AssignmentEventKind,
        detail: Option<String>,
    ) -> CowResult<()> {
        let mut tx = self.pool().begin().await.map_err(db)?;

        sqlx::query(
            "UPDATE assignments SET
                status = ?, executions = ?, restarts = ?, wakes = ?,
                approval_level = ?, outcome = ?, stall_strikes = ?, repeated_errors = ?,
                last_error = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(status_to_str(assignment.status))
        .bind(assignment.executions as i64)
        .bind(assignment.restarts as i64)
        .bind(assignment.wakes as i64)
        .bind(approval_level_to_str(assignment.approval_level))
        .bind(
            assignment
                .outcome
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(assignment.stall_strikes as i64)
        .bind(assignment.repeated_errors as i64)
        .bind(&assignment.last_error)
        .bind(assignment.updated_at.to_rfc3339())
        .bind(&assignment.id.0)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        let event = AssignmentEvent::new(assignment.id.clone(), event_kind, detail);
        sqlx::query(
            "INSERT INTO assignment_events (id, assignment_id, kind, detail, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.assignment_id.0)
        .bind(event_kind_to_str(event.kind))
        .bind(&event.detail)
        .bind(event.created_at.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn update_assignment_progress(&self, assignment: &Assignment) -> CowResult<()> {
        sqlx::query(
            "UPDATE assignments SET
                stall_strikes = ?, repeated_errors = ?, last_error = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(assignment.stall_strikes as i64)
        .bind(assignment.repeated_errors as i64)
        .bind(&assignment.last_error)
        .bind(assignment.updated_at.to_rfc3339())
        .bind(&assignment.id.0)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Change the approval level of a queued assignment before it starts.
    pub async fn set_assignment_approval_level(
        &self,
        id: &AssignmentId,
        approval_level: ApprovalLevel,
    ) -> CowResult<()> {
        sqlx::query("UPDATE assignments SET approval_level = ?, updated_at = ? WHERE id = ?")
            .bind(approval_level_to_str(approval_level))
            .bind(Utc::now().to_rfc3339())
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn list_assignment_events(
        &self,
        assignment_id: &AssignmentId,
    ) -> CowResult<Vec<AssignmentEvent>> {
        let rows = sqlx::query_as::<_, AssignmentEventRow>(
            "SELECT id, assignment_id, kind, detail, created_at
             FROM assignment_events WHERE assignment_id = ? ORDER BY created_at",
        )
        .bind(&assignment_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(AssignmentEventRow::into_domain)
            .collect()
    }

    pub async fn count_assignments_for_root(&self, root_id: &AssignmentId) -> CowResult<i64> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assignments WHERE root_id = ?")
            .bind(&root_id.0)
            .fetch_one(self.pool())
            .await
            .map_err(db)
    }

    pub async fn sum_executions_for_root(&self, root_id: &AssignmentId) -> CowResult<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(executions), 0) FROM assignments WHERE root_id = ?",
        )
        .bind(&root_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)
    }

    pub async fn sum_wakes_for_root(&self, root_id: &AssignmentId) -> CowResult<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(wakes), 0) FROM assignments WHERE root_id = ?",
        )
        .bind(&root_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)
    }
}
