use cowbot_bus::EventBus;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{Assignment, AssignmentOrigin, AssignmentRequester};
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::{Task, TaskId};
use cowbot_store::Store;

use crate::assignments::AssignmentService;

/// Application service for task creation, lookup, archiving, and cancellation.
#[derive(Clone)]
pub struct TaskService {
    store: Store,
    bus: EventBus,
    assignments: AssignmentService,
}

impl TaskService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self {
            assignments: AssignmentService::new(store.clone(), bus.clone()),
            store,
            bus,
        }
    }

    /// Persist a task and enqueue its root assignment.
    pub async fn create(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: Option<&ChannelId>,
        task: &Task,
        requester: AssignmentRequester,
        approval_level: ApprovalLevel,
    ) -> CowResult<Assignment> {
        let assignee = task
            .assigned_agent
            .clone()
            .ok_or_else(|| CowError::Task(format!("task '{}' has no assigned agent", task.id)))?;
        self.store
            .create_task(workspace_id, channel_id, task)
            .await?;
        self.assignments
            .enqueue_root(
                task,
                AssignmentOrigin::User,
                requester,
                assignee,
                approval_level,
            )
            .await
    }

    pub async fn list(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<Task>> {
        self.store.list_tasks(workspace_id).await
    }

    pub async fn get(&self, id: &TaskId) -> CowResult<Option<Task>> {
        self.store.get_task(id).await
    }

    /// Tasks that belong to the run started by `task_id`.
    pub async fn list_run_tasks(&self, task_id: &TaskId) -> CowResult<Vec<Task>> {
        self.store.list_tasks_by_run_group(task_id).await
    }

    /// Cancel the task's active root assignment; task status follows the
    /// assignment lifecycle.
    pub async fn cancel(&self, task_id: &TaskId, reason: impl Into<String>) -> CowResult<()> {
        let assignments = self.store.list_assignments_for_task(task_id).await?;
        let root = assignments
            .iter()
            .find(|assignment| assignment.parent_id.is_none() && !assignment.is_terminal())
            .ok_or_else(|| CowError::Task(format!("task '{task_id}' has no active assignment")))?;
        self.assignments.cancel(&root.id, reason).await
    }

    /// Approve the plan stored with a task so the user can continue in Manual
    /// mode. Idempotent: approving an approved plan keeps it approved.
    pub async fn approve_plan(&self, task_id: &TaskId) -> CowResult<Task> {
        let Some(mut task) = self.store.get_task(task_id).await? else {
            return Err(CowError::Task(format!("task '{task_id}' was not found")));
        };
        let has_plan = task
            .plan
            .as_deref()
            .is_some_and(|plan| !plan.trim().is_empty());
        if !has_plan {
            return Err(CowError::Task(format!(
                "task '{task_id}' has no plan to approve"
            )));
        }
        if !task.plan_approved {
            self.store.approve_task_plan(task_id).await?;
            task.plan_approved = true;
            self.bus
                .publish(Event::new(EventPayload::TaskUpdated(task.clone())));
        }
        Ok(task)
    }

    /// Archive or restore a terminal task without deleting its history.
    pub async fn set_archived(&self, task_id: &TaskId, archived: bool) -> CowResult<Task> {
        let Some(mut task) = self.store.get_task(task_id).await? else {
            return Err(CowError::Task(format!("task '{task_id}' was not found")));
        };
        if !task.status.is_terminal() {
            return Err(CowError::Task(format!(
                "task '{task_id}' must complete, fail, or be cancelled before it can be archived"
            )));
        }
        self.store.set_task_archived(task_id, archived).await?;
        task.archived = archived;
        self.bus
            .publish(Event::new(EventPayload::TaskUpdated(task.clone())));
        Ok(task)
    }
}
