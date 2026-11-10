use chrono::Utc;
use cowbot_bus::EventBus;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{
    Assignment, AssignmentEvent, AssignmentEventKind, AssignmentId, AssignmentMode,
    AssignmentOrigin, AssignmentOutcome, AssignmentRequester, AssignmentStatus, ReplyMode,
    MAX_ASSIGNMENT_DEPTH,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::task::{Task, TaskId, TaskStatus};
use cowbot_store::Store;

/// Application service that owns assignment lifecycle transitions.
#[derive(Clone)]
pub struct AssignmentService {
    store: Store,
    bus: EventBus,
}

impl AssignmentService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self { store, bus }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub async fn enqueue_root(
        &self,
        task: &Task,
        origin: AssignmentOrigin,
        requester: AssignmentRequester,
        assignee: cowbot_core::agent::AgentId,
        approval_level: ApprovalLevel,
    ) -> CowResult<Assignment> {
        let assignment = Assignment::new_root(
            task.id.clone(),
            origin,
            requester,
            assignee,
            AssignmentMode::Work,
            ReplyMode::Resume,
            approval_level,
        );
        self.store.create_assignment(&assignment).await?;
        self.publish_task_created(task);
        self.publish_assignment_updated(&assignment, AssignmentEventKind::Enqueued, None);
        self.publish_task_assigned(task, &assignment);
        Ok(assignment)
    }

    pub async fn delegate(
        &self,
        parent_id: &AssignmentId,
        task: &Task,
        requester: AssignmentRequester,
        assignee: cowbot_core::agent::AgentId,
        approval_level: ApprovalLevel,
    ) -> CowResult<Assignment> {
        let parent = self.load(parent_id).await?;
        if parent.is_terminal() {
            return Err(CowError::Task(format!(
                "cannot delegate from terminal assignment '{}'",
                parent.id
            )));
        }
        if parent.depth + 1 > MAX_ASSIGNMENT_DEPTH {
            return Err(CowError::Task(format!(
                "assignment depth limit ({MAX_ASSIGNMENT_DEPTH}) reached"
            )));
        }

        let request_count = self
            .store
            .count_assignments_for_root(&parent.root_id)
            .await?;
        if request_count >= i64::from(parent.budgets.requests) {
            return Err(CowError::Task(format!(
                "assignment request budget exhausted for root '{}'",
                parent.root_id
            )));
        }

        if self.is_ancestor_assignee(&parent, &requester).await? {
            return Err(CowError::Task(
                "delegation cannot return work to an ancestor agent".to_string(),
            ));
        }

        let child = Assignment::new_child(
            &parent,
            task.id.clone(),
            requester,
            assignee,
            AssignmentMode::Work,
            ReplyMode::Resume,
            cowbot_core::assignment::inherit_approval_level(parent.approval_level, approval_level),
        );
        self.store.create_assignment(&child).await?;
        self.publish_task_created(task);
        self.publish_assignment_updated(&child, AssignmentEventKind::Enqueued, None);
        self.publish_task_assigned(task, &child);
        Ok(child)
    }

    pub async fn start(&self, id: &AssignmentId) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status == AssignmentStatus::Running {
            return Ok(assignment);
        }
        if assignment.status.is_terminal() {
            return Err(CowError::Task(format!(
                "assignment '{}' is already terminal",
                assignment.id
            )));
        }

        let executions = self
            .store
            .sum_executions_for_root(&assignment.root_id)
            .await?;
        if executions >= i64::from(assignment.budgets.executions) {
            assignment.status = AssignmentStatus::Failed;
            assignment.outcome = Some(AssignmentOutcome::Failed {
                error: "execution budget exhausted".to_string(),
            });
            assignment.updated_at = Utc::now();
            self.save_assignment(
                &assignment,
                AssignmentEventKind::BudgetExhausted,
                Some("execution budget exhausted".to_string()),
            )
            .await?;
            self.store
                .update_task_status(
                    &assignment.task_id,
                    &TaskStatus::Failed {
                        error: "execution budget exhausted".to_string(),
                    },
                )
                .await?;
            return Err(CowError::Task(format!(
                "execution budget exhausted for root '{}'",
                assignment.root_id
            )));
        }

        assignment.status = AssignmentStatus::Running;
        assignment.executions += 1;
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::Started, None)
            .await?;
        self.store
            .update_task_status(&assignment.task_id, &TaskStatus::InProgress)
            .await?;
        Ok(assignment)
    }

    pub async fn mark_waiting(&self, id: &AssignmentId) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status != AssignmentStatus::Running {
            return Err(CowError::Task(format!(
                "assignment '{}' is not running",
                assignment.id
            )));
        }
        assignment.status = AssignmentStatus::Waiting;
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::Waiting, None)
            .await?;
        Ok(assignment)
    }

    pub async fn resume(&self, id: &AssignmentId) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status == AssignmentStatus::Resume {
            return Ok(assignment);
        }
        if assignment.status != AssignmentStatus::Waiting {
            return Err(CowError::Task(format!(
                "assignment '{}' is not waiting",
                assignment.id
            )));
        }
        assignment.status = AssignmentStatus::Resume;
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::Resumed, None)
            .await?;
        Ok(assignment)
    }

    pub async fn needs_input(
        &self,
        id: &AssignmentId,
        detail: Option<String>,
    ) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status.is_terminal() {
            return Err(CowError::Task(format!(
                "assignment '{}' is already terminal",
                assignment.id
            )));
        }
        assignment.status = AssignmentStatus::NeedsInput;
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::NeedsInput, detail)
            .await?;
        Ok(assignment)
    }

    pub async fn complete(
        &self,
        id: &AssignmentId,
        output: impl Into<String>,
    ) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status.is_terminal() {
            return Err(CowError::Task(format!(
                "assignment '{}' is already terminal",
                assignment.id
            )));
        }
        let output = output.into();
        assignment.status = AssignmentStatus::Completed;
        assignment.outcome = Some(AssignmentOutcome::Completed {
            output: output.clone(),
        });
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::Completed, None)
            .await?;
        self.store
            .update_task_status(
                &assignment.task_id,
                &TaskStatus::Completed {
                    output: output.clone(),
                },
            )
            .await?;
        self.bus.publish(Event::new(EventPayload::TaskCompleted {
            task_id: assignment.task_id.clone(),
            output,
        }));
        self.wake_parent(&assignment).await?;
        Ok(assignment)
    }

    pub async fn fail(&self, id: &AssignmentId, error: impl Into<String>) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.status.is_terminal() {
            return Err(CowError::Task(format!(
                "assignment '{}' is already terminal",
                assignment.id
            )));
        }
        let error = error.into();
        assignment.status = AssignmentStatus::Failed;
        assignment.outcome = Some(AssignmentOutcome::Failed {
            error: error.clone(),
        });
        assignment.updated_at = Utc::now();
        self.save_assignment(&assignment, AssignmentEventKind::Failed, None)
            .await?;
        self.store
            .update_task_status(
                &assignment.task_id,
                &TaskStatus::Failed {
                    error: error.clone(),
                },
            )
            .await?;
        self.bus.publish(Event::new(EventPayload::TaskFailed {
            task_id: assignment.task_id.clone(),
            error,
        }));
        self.wake_parent(&assignment).await?;
        Ok(assignment)
    }

    pub async fn cancel(&self, id: &AssignmentId, reason: impl Into<String>) -> CowResult<()> {
        let reason = reason.into();
        let mut stack = vec![id.clone()];
        while let Some(current_id) = stack.pop() {
            let children = self.store.list_child_assignments(&current_id).await?;
            for child in children {
                if !child.is_terminal() {
                    stack.push(child.id.clone());
                }
            }

            if let Some(mut assignment) = self.store.get_assignment(&current_id).await? {
                if !assignment.is_terminal() {
                    assignment.status = AssignmentStatus::Cancelled;
                    assignment.outcome = Some(AssignmentOutcome::Cancelled {
                        reason: reason.clone(),
                    });
                    assignment.updated_at = Utc::now();
                    self.save_assignment(
                        &assignment,
                        AssignmentEventKind::Cancelled,
                        Some(reason.clone()),
                    )
                    .await?;
                    self.store
                        .update_task_status(&assignment.task_id, &TaskStatus::Cancelled)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Reconcile interrupted assignments once at startup.
    pub async fn reconcile_on_startup(&self) -> CowResult<()> {
        for mut assignment in self
            .store
            .list_assignments_by_status(AssignmentStatus::Running)
            .await?
        {
            let children = self.store.list_child_assignments(&assignment.id).await?;
            let has_started_children = children
                .iter()
                .any(|child| child.status != AssignmentStatus::Queued);

            assignment.updated_at = Utc::now();
            if has_started_children {
                assignment.status = AssignmentStatus::Waiting;
                self.save_assignment(
                    &assignment,
                    AssignmentEventKind::Waiting,
                    Some("restart: children in flight".to_string()),
                )
                .await?;
            } else if assignment.restarts < 1 {
                assignment.restarts += 1;
                assignment.status = AssignmentStatus::Queued;
                self.save_assignment(
                    &assignment,
                    AssignmentEventKind::Restarted,
                    Some("restart: rerun once".to_string()),
                )
                .await?;
            } else {
                assignment.status = AssignmentStatus::Failed;
                assignment.outcome = Some(AssignmentOutcome::Failed {
                    error: "interrupted after restart".to_string(),
                });
                self.save_assignment(
                    &assignment,
                    AssignmentEventKind::Failed,
                    Some("restart: interrupted".to_string()),
                )
                .await?;
                self.store
                    .update_task_status(
                        &assignment.task_id,
                        &TaskStatus::Failed {
                            error: "interrupted after restart".to_string(),
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Record turn evidence for the stall guard.
    pub async fn record_progress(
        &self,
        id: &AssignmentId,
        successful_tool_calls: u32,
        error: Option<&str>,
    ) -> CowResult<Assignment> {
        let mut assignment = self.load(id).await?;
        if assignment.is_terminal() {
            return Ok(assignment);
        }

        if successful_tool_calls > 0 {
            assignment.stall_strikes = 0;
            assignment.repeated_errors = 0;
            assignment.last_error = None;
        } else {
            assignment.stall_strikes += 1;
            match error {
                Some(current) if assignment.last_error.as_deref() == Some(current) => {
                    assignment.repeated_errors += 1;
                }
                Some(current) => {
                    assignment.repeated_errors = 1;
                    assignment.last_error = Some(current.to_string());
                }
                None => {
                    assignment.repeated_errors = 0;
                }
            }
        }
        assignment.updated_at = Utc::now();

        if assignment.stall_strikes >= 2 || assignment.repeated_errors >= 3 {
            let reason = format!(
                "stalled after {} attempts without progress",
                assignment.stall_strikes.max(assignment.repeated_errors)
            );
            assignment.status = AssignmentStatus::Failed;
            assignment.outcome = Some(AssignmentOutcome::Failed {
                error: reason.clone(),
            });
            self.save_assignment(&assignment, AssignmentEventKind::Stalled, Some(reason))
                .await?;
        } else {
            self.store.update_assignment_progress(&assignment).await?;
        }
        Ok(assignment)
    }

    pub async fn get(&self, id: &AssignmentId) -> CowResult<Option<Assignment>> {
        self.store.get_assignment(id).await
    }

    pub async fn list_for_task(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>> {
        self.store.list_assignments_for_task(task_id).await
    }

    pub async fn events(&self, id: &AssignmentId) -> CowResult<Vec<AssignmentEvent>> {
        self.store.list_assignment_events(id).await
    }

    async fn load(&self, id: &AssignmentId) -> CowResult<Assignment> {
        self.store
            .get_assignment(id)
            .await?
            .ok_or_else(|| CowError::Task(format!("assignment '{}' was not found", id)))
    }

    async fn is_ancestor_assignee(
        &self,
        parent: &Assignment,
        requester: &AssignmentRequester,
    ) -> CowResult<bool> {
        let AssignmentRequester::Agent(agent) = requester else {
            return Ok(false);
        };
        let mut current = Some(parent.clone());
        while let Some(assignment) = current {
            if &assignment.assignee == agent {
                return Ok(true);
            }
            current = match &assignment.parent_id {
                Some(parent_id) => self.store.get_assignment(parent_id).await?,
                None => None,
            };
        }
        Ok(false)
    }

    async fn wake_parent(&self, child: &Assignment) -> CowResult<()> {
        if child.reply != ReplyMode::Resume {
            return Ok(());
        }
        let Some(parent_id) = &child.parent_id else {
            return Ok(());
        };
        let Some(mut parent) = self.store.get_assignment(parent_id).await? else {
            return Ok(());
        };
        if parent.status != AssignmentStatus::Waiting {
            return Ok(());
        }

        let wakes = self.store.sum_wakes_for_root(&parent.root_id).await?;
        parent.updated_at = Utc::now();
        if wakes >= i64::from(parent.budgets.wakes) {
            parent.status = AssignmentStatus::Failed;
            parent.outcome = Some(AssignmentOutcome::Failed {
                error: "wake budget exhausted".to_string(),
            });
            self.save_assignment(
                &parent,
                AssignmentEventKind::BudgetExhausted,
                Some("wake budget exhausted".to_string()),
            )
            .await?;
            self.store
                .update_task_status(
                    &parent.task_id,
                    &TaskStatus::Failed {
                        error: "wake budget exhausted".to_string(),
                    },
                )
                .await?;
            return Ok(());
        }

        parent.status = AssignmentStatus::Resume;
        parent.wakes += 1;
        self.save_assignment(
            &parent,
            AssignmentEventKind::Resumed,
            Some(format!("child '{}' settled", child.id)),
        )
        .await?;
        Ok(())
    }

    fn publish_task_created(&self, task: &Task) {
        self.bus
            .publish(Event::new(EventPayload::TaskCreated(task.clone())));
    }

    /// Persist an assignment transition and publish it to the event stream.
    ///
    /// Every transition goes through here so background cards never miss a
    /// status change.
    async fn save_assignment(
        &self,
        assignment: &Assignment,
        kind: AssignmentEventKind,
        detail: Option<String>,
    ) -> CowResult<()> {
        self.store
            .save_assignment(assignment, kind, detail.clone())
            .await?;
        self.publish_assignment_updated(assignment, kind, detail);
        Ok(())
    }

    fn publish_assignment_updated(
        &self,
        assignment: &Assignment,
        kind: AssignmentEventKind,
        detail: Option<String>,
    ) {
        self.bus
            .publish(Event::new(EventPayload::AssignmentUpdated {
                assignment_id: assignment.id.clone(),
                task_id: assignment.task_id.clone(),
                status: assignment.status,
                kind,
                detail,
            }));
    }

    fn publish_task_assigned(&self, task: &Task, assignment: &Assignment) {
        self.bus.publish(Event::new(EventPayload::TaskAssigned {
            task_id: task.id.clone(),
            agent_id: assignment.assignee.clone(),
        }));
    }
}
