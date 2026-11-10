use cowbot_core::error::{CowError, CowResult};
use cowbot_core::task::TaskId;
use cowbot_core::worktree::{TaskWorktree, WorktreeStatus};

use crate::error::{db, parse_datetime};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct WorktreeRow {
    task_id: String,
    project_id: String,
    project_path: String,
    base_branch: String,
    branch: String,
    worktree_path: String,
    status: String,
    created_at: String,
    updated_at: String,
}

impl WorktreeRow {
    fn into_domain(self) -> CowResult<TaskWorktree> {
        Ok(TaskWorktree {
            task_id: TaskId(self.task_id),
            project_id: self.project_id,
            project_path: self.project_path,
            base_branch: self.base_branch,
            branch: self.branch,
            worktree_path: self.worktree_path,
            status: status_from_str(&self.status)?,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

fn status_to_str(status: WorktreeStatus) -> &'static str {
    match status {
        WorktreeStatus::Requested => "requested",
        WorktreeStatus::Ready => "ready",
        WorktreeStatus::Removed => "removed",
        WorktreeStatus::Missing => "missing",
    }
}

fn status_from_str(value: &str) -> CowResult<WorktreeStatus> {
    match value {
        "requested" => Ok(WorktreeStatus::Requested),
        "ready" => Ok(WorktreeStatus::Ready),
        "removed" => Ok(WorktreeStatus::Removed),
        "missing" => Ok(WorktreeStatus::Missing),
        other => Err(CowError::Storage(format!(
            "unknown worktree status '{other}'"
        ))),
    }
}

const COLUMNS: &str = "task_id, project_id, project_path, base_branch, branch, worktree_path, \
                       status, created_at, updated_at";

impl Store {
    /// Insert or replace the worktree record for its task.
    pub async fn save_worktree(&self, worktree: &TaskWorktree) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO task_worktrees
                (task_id, project_id, project_path, base_branch, branch, worktree_path,
                 status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(task_id) DO UPDATE SET
                project_id = excluded.project_id,
                project_path = excluded.project_path,
                base_branch = excluded.base_branch,
                branch = excluded.branch,
                worktree_path = excluded.worktree_path,
                status = excluded.status,
                updated_at = excluded.updated_at",
        )
        .bind(&worktree.task_id.0)
        .bind(&worktree.project_id)
        .bind(&worktree.project_path)
        .bind(&worktree.base_branch)
        .bind(&worktree.branch)
        .bind(&worktree.worktree_path)
        .bind(status_to_str(worktree.status))
        .bind(worktree.created_at.to_rfc3339())
        .bind(worktree.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_worktree(&self, task_id: &TaskId) -> CowResult<Option<TaskWorktree>> {
        let row = sqlx::query_as::<_, WorktreeRow>(&format!(
            "SELECT {COLUMNS} FROM task_worktrees WHERE task_id = ?"
        ))
        .bind(&task_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(WorktreeRow::into_domain).transpose()
    }

    /// Every worktree record in this local workspace database.
    pub async fn list_worktrees(&self) -> CowResult<Vec<TaskWorktree>> {
        let rows = sqlx::query_as::<_, WorktreeRow>(&format!(
            "SELECT {COLUMNS} FROM task_worktrees ORDER BY created_at"
        ))
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(WorktreeRow::into_domain).collect()
    }
}
