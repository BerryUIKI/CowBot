use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use chrono::Utc;
use cowbot_agent::AgentInstance;
use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{
    Assignment, AssignmentId, AssignmentOrigin, AssignmentOutcome, AssignmentRequester,
    AssignmentStatus,
};
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::run::{Run, RunId, RunStatus, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::tool::{ToolCall, ToolCallRecorder, ToolGate, ToolResult};
use cowbot_core::usage::{LlmCallRecord, LlmCallUsage, LlmUsageRecorder};
use cowbot_llm::{ChatMessage, LlmProvider};
use cowbot_store::Store;
use cowbot_tools::ToolRegistry;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::approvals::{ApprovalService, ScopedApprovalGate};
use crate::assignments::AssignmentService;
use crate::permissions::ModelPermissionEvaluator;
use crate::session_decisions::SessionDecisions;

/// Instruction prepended to a Plan-mode run. Mutating tools are denied by the
/// approval policy; the agent is asked to finish with the plan itself.
const PLAN_MODE_INSTRUCTION: &str = "You are in Plan mode. Do not change files, run state-changing commands, or call tools that modify the workspace; those calls are denied. Investigate with read-only tools, then answer with a concrete, numbered plan that the user can approve. Your answer is stored with the task as its plan.";

/// A conversation turn whose model request is assembled before the assignment
/// is enqueued. The per-agent context branch receives the assistant and tool
/// messages the agent produces.
#[derive(Clone)]
struct PreparedConversationTurn {
    messages: Vec<ChatMessage>,
    channel_id: ChannelId,
    agent_id: AgentId,
}

/// Options for one scheduled crew run.
#[derive(Debug, Clone)]
pub struct CrewRunOptions {
    /// Approval policy applied to every root assignment.
    pub approval_level: ApprovalLevel,
    /// Channel the root tasks belong to, when they were created in one.
    pub channel_id: Option<ChannelId>,
    /// Maximum number of assignments that may execute at the same time.
    pub max_concurrency: usize,
    /// Let Auto mode ask the active model for permission decisions.
    pub use_model_permissions: bool,
    /// Capture this task's Plan-mode output as its plan. Crew runs that plan
    /// with one agent set the planning task; pipeline runs leave it unset so
    /// coder and reviewer outputs are not shown as plans.
    pub capture_plan_task: Option<TaskId>,
}

impl Default for CrewRunOptions {
    fn default() -> Self {
        Self {
            approval_level: ApprovalLevel::Ask,
            channel_id: None,
            max_concurrency: 4,
            use_model_permissions: false,
            capture_plan_task: None,
        }
    }
}

/// Reusable execution handles for one registered agent.
#[derive(Clone)]
struct AgentExecution {
    profile: AgentProfile,
    llm: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    model: String,
}

impl AgentExecution {
    /// Execute one assignment on a fresh agent instance.
    ///
    /// A new instance per call keeps assignment execution independent, which
    /// lets independent roots run concurrently.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        task: &Task,
        context: &[String],
        bus: &EventBus,
        gate: Arc<dyn ToolGate>,
        recorder: Arc<dyn ToolCallRecorder>,
        usage_recorder: Arc<dyn LlmUsageRecorder>,
        conversation: Option<Vec<ChatMessage>>,
        plan_mode: bool,
    ) -> CowResult<(String, Option<Vec<ChatMessage>>)> {
        let mut agent = AgentInstance::new(
            self.profile.clone(),
            self.llm.clone(),
            self.tools.clone(),
            &self.model,
        );
        agent.set_tool_gate(Some(gate));
        agent.set_tool_recorder(Some(recorder));
        agent.set_usage_recorder(Some(usage_recorder));
        if plan_mode {
            agent.set_mode_instruction(Some(PLAN_MODE_INSTRUCTION.to_string()));
        }

        match conversation {
            Some(messages) => {
                let collector = Arc::new(CollectingConversation::new());
                agent.set_conversation_recorder(Some(collector.clone()));
                let output = agent.execute_messages(task, messages, bus).await?;
                Ok((output, Some(collector.messages())))
            }
            None => {
                let output = agent.execute_task(task, context, bus).await?;
                Ok((output, None))
            }
        }
    }
}

/// Collects the messages an agent appends during a conversation turn.
///
/// The scheduler writes them to the context branch only after the turn
/// succeeds, so a failed turn never leaves an assistant tool call without its
/// tool results in the branch.
struct CollectingConversation {
    messages: StdMutex<Vec<ChatMessage>>,
}

impl CollectingConversation {
    fn new() -> Self {
        Self {
            messages: StdMutex::new(Vec::new()),
        }
    }

    fn messages(&self) -> Vec<ChatMessage> {
        self.messages
            .lock()
            .map(|messages| messages.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl cowbot_agent::ConversationRecorder for CollectingConversation {
    async fn record_message(&self, message: &ChatMessage) -> CowResult<()> {
        if let Ok(mut messages) = self.messages.lock() {
            messages.push(message.clone());
        }
        Ok(())
    }
}

/// One finished root: its root assignment, its task, and the execution result.
type SettledRoot = (AssignmentId, TaskId, CowResult<(TaskId, String)>);

/// Dispatches queued and resumed assignments until no work is left.
///
/// The scheduler is the single wake path described by ADR-0006: it picks up
/// queued roots and resumed parents, runs assignments concurrently across
/// roots, and serializes the assignments of one root.
#[derive(Clone)]
pub struct AssignmentScheduler {
    assignments: AssignmentService,
    approvals: ApprovalService,
    workspace_id: WorkspaceId,
    executions: HashMap<AgentId, AgentExecution>,
    max_concurrency: usize,
    /// When set, only assignments rooted in a task enqueued here are claimed.
    scope: Option<Arc<StdMutex<HashSet<AssignmentId>>>>,
    /// Ask the active model for Auto-mode permission decisions.
    use_model_permissions: bool,
    /// Human decisions remembered for this process.
    session_decisions: Option<Arc<SessionDecisions>>,
    /// Model requests prepared for conversation turns, keyed by task id.
    prepared: Arc<StdMutex<HashMap<TaskId, PreparedConversationTurn>>>,
    /// Tasks whose completed Plan-mode output becomes their stored plan.
    plan_tasks: Arc<StdMutex<HashSet<TaskId>>>,
}

enum Claim {
    Run(Assignment),
    Fail(Assignment, String),
}

enum DependencyState {
    Ready,
    Blocked,
    Failed(String),
}

impl AssignmentScheduler {
    pub fn new(
        store: Store,
        bus: EventBus,
        workspace_id: WorkspaceId,
        max_concurrency: usize,
    ) -> Self {
        let assignments = AssignmentService::new(store.clone(), bus.clone());
        let approvals = ApprovalService::new(store, bus, workspace_id.clone());
        Self {
            assignments,
            approvals,
            workspace_id,
            executions: HashMap::new(),
            max_concurrency,
            scope: None,
            use_model_permissions: false,
            session_decisions: None,
            prepared: Arc::new(StdMutex::new(HashMap::new())),
            plan_tasks: Arc::new(StdMutex::new(HashSet::new())),
        }
    }

    /// Enable model-decided Auto mode for assignments run by this scheduler.
    pub fn with_model_permissions(mut self, enabled: bool) -> Self {
        self.use_model_permissions = enabled;
        self
    }

    /// Share the process-lifetime session approval decisions.
    pub fn with_session_decisions(mut self, decisions: Option<Arc<SessionDecisions>>) -> Self {
        self.session_decisions = decisions;
        self
    }

    /// Limit this scheduler to the roots enqueued through it.
    ///
    /// Without a scope, [`Self::run_until_idle`] drains every queued assignment
    /// in the database, which lets two schedulers started from different
    /// callers race for the same work. A scoped scheduler only claims roots it
    /// enqueued, so independent background runs (such as channel turns) stay
    /// separate from each other and from visible crew runs.
    pub fn with_scope(mut self) -> Self {
        self.scope = Some(Arc::new(StdMutex::new(HashSet::new())));
        self
    }

    fn tracks(&self, root_id: &AssignmentId) -> bool {
        match &self.scope {
            None => true,
            Some(scope) => scope
                .lock()
                .map(|scope| scope.contains(root_id))
                .unwrap_or(false),
        }
    }

    pub fn register_agent(
        &mut self,
        profile: AgentProfile,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        model: impl Into<String>,
    ) {
        self.executions.insert(
            profile.id.clone(),
            AgentExecution {
                profile,
                llm,
                tools,
                model: model.into(),
            },
        );
    }

    /// Register the agent held by a crew instance.
    pub fn register_instance(&mut self, agent: &AgentInstance) {
        self.register_agent(
            agent.profile.clone(),
            agent.llm.clone(),
            agent.tools.clone(),
            agent.model.clone(),
        );
    }

    /// Persist the task when needed and enqueue its root assignment.
    pub async fn enqueue(
        &self,
        task: &Task,
        requester: AssignmentRequester,
        origin: AssignmentOrigin,
        options: &CrewRunOptions,
    ) -> CowResult<Assignment> {
        if options.capture_plan_task.as_ref() == Some(&task.id) {
            if let Ok(mut plan_tasks) = self.plan_tasks.lock() {
                plan_tasks.insert(task.id.clone());
            }
        }
        let assignment = self
            .enqueue_untracked(task, requester, origin, options)
            .await?;
        if let Some(scope) = &self.scope {
            if let Ok(mut scope) = scope.lock() {
                scope.insert(assignment.root_id.clone());
            }
        }
        Ok(assignment)
    }

    /// Enqueue a conversation turn that runs against a caller-assembled request.
    ///
    /// The assistant and tool messages produced by the turn are appended to the
    /// conversation's context branch when the assignment completes.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_conversation_turn(
        &self,
        task: &Task,
        requester: AssignmentRequester,
        origin: AssignmentOrigin,
        options: &CrewRunOptions,
        messages: Vec<ChatMessage>,
        channel_id: ChannelId,
        agent_id: AgentId,
    ) -> CowResult<Assignment> {
        if let Ok(mut prepared) = self.prepared.lock() {
            prepared.insert(
                task.id.clone(),
                PreparedConversationTurn {
                    messages,
                    channel_id,
                    agent_id,
                },
            );
        }

        match self.enqueue(task, requester, origin, options).await {
            Ok(assignment) => Ok(assignment),
            Err(error) => {
                if let Ok(mut prepared) = self.prepared.lock() {
                    prepared.remove(&task.id);
                }
                Err(error)
            }
        }
    }

    async fn enqueue_untracked(
        &self,
        task: &Task,
        requester: AssignmentRequester,
        origin: AssignmentOrigin,
        options: &CrewRunOptions,
    ) -> CowResult<Assignment> {
        let assignee = task
            .assigned_agent
            .clone()
            .ok_or_else(|| CowError::Task(format!("task '{}' has no assigned agent", task.id)))?;

        let existing = self
            .assignments
            .store()
            .list_assignments_for_task(&task.id)
            .await?
            .into_iter()
            .find(|assignment| assignment.parent_id.is_none() && !assignment.is_terminal());
        if let Some(existing) = existing {
            return Ok(existing);
        }

        if self.assignments.store().get_task(&task.id).await?.is_none() {
            self.assignments
                .store()
                .create_task(&self.workspace_id, options.channel_id.as_ref(), task)
                .await?;
        }

        self.assignments
            .enqueue_root(task, origin, requester, assignee, options.approval_level)
            .await
    }

    /// Dispatch assignments until no queued or resumed work remains.
    pub async fn run_until_idle(&self) -> CowResult<HashMap<TaskId, String>> {
        self.run_until_idle_inner(None).await
    }

    /// Dispatch until idle, reporting each settled task as it settles.
    ///
    /// [`Self::run_until_idle`] waits for every root before returning. This
    /// variant sends each task id to `settled` once its assignment is terminal
    /// (completed or failed), so a caller can post replies while slower
    /// participants are still running. The returned map and error behavior are
    /// unchanged.
    pub async fn run_until_idle_observed(
        &self,
        settled: tokio::sync::mpsc::UnboundedSender<TaskId>,
    ) -> CowResult<HashMap<TaskId, String>> {
        self.run_until_idle_inner(Some(settled)).await
    }

    async fn run_until_idle_inner(
        &self,
        settled: Option<tokio::sync::mpsc::UnboundedSender<TaskId>>,
    ) -> CowResult<HashMap<TaskId, String>> {
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency.max(1)));
        let mut running: JoinSet<SettledRoot> = JoinSet::new();
        let mut active_roots: HashSet<AssignmentId> = HashSet::new();
        let mut completed: HashMap<TaskId, String> = HashMap::new();
        let mut first_error: Option<CowError> = None;

        loop {
            let mut progressed = false;
            for claim in self.claimable(&active_roots).await? {
                match claim {
                    Claim::Run(assignment) => {
                        let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() else {
                            break;
                        };
                        let root_id = assignment.root_id.clone();
                        let task_id = assignment.task_id.clone();
                        active_roots.insert(root_id.clone());
                        let scheduler = self.clone();
                        running.spawn(async move {
                            let result = scheduler.execute_assignment(assignment).await;
                            drop(permit);
                            (root_id, task_id, result)
                        });
                        progressed = true;
                    }
                    Claim::Fail(assignment, reason) => {
                        if first_error.is_none() {
                            first_error = Some(CowError::Task(reason.clone()));
                        }
                        let task_id = assignment.task_id.clone();
                        match self.assignments.fail(&assignment.id, reason).await {
                            Ok(_) => {
                                progressed = true;
                                if let Some(sender) = &settled {
                                    let _ = sender.send(task_id);
                                }
                            }
                            Err(error) => {
                                if first_error.is_none() {
                                    first_error = Some(error);
                                }
                            }
                        }
                    }
                }
            }

            if running.is_empty() {
                if progressed {
                    // A failure may have woken a parent or failed a
                    // dependency; scan again before declaring the run idle.
                    continue;
                }
                break;
            }

            let Some(joined) = running.join_next().await else {
                break;
            };
            match joined {
                Ok((root_id, task_id, result)) => {
                    active_roots.remove(&root_id);
                    match result {
                        Ok((task_id, output)) => {
                            completed.insert(task_id, output);
                        }
                        Err(error) => {
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        }
                    }
                    if let Some(sender) = &settled {
                        let _ = sender.send(task_id);
                    }
                }
                Err(error) => {
                    return Err(CowError::Task(format!("scheduler task failed: {error}")));
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(completed),
        }
    }

    async fn claimable(&self, active_roots: &HashSet<AssignmentId>) -> CowResult<Vec<Claim>> {
        let mut claims = Vec::new();
        for status in [AssignmentStatus::Queued, AssignmentStatus::Resume] {
            for assignment in self
                .assignments
                .store()
                .list_assignments_by_status(status)
                .await?
            {
                if active_roots.contains(&assignment.root_id) {
                    continue;
                }
                if !self.tracks(&assignment.root_id) {
                    continue;
                }

                if !self.executions.contains_key(&assignment.assignee) {
                    let reason =
                        format!("no executor registered for agent '{}'", assignment.assignee);
                    claims.push(Claim::Fail(assignment, reason));
                    continue;
                }

                let Some(task) = self
                    .assignments
                    .store()
                    .get_task(&assignment.task_id)
                    .await?
                else {
                    let reason = format!("task '{}' was not found", assignment.task_id);
                    claims.push(Claim::Fail(assignment, reason));
                    continue;
                };

                let dependency = self.dependency_state(&task).await?;
                match dependency {
                    DependencyState::Blocked => continue,
                    DependencyState::Failed(reason) => {
                        claims.push(Claim::Fail(assignment, reason));
                        continue;
                    }
                    DependencyState::Ready => {}
                }

                if status == AssignmentStatus::Resume {
                    let children = self
                        .assignments
                        .store()
                        .list_child_assignments(&assignment.id)
                        .await?;
                    if children.iter().any(|child| !child.is_terminal()) {
                        continue;
                    }
                }

                claims.push(Claim::Run(assignment));
            }
        }
        Ok(claims)
    }

    async fn dependency_state(&self, task: &Task) -> CowResult<DependencyState> {
        for dependency in &task.context_tasks {
            let assignments = self
                .assignments
                .store()
                .list_assignments_for_task(dependency)
                .await?;
            if assignments.is_empty() {
                if self
                    .assignments
                    .store()
                    .get_task(dependency)
                    .await?
                    .is_none()
                {
                    continue;
                }
                return Ok(DependencyState::Blocked);
            }
            if assignments
                .iter()
                .any(|assignment| assignment.status == AssignmentStatus::Completed)
            {
                continue;
            }
            if let Some(settled) = assignments
                .iter()
                .find(|assignment| assignment.is_terminal())
            {
                let reason = match &settled.outcome {
                    Some(AssignmentOutcome::Failed { error }) => {
                        format!("dependency task '{dependency}' failed: {error}")
                    }
                    Some(AssignmentOutcome::Cancelled { reason }) => {
                        format!("dependency task '{dependency}' was cancelled: {reason}")
                    }
                    _ => format!("dependency task '{dependency}' did not complete"),
                };
                return Ok(DependencyState::Failed(reason));
            }
            return Ok(DependencyState::Blocked);
        }
        Ok(DependencyState::Ready)
    }

    async fn execute_assignment(&self, assignment: Assignment) -> CowResult<(TaskId, String)> {
        let Some(task) = self
            .assignments
            .store()
            .get_task(&assignment.task_id)
            .await?
        else {
            let reason = format!("task '{}' was not found", assignment.task_id);
            self.assignments
                .fail(&assignment.id, reason.clone())
                .await?;
            return Err(CowError::Task(reason));
        };
        let Some(executor) = self.executions.get(&assignment.assignee).cloned() else {
            let reason = format!("no executor registered for agent '{}'", assignment.assignee);
            self.assignments
                .fail(&assignment.id, reason.clone())
                .await?;
            return Err(CowError::Agent(reason));
        };
        let context = self.context_for(&assignment, &task).await?;
        let prepared = self
            .prepared
            .lock()
            .ok()
            .and_then(|mut prepared| prepared.remove(&task.id));

        self.assignments.start(&assignment.id).await?;

        let run = Run::new(task.id.clone(), assignment.assignee.clone());
        self.assignments.store().create_run(&run).await?;

        let mut approvals = self
            .approvals
            .clone()
            .with_session_decisions(self.session_decisions.clone());
        if self.use_model_permissions {
            approvals = approvals.with_evaluator(Some(Arc::new(ModelPermissionEvaluator::new(
                executor.llm.clone(),
                executor.model.clone(),
            ))));
        }
        let gate = Arc::new(ScopedApprovalGate::new(approvals, assignment.id.clone()));
        let recorder = Arc::new(RunToolCallRecorder {
            store: self.assignments.store().clone(),
            run_id: run.id.clone(),
        });
        let usage_recorder = Arc::new(RunUsageRecorder {
            store: self.assignments.store().clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            agent_id: assignment.assignee.clone(),
            channel_id: task.channel_id.clone(),
        });

        let messages = prepared.as_ref().map(|turn| turn.messages.clone());
        let plan_mode = assignment.approval_level == ApprovalLevel::PlanOnly;
        match executor
            .execute(
                &task,
                &context,
                self.assignments.bus(),
                gate,
                recorder,
                usage_recorder,
                messages,
                plan_mode,
            )
            .await
        {
            Ok((output, recorded)) => {
                if let (Some(turn), Some(messages)) = (&prepared, recorded) {
                    let entries =
                        context_entries_from_messages(&turn.channel_id, &turn.agent_id, &messages);
                    if let Err(error) = self
                        .assignments
                        .store()
                        .append_context_entries(&entries)
                        .await
                    {
                        self.assignments
                            .store()
                            .update_run_status(&run.id, RunStatus::Failed, Some(Utc::now()))
                            .await?;
                        self.assignments
                            .fail(&assignment.id, error.to_string())
                            .await?;
                        return Err(error);
                    }
                }
                self.assignments
                    .store()
                    .update_run_status(&run.id, RunStatus::Completed, Some(Utc::now()))
                    .await?;
                self.assignments.complete(&assignment.id, &output).await?;
                if self
                    .plan_tasks
                    .lock()
                    .map(|tasks| tasks.contains(&task.id))
                    .unwrap_or(false)
                    && assignment.approval_level == ApprovalLevel::PlanOnly
                    && !output.trim().is_empty()
                {
                    self.assignments
                        .store()
                        .set_task_plan(&task.id, &output)
                        .await?;
                    if let Some(updated) = self.assignments.store().get_task(&task.id).await? {
                        self.assignments
                            .bus()
                            .publish(Event::new(EventPayload::TaskUpdated(updated)));
                    }
                }
                Ok((task.id.clone(), output))
            }
            Err(error) => {
                self.assignments
                    .store()
                    .update_run_status(&run.id, RunStatus::Failed, Some(Utc::now()))
                    .await?;
                self.assignments
                    .fail(&assignment.id, error.to_string())
                    .await?;
                Err(error)
            }
        }
    }

    async fn context_for(&self, assignment: &Assignment, task: &Task) -> CowResult<Vec<String>> {
        let mut context = self.task_context_outputs(task).await?;
        if assignment.status == AssignmentStatus::Resume {
            for child in self
                .assignments
                .store()
                .list_child_assignments(&assignment.id)
                .await?
            {
                if let Some(AssignmentOutcome::Completed { output }) = child.outcome {
                    context.push(output);
                }
            }
        }
        // An approved plan is the instruction for the follow-up Manual run.
        if assignment.approval_level != ApprovalLevel::PlanOnly && task.plan_approved {
            if let Some(plan) = task.plan.as_deref().filter(|plan| !plan.trim().is_empty()) {
                context.push(format!("Approved plan to implement:\n{plan}"));
            }
        }
        Ok(context)
    }

    async fn task_context_outputs(&self, task: &Task) -> CowResult<Vec<String>> {
        let mut outputs = Vec::new();
        for dependency in &task.context_tasks {
            let assignments = self
                .assignments
                .store()
                .list_assignments_for_task(dependency)
                .await?;
            if let Some(completed) = assignments
                .iter()
                .find(|assignment| assignment.status == AssignmentStatus::Completed)
            {
                if let Some(AssignmentOutcome::Completed { output }) = &completed.outcome {
                    outputs.push(output.clone());
                }
            }
        }
        Ok(outputs)
    }
}

/// Persists tool calls under the run that produced them.
struct RunToolCallRecorder {
    store: Store,
    run_id: RunId,
}

#[async_trait]
impl ToolCallRecorder for RunToolCallRecorder {
    async fn record_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        approved_by: Option<&str>,
    ) -> CowResult<()> {
        let mut record = ToolCallRecord::new(
            self.run_id.clone(),
            call.name.clone(),
            call.arguments.clone(),
        );
        record.id = call.id.clone();
        record.result = Some(serde_json::to_value(result)?);
        record.approved_by = approved_by.map(str::to_string);
        self.store.record_tool_call(&record).await
    }
}

/// Persists LLM usage under the run that produced it.
struct RunUsageRecorder {
    store: Store,
    run_id: RunId,
    task_id: TaskId,
    agent_id: AgentId,
    channel_id: Option<ChannelId>,
}

#[async_trait]
impl LlmUsageRecorder for RunUsageRecorder {
    async fn record_llm_call(&self, call: &LlmCallUsage) -> CowResult<()> {
        let mut record = LlmCallRecord::new(call.call_kind, call.model.clone());
        record.run_id = Some(self.run_id.clone());
        record.task_id = Some(self.task_id.clone());
        record.channel_id = self.channel_id.clone();
        record.agent_id = Some(self.agent_id.clone());
        record.usage = call.usage.clone();
        record.raw_usage = call.raw_usage.clone();
        self.store.record_llm_call(&record).await
    }
}

/// Convert the messages an agent appended into context branch entries.
fn context_entries_from_messages(
    channel_id: &ChannelId,
    agent_id: &AgentId,
    messages: &[ChatMessage],
) -> Vec<cowbot_core::conversation::ContextEntry> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                cowbot_llm::ChatRole::System => cowbot_core::conversation::ContextRole::System,
                cowbot_llm::ChatRole::User => cowbot_core::conversation::ContextRole::User,
                cowbot_llm::ChatRole::Assistant => {
                    cowbot_core::conversation::ContextRole::Assistant
                }
                cowbot_llm::ChatRole::Tool => cowbot_core::conversation::ContextRole::Tool,
            };
            let mut entry = cowbot_core::conversation::ContextEntry::new(
                channel_id.clone(),
                agent_id.clone(),
                role,
                message.content.clone(),
            );
            entry.tool_calls = message.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|call| cowbot_core::conversation::ContextToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })
                    .collect()
            });
            entry.tool_call_id = message.tool_call_id.clone();
            entry.recount();
            entry
        })
        .collect()
}
