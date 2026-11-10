use chrono::{DateTime, Utc};
use cowbot_core::agent::AgentId;
use cowbot_core::approval::{Approval, ApprovalId, ApprovalStatus};
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::run::{Run, RunId, RunStatus, ToolCallRecord};
use cowbot_core::task::{Task, TaskId, TaskStatus};
use serde_json::Value;

use crate::error::{db, parse_datetime, parse_json, parse_optional_datetime};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    channel_id: Option<String>,
    title: String,
    description: String,
    expected_output: String,
    assigned_agent: Option<String>,
    context_tasks: String,
    run_group: Option<String>,
    status: String,
    archived_at: Option<String>,
    plan: Option<String>,
    plan_approved: i64,
}

impl TaskRow {
    fn into_domain(self) -> CowResult<Task> {
        Ok(Task {
            id: TaskId(self.id),
            title: self.title,
            description: self.description,
            expected_output: self.expected_output,
            assigned_agent: self.assigned_agent.map(AgentId),
            channel_id: self.channel_id.map(ChannelId::new),
            context_tasks: parse_json(&self.context_tasks, "tasks.context_tasks")?,
            run_group: self.run_group.map(TaskId),
            status: parse_json(&self.status, "tasks.status")?,
            archived: self.archived_at.is_some(),
            plan: self.plan,
            plan_approved: self.plan_approved != 0,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: String,
    task_id: String,
    agent_id: String,
    status: String,
    started_at: String,
    finished_at: Option<String>,
}

impl RunRow {
    fn into_domain(self) -> CowResult<Run> {
        Ok(Run {
            id: RunId(self.id),
            task_id: TaskId(self.task_id),
            agent_id: AgentId(self.agent_id),
            status: run_status_from_str(&self.status)?,
            started_at: parse_datetime(&self.started_at)?,
            finished_at: parse_optional_datetime(self.finished_at.as_deref())?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ToolCallRow {
    id: String,
    run_id: String,
    name: String,
    arguments: String,
    result: Option<String>,
    approved_by: Option<String>,
    created_at: String,
}

impl ToolCallRow {
    fn into_domain(self) -> CowResult<ToolCallRecord> {
        Ok(ToolCallRecord {
            id: self.id,
            run_id: RunId(self.run_id),
            name: self.name,
            arguments: parse_json(&self.arguments, "tool_calls.arguments")?,
            result: self
                .result
                .as_deref()
                .map(|value| parse_json(value, "tool_calls.result"))
                .transpose()?,
            approved_by: self.approved_by,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ApprovalRow {
    id: String,
    task_id: String,
    tool_call_id: String,
    status: String,
    decided_by: Option<String>,
    decided_at: Option<String>,
    created_at: String,
}

impl ApprovalRow {
    fn into_domain(self) -> CowResult<Approval> {
        Ok(Approval {
            id: ApprovalId(self.id),
            task_id: TaskId(self.task_id),
            tool_call_id: self.tool_call_id,
            status: approval_status_from_str(&self.status)?,
            decided_by: self.decided_by,
            decided_at: parse_optional_datetime(self.decided_at.as_deref())?,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

fn run_status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::WaitingApproval => "waiting_approval",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn run_status_from_str(value: &str) -> CowResult<RunStatus> {
    match value {
        "running" => Ok(RunStatus::Running),
        "waiting_approval" => Ok(RunStatus::WaitingApproval),
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        "cancelled" => Ok(RunStatus::Cancelled),
        other => Err(CowError::Storage(format!("unknown run status '{other}'"))),
    }
}

fn approval_status_to_str(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Rejected => "rejected",
        ApprovalStatus::Expired => "expired",
    }
}

fn approval_status_from_str(value: &str) -> CowResult<ApprovalStatus> {
    match value {
        "pending" => Ok(ApprovalStatus::Pending),
        "approved" => Ok(ApprovalStatus::Approved),
        "rejected" => Ok(ApprovalStatus::Rejected),
        "expired" => Ok(ApprovalStatus::Expired),
        other => Err(CowError::Storage(format!(
            "unknown approval status '{other}'"
        ))),
    }
}

impl Store {
    pub async fn create_task(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
        task: &Task,
    ) -> CowResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO tasks
                (id, workspace_id, channel_id, title, description, expected_output,
                 assigned_agent, context_tasks, run_group, status, archived_at, plan, plan_approved,
                 created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(&task.id.0)
        .bind(&workspace_id.0)
        .bind(
            channel_id
                .or(task.channel_id.as_ref())
                .map(|channel| channel.0.clone()),
        )
        .bind(&task.title)
        .bind(&task.description)
        .bind(&task.expected_output)
        .bind(task.assigned_agent.as_ref().map(|agent| agent.0.clone()))
        .bind(serde_json::to_string(&task.context_tasks)?)
        .bind(task.run_group.as_ref().map(|task_id| task_id.0.clone()))
        .bind(serde_json::to_string(&task.status)?)
        .bind(&task.plan)
        .bind(i64::from(task.plan_approved))
        .bind(&now)
        .bind(&now)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_task(&self, id: &TaskId) -> CowResult<Option<Task>> {
        let row = sqlx::query_as::<_, TaskRow>(
            "SELECT id, channel_id, title, description, expected_output, assigned_agent,
                    context_tasks, run_group, status, archived_at, plan, plan_approved
             FROM tasks WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(TaskRow::into_domain).transpose()
    }

    pub async fn update_task_status(&self, id: &TaskId, status: &TaskStatus) -> CowResult<()> {
        sqlx::query("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?")
            .bind(serde_json::to_string(status)?)
            .bind(Utc::now().to_rfc3339())
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Task>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT id, channel_id, title, description, expected_output, assigned_agent,
                    context_tasks, run_group, status, archived_at, plan, plan_approved
             FROM tasks WHERE workspace_id = ? ORDER BY created_at DESC",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(TaskRow::into_domain).collect()
    }

    /// Tasks created by one run, in creation order.
    pub async fn list_tasks_by_run_group(&self, run_group: &TaskId) -> CowResult<Vec<Task>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT id, channel_id, title, description, expected_output, assigned_agent,
                    context_tasks, run_group, status, archived_at, plan, plan_approved
             FROM tasks WHERE run_group = ? ORDER BY created_at",
        )
        .bind(&run_group.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(TaskRow::into_domain).collect()
    }

    pub async fn set_task_archived(&self, id: &TaskId, archived: bool) -> CowResult<()> {
        let archived_at = archived.then(|| Utc::now().to_rfc3339());
        sqlx::query("UPDATE tasks SET archived_at = ?, updated_at = ? WHERE id = ?")
            .bind(archived_at)
            .bind(Utc::now().to_rfc3339())
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Store a Plan-mode artifact and clear any previous approval.
    pub async fn set_task_plan(&self, id: &TaskId, plan: &str) -> CowResult<()> {
        sqlx::query("UPDATE tasks SET plan = ?, plan_approved = 0, updated_at = ? WHERE id = ?")
            .bind(plan)
            .bind(Utc::now().to_rfc3339())
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Mark the stored plan approved so the task can continue in Manual mode.
    pub async fn approve_task_plan(&self, id: &TaskId) -> CowResult<()> {
        sqlx::query("UPDATE tasks SET plan_approved = 1, updated_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn create_run(&self, run: &Run) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO runs (id, task_id, agent_id, status, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.id.0)
        .bind(&run.task_id.0)
        .bind(&run.agent_id.0)
        .bind(run_status_to_str(run.status))
        .bind(run.started_at.to_rfc3339())
        .bind(run.finished_at.map(|value| value.to_rfc3339()))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn update_run_status(
        &self,
        id: &RunId,
        status: RunStatus,
        finished_at: Option<DateTime<Utc>>,
    ) -> CowResult<()> {
        sqlx::query("UPDATE runs SET status = ?, finished_at = ? WHERE id = ?")
            .bind(run_status_to_str(status))
            .bind(finished_at.map(|value| value.to_rfc3339()))
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn get_run(&self, id: &RunId) -> CowResult<Option<Run>> {
        let row = sqlx::query_as::<_, RunRow>(
            "SELECT id, task_id, agent_id, status, started_at, finished_at
             FROM runs WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(RunRow::into_domain).transpose()
    }

    pub async fn list_runs(&self, task_id: &TaskId) -> CowResult<Vec<Run>> {
        let rows = sqlx::query_as::<_, RunRow>(
            "SELECT id, task_id, agent_id, status, started_at, finished_at
             FROM runs WHERE task_id = ? ORDER BY started_at",
        )
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(RunRow::into_domain).collect()
    }

    pub async fn record_tool_call(&self, call: &ToolCallRecord) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO tool_calls
                (id, run_id, name, arguments, result, approved_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&call.id)
        .bind(&call.run_id.0)
        .bind(&call.name)
        .bind(serde_json::to_string(&call.arguments)?)
        .bind(
            call.result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(&call.approved_by)
        .bind(call.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn update_tool_call_result(
        &self,
        id: &str,
        result: &Value,
        approved_by: Option<&str>,
    ) -> CowResult<()> {
        sqlx::query("UPDATE tool_calls SET result = ?, approved_by = ? WHERE id = ?")
            .bind(serde_json::to_string(result)?)
            .bind(approved_by)
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn list_tool_calls(&self, run_id: &RunId) -> CowResult<Vec<ToolCallRecord>> {
        let rows = sqlx::query_as::<_, ToolCallRow>(
            "SELECT id, run_id, name, arguments, result, approved_by, created_at
             FROM tool_calls WHERE run_id = ? ORDER BY created_at",
        )
        .bind(&run_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ToolCallRow::into_domain).collect()
    }

    /// Newest tool call in a run, including the owning task's crew tasks.
    pub async fn latest_tool_call_for_task(
        &self,
        task_id: &TaskId,
    ) -> CowResult<Option<ToolCallRecord>> {
        let row = sqlx::query_as::<_, ToolCallRow>(
            "SELECT tc.id, tc.run_id, tc.name, tc.arguments, tc.result, tc.approved_by,
                    tc.created_at
             FROM tool_calls tc
             JOIN runs r ON r.id = tc.run_id
             JOIN tasks t ON t.id = r.task_id
             WHERE t.id = ? OR t.run_group = ?
             ORDER BY tc.created_at DESC
             LIMIT 1",
        )
        .bind(&task_id.0)
        .bind(&task_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ToolCallRow::into_domain).transpose()
    }

    pub async fn create_approval(&self, approval: &Approval) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO approvals
                (id, task_id, tool_call_id, status, decided_by, decided_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&approval.id.0)
        .bind(&approval.task_id.0)
        .bind(&approval.tool_call_id)
        .bind(approval_status_to_str(approval.status))
        .bind(&approval.decided_by)
        .bind(approval.decided_at.map(|value| value.to_rfc3339()))
        .bind(approval.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn decide_approval(
        &self,
        id: &ApprovalId,
        status: ApprovalStatus,
        decided_by: &str,
    ) -> CowResult<()> {
        let updated = sqlx::query(
            "UPDATE approvals SET status = ?, decided_by = ?, decided_at = ?
             WHERE id = ? AND status = 'pending'",
        )
        .bind(approval_status_to_str(status))
        .bind(decided_by)
        .bind(Utc::now().to_rfc3339())
        .bind(&id.0)
        .execute(self.pool())
        .await
        .map_err(db)?
        .rows_affected();
        if updated == 0 {
            return Err(CowError::Task(format!(
                "approval '{id}' is not pending and cannot be decided"
            )));
        }
        Ok(())
    }

    pub async fn get_approval(&self, id: &ApprovalId) -> CowResult<Option<Approval>> {
        let row = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, task_id, tool_call_id, status, decided_by, decided_at, created_at
             FROM approvals WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(ApprovalRow::into_domain).transpose()
    }

    pub async fn list_pending_approvals(&self, task_id: &TaskId) -> CowResult<Vec<Approval>> {
        let rows = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, task_id, tool_call_id, status, decided_by, decided_at, created_at
             FROM approvals WHERE task_id = ? AND status = 'pending' ORDER BY created_at",
        )
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ApprovalRow::into_domain).collect()
    }

    pub async fn list_approvals_for_task(&self, task_id: &TaskId) -> CowResult<Vec<Approval>> {
        let rows = sqlx::query_as::<_, ApprovalRow>(
            "SELECT id, task_id, tool_call_id, status, decided_by, decided_at, created_at
             FROM approvals WHERE task_id = ? ORDER BY created_at",
        )
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ApprovalRow::into_domain).collect()
    }

    pub async fn list_pending_approvals_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<Approval>> {
        let rows = sqlx::query_as::<_, ApprovalRow>(
            "SELECT a.id, a.task_id, a.tool_call_id, a.status, a.decided_by, a.decided_at, a.created_at
             FROM approvals a
             JOIN tasks t ON t.id = a.task_id
             WHERE a.status = 'pending' AND t.workspace_id = ?
             ORDER BY a.created_at",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(ApprovalRow::into_domain).collect()
    }
}
