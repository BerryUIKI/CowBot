use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::Utc;
use cowbot_bus::EventBus;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::worktree::{TaskWorktree, TaskWorktreeDiff, WorktreeStatus, WORKTREE_PATCH_LIMIT};
use cowbot_store::Store;

/// Create, inspect, and discard per-task git worktrees (ADR-0031).
#[derive(Clone)]
pub struct WorktreeService {
    store: Store,
    bus: EventBus,
}

impl WorktreeService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self { store, bus }
    }

    /// Record that a task wants a worktree. Nothing is created yet.
    ///
    /// Idempotent: an existing record is returned unchanged.
    pub async fn request(
        &self,
        task_id: &TaskId,
        project_id: &str,
        project_path: &str,
        base_branch: &str,
    ) -> CowResult<TaskWorktree> {
        if let Some(existing) = self.store.get_worktree(task_id).await? {
            return Ok(existing);
        }
        if self.store.get_task(task_id).await?.is_none() {
            return Err(CowError::Task(format!("task '{task_id}' was not found")));
        }

        let now = Utc::now();
        let record = TaskWorktree {
            task_id: task_id.clone(),
            project_id: project_id.to_string(),
            project_path: project_path.to_string(),
            base_branch: base_branch.to_string(),
            branch: String::new(),
            worktree_path: String::new(),
            status: WorktreeStatus::Requested,
            created_at: now,
            updated_at: now,
        };
        self.store.save_worktree(&record).await?;
        self.publish(&record);
        Ok(record)
    }

    /// Create the branch and checkout for a requested worktree.
    ///
    /// Idempotent while the record is ready; a discarded or missing worktree
    /// needs a new task.
    pub async fn prepare(&self, task_id: &TaskId) -> CowResult<TaskWorktree> {
        let mut record = self.required(task_id).await?;
        match record.status {
            WorktreeStatus::Ready => return Ok(record),
            WorktreeStatus::Requested => {}
            WorktreeStatus::Removed => {
                return Err(CowError::Task(
                    "this task's worktree was discarded; create a new task to run in a worktree"
                        .to_string(),
                ))
            }
            WorktreeStatus::Missing => {
                return Err(CowError::Task(
                    "the worktree folder is missing. Discard this worktree and create a new task"
                        .to_string(),
                ))
            }
        }

        let Some(task) = self.store.get_task(task_id).await? else {
            return Err(CowError::Task(format!("task '{task_id}' was not found")));
        };
        let root = self.resolve_root(&record.project_path).await?;
        self.validate_base(&root, &record.base_branch).await?;

        let slug = slug_for(&task);
        let branch = format!("cowbot/{slug}");
        let worktree_path = root.join(".cowbot").join("worktrees").join(&slug);
        if worktree_path.exists() {
            return Err(CowError::Task(format!(
                "the worktree path {} already exists. Remove it or create a new task",
                worktree_path.display()
            )));
        }
        if self
            .ref_exists(&root, &format!("refs/heads/{branch}"))
            .await?
        {
            return Err(CowError::Task(format!(
                "branch '{branch}' already exists in {}. Delete it or create a new task",
                root.display()
            )));
        }

        self.ensure_excluded(&root).await?;
        if let Some(parent) = worktree_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CowError::Task(format!("failed to create {}: {error}", parent.display()))
            })?;
        }

        let worktree_text = worktree_path.display().to_string();
        if let Err(error) = self
            .git_output(
                &root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    &worktree_text,
                    &record.base_branch,
                ],
            )
            .await
        {
            return Err(CowError::Task(format!(
                "git could not create the worktree at {worktree_text}: {error}"
            )));
        }

        record.branch = branch;
        record.worktree_path = worktree_text;
        record.status = WorktreeStatus::Ready;
        record.updated_at = Utc::now();
        self.store.save_worktree(&record).await?;
        self.publish(&record);
        Ok(record)
    }

    /// Remove the checkout and branch, keeping the record for history.
    pub async fn discard(&self, task_id: &TaskId) -> CowResult<TaskWorktree> {
        let mut record = self.required(task_id).await?;
        if record.status == WorktreeStatus::Removed {
            return Ok(record);
        }

        let project_available = Path::new(&record.project_path).is_dir();
        let worktree_exists =
            !record.worktree_path.is_empty() && Path::new(&record.worktree_path).exists();
        if project_available && worktree_exists {
            self.git_output(
                Path::new(&record.project_path),
                &["worktree", "remove", "--force", &record.worktree_path],
            )
            .await?;
        } else if project_available {
            // The folder is gone but git may still think the branch is in use;
            // prune its stale administrative entry before deleting the branch.
            let _ = self
                .git_output(Path::new(&record.project_path), &["worktree", "prune"])
                .await;
        }
        if project_available && !record.branch.is_empty() {
            let reference = format!("refs/heads/{}", record.branch);
            if self
                .ref_exists(Path::new(&record.project_path), &reference)
                .await?
            {
                self.git_output(
                    Path::new(&record.project_path),
                    &["branch", "-D", &record.branch],
                )
                .await?;
            }
        }

        record.status = WorktreeStatus::Removed;
        record.updated_at = Utc::now();
        self.store.save_worktree(&record).await?;
        self.publish(&record);
        Ok(record)
    }

    /// Status, diffstat, and patch of a ready worktree.
    pub async fn diff(&self, task_id: &TaskId) -> CowResult<TaskWorktreeDiff> {
        let record = self.required(task_id).await?;
        if record.status != WorktreeStatus::Ready {
            return Err(CowError::Task(format!(
                "worktree '{}' is {:?}; there is no diff to show",
                record.task_id, record.status
            )));
        }
        let path = Path::new(&record.worktree_path);
        if !path.is_dir() {
            return Err(CowError::Task(format!(
                "worktree folder {} is missing",
                record.worktree_path
            )));
        }

        let status = self.git_output(path, &["status", "--porcelain"]).await?;
        let stat = self
            .git_output(path, &["diff", &record.base_branch, "--stat"])
            .await?;
        let patch = self
            .git_output(path, &["diff", &record.base_branch])
            .await?;
        let (patch, truncated) = cap_patch(patch);
        Ok(TaskWorktreeDiff {
            status,
            stat,
            patch,
            truncated,
        })
    }

    pub async fn get(&self, task_id: &TaskId) -> CowResult<Option<TaskWorktree>> {
        self.store.get_worktree(task_id).await
    }

    pub async fn list(&self) -> CowResult<Vec<TaskWorktree>> {
        self.store.list_worktrees().await
    }

    /// Mark ready records whose folder disappeared as missing.
    pub async fn reconcile(&self) -> CowResult<Vec<TaskWorktree>> {
        let mut changed = Vec::new();
        for mut record in self.store.list_worktrees().await? {
            if record.status == WorktreeStatus::Ready && !Path::new(&record.worktree_path).exists()
            {
                record.status = WorktreeStatus::Missing;
                record.updated_at = Utc::now();
                self.store.save_worktree(&record).await?;
                self.publish(&record);
                changed.push(record);
            }
        }
        Ok(changed)
    }

    async fn required(&self, task_id: &TaskId) -> CowResult<TaskWorktree> {
        self.store
            .get_worktree(task_id)
            .await?
            .ok_or_else(|| CowError::Task(format!("task '{task_id}' has no worktree")))
    }

    async fn resolve_root(&self, project_path: &str) -> CowResult<PathBuf> {
        let root = PathBuf::from(project_path);
        if !root.is_dir() {
            return Err(CowError::Task(format!(
                "project folder {} is not available on this machine",
                root.display()
            )));
        }
        let toplevel = self
            .git_output(&root, &["rev-parse", "--show-toplevel"])
            .await
            .map_err(|error| {
                CowError::Task(format!(
                    "{} is not a git repository: {error}",
                    root.display()
                ))
            })?;
        Ok(PathBuf::from(toplevel))
    }

    async fn validate_base(&self, root: &Path, base_branch: &str) -> CowResult<()> {
        if self
            .ref_exists(root, &format!("refs/heads/{base_branch}"))
            .await?
        {
            return Ok(());
        }
        let branches = self
            .git_output(
                root,
                &["for-each-ref", "refs/heads", "--format=%(refname:short)"],
            )
            .await
            .unwrap_or_default();
        let branches = branches.replace('\n', ", ");
        Err(CowError::Task(format!(
            "base branch '{base_branch}' does not exist in {}. Available branches: {branches}",
            root.display()
        )))
    }

    async fn ref_exists(&self, directory: &Path, reference: &str) -> CowResult<bool> {
        let output = self
            .git_raw(directory, &["show-ref", "--verify", "--quiet", reference])
            .await?;
        Ok(output.status.success())
    }

    /// Append `.cowbot/` to the repository's local exclude file.
    async fn ensure_excluded(&self, root: &Path) -> CowResult<()> {
        let git_path = self
            .git_output(root, &["rev-parse", "--git-path", "info/exclude"])
            .await?;
        let exclude = PathBuf::from(git_path);
        let exclude = if exclude.is_absolute() {
            exclude
        } else {
            root.join(exclude)
        };
        tokio::task::spawn_blocking(move || append_exclude(&exclude))
            .await
            .map_err(|error| {
                CowError::Task(format!("failed to update .git/info/exclude: {error}"))
            })??;
        Ok(())
    }

    async fn git_output(&self, directory: &Path, args: &[&str]) -> CowResult<String> {
        let output = self.git_raw(directory, args).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CowError::Task(if stderr.is_empty() {
                format!("git {} failed", args.join(" "))
            } else {
                stderr
            }));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn git_raw(&self, directory: &Path, args: &[&str]) -> CowResult<Output> {
        let directory = directory.to_path_buf();
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || {
            Command::new("git")
                .arg("-C")
                .arg(&directory)
                .args(&args)
                .output()
        })
        .await
        .map_err(|error| CowError::Task(format!("git task failed: {error}")))?
        .map_err(|error| {
            CowError::Task(format!(
                "could not run git: {error}. Install git and make sure it is on PATH."
            ))
        })
    }

    fn publish(&self, record: &TaskWorktree) {
        self.bus.publish(Event::new(EventPayload::WorktreeUpdated {
            task_id: record.task_id.clone(),
            status: record.status,
        }));
    }
}

/// Slug and four id characters used for the branch and folder name.
pub fn slug_for(task: &Task) -> String {
    let source = if task.title.trim().is_empty() {
        task.description.as_str()
    } else {
        task.title.as_str()
    };
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in source.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
            if slug.len() >= 32 {
                break;
            }
        } else if !slug.is_empty() {
            pending_dash = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "task" } else { slug };
    let suffix = task
        .id
        .0
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(4)
        .collect::<String>();
    format!("{slug}-{suffix}")
}

fn cap_patch(patch: String) -> (String, bool) {
    if patch.len() <= WORKTREE_PATCH_LIMIT {
        return (patch, false);
    }
    let mut end = WORKTREE_PATCH_LIMIT;
    while !patch.is_char_boundary(end) {
        end -= 1;
    }
    (patch[..end].to_string(), true)
}

fn append_exclude(path: &Path) -> CowResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CowError::Task(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let contents = fs::read_to_string(path).unwrap_or_default();
    if contents
        .lines()
        .any(|line| matches!(line.trim(), ".cowbot" | ".cowbot/"))
    {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| CowError::Task(format!("failed to open {}: {error}", path.display())))?;
    if !contents.is_empty() && !contents.ends_with('\n') {
        writeln!(file).map_err(|error| CowError::Task(error.to_string()))?;
    }
    writeln!(file, ".cowbot/").map_err(|error| CowError::Task(error.to_string()))?;
    Ok(())
}
