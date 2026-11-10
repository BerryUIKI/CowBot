use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::task::TaskId;

/// Lifecycle of a task's git worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    /// The task asked for a worktree; nothing has been created yet.
    Requested,
    /// The worktree exists at `worktree_path` on `branch`.
    Ready,
    /// The user discarded the worktree; the record is kept for history.
    Removed,
    /// The recorded worktree folder no longer exists on disk.
    Missing,
}

/// One task's git worktree.
///
/// Records are keyed by the owning task (the run group task for a crew run)
/// and are local to this machine: they are never exported in a `.cowpkg`
/// package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWorktree {
    pub task_id: TaskId,
    pub project_id: String,
    /// Canonical repository root the worktree belongs to.
    pub project_path: String,
    pub base_branch: String,
    /// Branch created for the worktree, for example `cowbot/fix-login-a1b2`.
    pub branch: String,
    /// Folder of the checkout, for example `<repo>/.cowbot/worktrees/fix-login-a1b2`.
    pub worktree_path: String,
    pub status: WorktreeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A worktree's uncommitted state relative to its base branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWorktreeDiff {
    /// `git status --porcelain` output.
    pub status: String,
    /// `git diff <base> --stat` output.
    pub stat: String,
    /// Full patch, capped at 256 KiB.
    pub patch: String,
    /// Whether `patch` was cut at the cap.
    pub truncated: bool,
}

/// Maximum number of bytes returned in [`TaskWorktreeDiff::patch`].
pub const WORKTREE_PATCH_LIMIT: usize = 256 * 1024;
