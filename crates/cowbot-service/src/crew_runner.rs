use std::collections::HashMap;
use std::sync::Arc;

use cowbot_agent::Crew;
use cowbot_bus::EventBus;
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{AssignmentOrigin, AssignmentRequester};
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::task::TaskId;
use cowbot_store::Store;

use crate::assignments::AssignmentService;
use crate::scheduler::{AssignmentScheduler, CrewRunOptions};
use crate::session_decisions::SessionDecisions;

/// Executes a crew through the assignment scheduler.
#[derive(Clone)]
pub struct CrewRunner {
    assignments: AssignmentService,
    session_decisions: Option<Arc<SessionDecisions>>,
}

impl CrewRunner {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self {
            assignments: AssignmentService::new(store, bus),
            session_decisions: None,
        }
    }

    pub fn with_session_decisions(mut self, decisions: Arc<SessionDecisions>) -> Self {
        self.session_decisions = Some(decisions);
        self
    }

    pub fn assignments(&self) -> &AssignmentService {
        &self.assignments
    }

    /// Run every task in a crew with the default options.
    pub async fn run(
        &self,
        crew: &mut Crew,
        workspace_id: &WorkspaceId,
        requester: AssignmentRequester,
        origin: AssignmentOrigin,
        approval_level: ApprovalLevel,
    ) -> CowResult<HashMap<TaskId, String>> {
        self.run_with_options(
            crew,
            workspace_id,
            requester,
            origin,
            CrewRunOptions {
                approval_level,
                ..CrewRunOptions::default()
            },
        )
        .await
    }

    /// Run every task through the scheduler until the crew is idle.
    pub async fn run_with_options(
        &self,
        crew: &mut Crew,
        workspace_id: &WorkspaceId,
        requester: AssignmentRequester,
        origin: AssignmentOrigin,
        mut options: CrewRunOptions,
    ) -> CowResult<HashMap<TaskId, String>> {
        // A pipeline runs one planning artifact: capture the first task's
        // output rather than every root's.
        if options.capture_plan_task.is_none() && options.approval_level == ApprovalLevel::PlanOnly
        {
            if let Some(first) = crew.tasks.first() {
                options.capture_plan_task = Some(first.id.clone());
            }
        }
        let mut scheduler = AssignmentScheduler::new(
            self.assignments.store().clone(),
            self.assignments.bus().clone(),
            workspace_id.clone(),
            options.max_concurrency,
        )
        .with_model_permissions(options.use_model_permissions)
        .with_session_decisions(self.session_decisions.clone());
        for agent in crew.agents.values() {
            scheduler.register_instance(agent);
        }

        let tasks = crew.tasks.clone();
        for task in &tasks {
            scheduler
                .enqueue(task, requester.clone(), origin, &options)
                .await?;
        }

        scheduler.run_until_idle().await
    }
}
