//! Read-only previews of the next model request (ADR-0036).
//!
//! The context inspector rebuilds the exact request a conversation participant
//! or a task run would send: named system-prompt slots, messages in order, the
//! memory block with its recall trace, and the tool schemas. Nothing here
//! writes: no context entry, no recall statistic, no LLM call.

use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::assignment::{Assignment, AssignmentOutcome, AssignmentStatus};
use cowbot_core::channel::ChannelId;
use cowbot_core::conversation::{
    context_entry_tokens, project_context, ChannelKind, ChannelParticipant, ContextEntry,
    ContextProjection, ContextRole, DEFAULT_COMPACT_THRESHOLD, MIN_GROUP_PARTICIPANTS,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::prompt::{
    PromptKind, PromptMemory, PromptMessage, PromptPlan, PromptRole, PromptSlot, PromptSlotName,
    PromptTool,
};
use cowbot_core::run::{Run, RunId};
use cowbot_core::task::{task_execution_prompt, Task, TaskId, TaskSource};
use cowbot_core::tool::ToolDefinition;
use cowbot_core::usage::TokenCounter;
use cowbot_llm::token_counter_for_model;
use cowbot_store::Store;

use crate::conversations::{
    conversation_system_prompt, group_system_prompt, resolve_default_responder,
};
use crate::memory::MemoryService;
use crate::scheduler::PLAN_MODE_INSTRUCTION;

/// What one group or conversation participant would send next.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConversationPreview {
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub agent_name: String,
    pub plan: PromptPlan,
}

/// What a task run sent on its first iteration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRunPreview {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub task_title: String,
    pub agent_id: AgentId,
    pub agent_name: String,
    pub plan: PromptPlan,
}

/// Builds conversation and task-run previews without writing anything.
#[derive(Clone)]
pub struct PreviewService {
    store: Store,
    workspace_id: WorkspaceId,
    memory: MemoryService,
}

impl PreviewService {
    pub fn new(store: Store, workspace_id: WorkspaceId) -> Self {
        let memory = MemoryService::new(store.clone(), workspace_id.clone());
        Self {
            store,
            workspace_id,
            memory,
        }
    }

    /// Rebuild the request one conversation participant would send.
    ///
    /// `draft` is the composer's pending message: the reply path appends the
    /// user message to the branch before assembling, so the preview does the
    /// same in memory. `platform_context` is the desktop's extra environment
    /// text, which group sends include in the platform slot. `plan_mode` adds
    /// the same Plan-mode instruction the agent runtime inserts for a turn.
    pub async fn conversation(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        draft: Option<&str>,
        platform_context: Option<&str>,
        plan_mode: bool,
        tools: &[ToolDefinition],
    ) -> CowResult<ConversationPreview> {
        let channel = self.store.get_channel(channel_id).await?.ok_or_else(|| {
            CowError::Config(format!("conversation '{}' was not found", channel_id.0))
        })?;
        if channel.kind != ChannelKind::Conversation {
            return Err(CowError::Config(format!(
                "'{}' is not a conversation with a context branch",
                channel.name
            )));
        }
        let settings = self
            .store
            .get_conversation_settings(channel_id)
            .await?
            .ok_or_else(|| {
                CowError::Config(format!("conversation '{}' has no settings", channel.name))
            })?;
        let agents = self.store.list_agents(&self.workspace_id).await?;
        let (participant_count, participants) = self.load_participants(channel_id, &agents).await?;
        let agent = participants
            .iter()
            .find(|participant| &participant.id == agent_id)
            .cloned()
            .ok_or_else(|| {
                CowError::Config(format!(
                    "agent '{}' is not a participant of '{}'",
                    agent_id.0, channel.name
                ))
            })?;
        let counter = token_counter_for_model(&settings.model);
        // The reply path dispatches on persisted participant rows, not on the
        // profiles that still exist, so a group keeps its group prompt.
        let group = participant_count >= MIN_GROUP_PARTICIPANTS;
        let default_name = resolve_default_responder(&settings, &participants)
            .0
            .map(|agent| agent.name.clone());
        let prompt = if group {
            group_system_prompt(
                counter.as_ref(),
                &agent,
                &participants,
                default_name.as_deref(),
                platform_context,
                &agents,
            )
        } else {
            conversation_system_prompt(counter.as_ref(), &agent, &agents)
        };
        let mut system_tokens = counter.count_message(&prompt.text);
        let mut slots = prompt.slots;
        // The agent runtime inserts the Plan-mode instruction after the system
        // prompt and before the memory block, so the preview does the same.
        let mode_instruction = plan_mode.then(|| {
            let tokens = counter.count_message(PLAN_MODE_INSTRUCTION);
            system_tokens = system_tokens.saturating_add(tokens);
            slots.push(PromptSlot {
                name: PromptSlotName::ModeInstruction,
                content: PLAN_MODE_INSTRUCTION.to_string(),
                tokens: counter.count(PLAN_MODE_INSTRUCTION),
            });
            PromptMessage {
                role: PromptRole::System,
                content: PLAN_MODE_INSTRUCTION.to_string(),
                tokens,
                entry_id: None,
                latest: false,
                tool_calls: Vec::new(),
                tool_call_id: None,
            }
        });

        let draft = draft.map(str::trim).filter(|draft| !draft.is_empty());
        let entries = self
            .store
            .list_context_entries(channel_id, agent_id)
            .await?;
        let latest_entry_id = if draft.is_none() {
            latest_user_entry(&entries).map(|entry| entry.id.clone())
        } else {
            None
        };

        let recall = self
            .memory
            .preview_recall_trace(Some(channel_id), agent_id, draft.unwrap_or(""))
            .await?;
        let memory = recall.block.as_ref().map(|block| PromptMemory {
            block: block.clone(),
            tokens: counter.count_message(block),
        });
        let (tool_views, tool_tokens) = PromptTool::plan(counter.as_ref(), tools);

        // The reply path appends the user message before assembling, so the
        // projection counts the draft as the latest turn.
        let mut projection_entries = entries.clone();
        if let Some(draft) = draft {
            let mut entry = ContextEntry::new(
                channel_id.clone(),
                agent_id.clone(),
                ContextRole::User,
                draft,
            );
            entry.recount();
            projection_entries.push(entry);
        }
        let projection = project_context(
            counter.as_ref(),
            system_tokens,
            tool_tokens,
            &projection_entries,
        );

        let mut messages = vec![PromptMessage {
            role: PromptRole::System,
            content: prompt.text.clone(),
            tokens: counter.count_message(&prompt.text),
            entry_id: None,
            latest: false,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }];
        if let Some(mode_instruction) = mode_instruction {
            messages.push(mode_instruction);
        }
        if let Some(memory) = &memory {
            messages.push(PromptMessage {
                role: PromptRole::System,
                content: memory.block.clone(),
                tokens: memory.tokens,
                entry_id: None,
                latest: false,
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
        }
        messages.extend(history_messages(
            counter.as_ref(),
            &entries,
            latest_entry_id.as_deref(),
        ));
        if let Some(draft) = draft {
            messages.push(PromptMessage {
                role: PromptRole::User,
                content: draft.to_string(),
                tokens: counter.count_message(draft),
                entry_id: None,
                latest: true,
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
        }

        Ok(ConversationPreview {
            channel_id: channel.id.clone(),
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            plan: PromptPlan {
                kind: if group {
                    PromptKind::Group
                } else {
                    PromptKind::Conversation
                },
                model: settings.model,
                provider_id: settings.provider_id,
                slots,
                messages,
                memory,
                recall,
                tools: tool_views,
                projection,
                auto_compact: settings.auto_compact,
                compact_threshold: settings.compact_threshold,
            },
        })
    }

    /// Rebuild the request that started one task run.
    ///
    /// Prompts are not persisted yet (issue #178), so the plan is reconstructed
    /// from the task, the agent profile, the recall preview, and the persisted
    /// prerequisite outputs. The Plan-mode instruction is included when the
    /// assignment nearest the run's start time ran in Plan mode.
    pub async fn task_run(
        &self,
        run_id: &RunId,
        model: &str,
        tools: &[ToolDefinition],
    ) -> CowResult<Option<TaskRunPreview>> {
        let Some(run) = self.store.get_run(run_id).await? else {
            return Ok(None);
        };
        let Some(task) = self.store.get_task(&run.task_id).await? else {
            return Err(CowError::Config(format!(
                "task '{}' for run '{}' was not found",
                run.task_id.0, run.id.0
            )));
        };
        if task.source != TaskSource::User {
            return Err(CowError::Config(format!(
                "run '{}' executed a {} turn, not a user task; inspect the conversation's \
                 context meter instead",
                run.id.0,
                task.source.as_str().replace('_', " ")
            )));
        }
        let agents = self.store.list_agents(&self.workspace_id).await?;
        // Standard crew roles are transient, so the run's own snapshot is the
        // authoritative profile; older runs fall back to the stored agent.
        let agent = run
            .agent_profile
            .clone()
            .or_else(|| {
                agents
                    .iter()
                    .find(|agent| agent.id == run.agent_id)
                    .cloned()
            })
            .ok_or_else(|| CowError::Config(format!("agent '{}' was not found", run.agent_id.0)))?;
        // The model the run actually used is recorded with its first LLM call.
        let model = self
            .store
            .list_llm_calls_for_run(run_id)
            .await?
            .first()
            .map(|call| call.model.clone())
            .unwrap_or_else(|| model.to_string());
        let assignments = self.store.list_assignments_for_task(&task.id).await?;
        let plan_mode = plan_mode_for_run(&run, &assignments);
        let counter = token_counter_for_model(&model);

        let recall = self
            .memory
            .preview_recall_trace(task.channel_id.as_ref(), &run.agent_id, &task.description)
            .await?;
        let memory = recall.block.as_ref().map(|block| PromptMemory {
            block: block.clone(),
            tokens: counter.count_message(block),
        });
        let (tool_views, tool_tokens) = PromptTool::plan(counter.as_ref(), tools);

        let mut slots = Vec::new();
        let mut messages = Vec::new();
        let agent_prompt = agent.build_system_prompt();
        let agent_prompt_tokens = counter.count_message(&agent_prompt);
        slots.push(PromptSlot {
            name: PromptSlotName::Agent,
            tokens: counter.count(&agent_prompt),
            content: agent_prompt.clone(),
        });
        messages.push(PromptMessage {
            role: PromptRole::System,
            content: agent_prompt,
            tokens: agent_prompt_tokens,
            entry_id: None,
            latest: false,
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
        let mut system_tokens = agent_prompt_tokens;
        if plan_mode {
            slots.push(PromptSlot {
                name: PromptSlotName::ModeInstruction,
                content: PLAN_MODE_INSTRUCTION.to_string(),
                tokens: counter.count(PLAN_MODE_INSTRUCTION),
            });
            let tokens = counter.count_message(PLAN_MODE_INSTRUCTION);
            system_tokens = system_tokens.saturating_add(tokens);
            messages.push(PromptMessage {
                role: PromptRole::System,
                content: PLAN_MODE_INSTRUCTION.to_string(),
                tokens,
                entry_id: None,
                latest: false,
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
        }
        if let Some(memory) = &memory {
            messages.push(PromptMessage {
                role: PromptRole::System,
                content: memory.block.clone(),
                tokens: memory.tokens,
                entry_id: None,
                latest: false,
                tool_calls: Vec::new(),
                tool_call_id: None,
            });
        }
        let context_outputs = self.context_outputs(&task, plan_mode).await?;
        let prompt = task_execution_prompt(&task, &context_outputs);
        let prompt_tokens = counter.count_message(&prompt);
        messages.push(PromptMessage {
            role: PromptRole::User,
            content: prompt,
            tokens: prompt_tokens,
            entry_id: None,
            latest: true,
            tool_calls: Vec::new(),
            tool_call_id: None,
        });

        let projection = ContextProjection {
            system_tokens,
            tool_tokens,
            history_tokens: prompt_tokens,
            latest_tokens: 0,
            active_entries: 0,
            excluded_entries: 0,
            token_source: counter.source(),
            token_counter: Some(counter.label().to_string()),
        };

        Ok(Some(TaskRunPreview {
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            plan: PromptPlan {
                kind: PromptKind::TaskRun,
                model: model.to_string(),
                provider_id: None,
                slots,
                messages,
                memory,
                recall,
                tools: tool_views,
                projection,
                auto_compact: false,
                compact_threshold: DEFAULT_COMPACT_THRESHOLD,
            },
        }))
    }

    /// Participant profiles in roster order and the persisted row count.
    ///
    /// The count drives the single-versus-group prompt choice, matching the
    /// reply path, while the profiles skip agents that were deleted.
    async fn load_participants(
        &self,
        channel_id: &ChannelId,
        agents: &[AgentProfile],
    ) -> CowResult<(usize, Vec<AgentProfile>)> {
        let participants: Vec<ChannelParticipant> =
            self.store.list_channel_participants(channel_id).await?;
        let profiles = participants
            .iter()
            .filter_map(|participant| {
                agents
                    .iter()
                    .find(|agent| agent.id == participant.agent_id)
                    .cloned()
            })
            .collect();
        Ok((participants.len(), profiles))
    }

    /// Prerequisite outputs the run received, mirroring the scheduler.
    async fn context_outputs(&self, task: &Task, plan_mode: bool) -> CowResult<Vec<String>> {
        let mut outputs = Vec::new();
        for dependency in &task.context_tasks {
            let assignments = self.store.list_assignments_for_task(dependency).await?;
            if let Some(completed) = assignments
                .iter()
                .find(|assignment| assignment.status == AssignmentStatus::Completed)
            {
                if let Some(AssignmentOutcome::Completed { output }) = &completed.outcome {
                    outputs.push(output.clone());
                }
            }
        }
        if !plan_mode && task.plan_approved {
            if let Some(plan) = task.plan.as_deref().filter(|plan| !plan.trim().is_empty()) {
                outputs.push(format!("Approved plan to implement:\n{plan}"));
            }
        }
        Ok(outputs)
    }
}

/// The latest user turn a reply answers, matching the projection's rule.
fn latest_user_entry(entries: &[ContextEntry]) -> Option<&ContextEntry> {
    entries.iter().rev().find(|entry| {
        !entry.excluded && entry.role == ContextRole::User && entry.author_agent_id.is_none()
    })
}

/// Messages for the active branch, in order, with the latest turn marked.
fn history_messages(
    counter: &dyn TokenCounter,
    entries: &[ContextEntry],
    latest_entry_id: Option<&str>,
) -> Vec<PromptMessage> {
    entries
        .iter()
        .filter(|entry| !entry.excluded)
        .map(|entry| PromptMessage {
            role: prompt_role(entry.role),
            content: entry.content.clone(),
            tokens: context_entry_tokens(counter, entry),
            entry_id: Some(entry.id.clone()),
            latest: latest_entry_id == Some(entry.id.as_str()),
            tool_calls: entry
                .tool_calls
                .as_ref()
                .map(|calls| calls.iter().map(|call| call.name.clone()).collect())
                .unwrap_or_default(),
            tool_call_id: entry.tool_call_id.clone(),
        })
        .collect()
}

fn prompt_role(role: ContextRole) -> PromptRole {
    match role {
        ContextRole::System => PromptRole::System,
        ContextRole::User => PromptRole::User,
        ContextRole::Assistant => PromptRole::Assistant,
        ContextRole::Tool => PromptRole::Tool,
    }
}

/// Whether the run executed with the Plan-mode instruction.
///
/// Runs do not persist the assignment that produced them. The assignment that
/// produced the run already existed when it started, and `updated_at` moves at
/// completion, so the best match is the assignment for the same task and agent
/// created latest before the run started. When clock skew puts every
/// assignment after the start, the earliest one is used.
fn plan_mode_for_run(run: &Run, assignments: &[Assignment]) -> bool {
    assignments
        .iter()
        .filter(|assignment| {
            assignment.task_id == run.task_id && assignment.assignee == run.agent_id
        })
        .filter(|assignment| assignment.created_at <= run.started_at)
        .max_by_key(|assignment| assignment.created_at)
        .or_else(|| {
            assignments
                .iter()
                .filter(|assignment| {
                    assignment.task_id == run.task_id && assignment.assignee == run.agent_id
                })
                .min_by_key(|assignment| assignment.created_at)
        })
        .map(|assignment| assignment.approval_level == ApprovalLevel::PlanOnly)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: ContextRole, content: &str) -> ContextEntry {
        let mut entry = ContextEntry::new(
            ChannelId::new("channel"),
            AgentId::new("agent"),
            role,
            content,
        );
        entry.recount();
        entry
    }

    #[test]
    fn history_messages_mark_the_latest_user_turn_and_skip_excluded_entries() {
        let mut excluded = entry(ContextRole::User, "forgotten");
        excluded.excluded = true;
        let entries = vec![
            entry(ContextRole::User, "first"),
            excluded,
            entry(ContextRole::Assistant, "answer"),
            entry(ContextRole::User, "latest"),
        ];
        let latest = latest_user_entry(&entries).map(|entry| entry.id.clone());
        let messages = history_messages(
            &cowbot_core::usage::ESTIMATE_COUNTER,
            &entries,
            latest.as_deref(),
        );
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "first");
        assert!(!messages[0].latest);
        assert!(messages[2].latest);
        assert_eq!(messages[2].content, "latest");
        assert!(messages[2].entry_id.is_some());
        assert!(!messages
            .iter()
            .any(|message| message.content == "forgotten"));
    }

    #[test]
    fn plan_mode_matches_the_assignment_created_latest_before_the_run() {
        use cowbot_core::assignment::{AssignmentId, AssignmentMode, AssignmentOrigin, ReplyMode};

        let run = Run::new(TaskId::new("task"), AgentId::new("agent"));
        let started = run.started_at;
        let mut planner = Assignment::new_root(
            TaskId::new("task"),
            AssignmentOrigin::User,
            cowbot_core::assignment::AssignmentRequester::System,
            AgentId::new("agent"),
            AssignmentMode::Work,
            ReplyMode::None,
            ApprovalLevel::PlanOnly,
        );
        planner.created_at = started - chrono::Duration::minutes(5);
        let mut worker = planner.clone();
        worker.id = AssignmentId::new("worker");
        worker.approval_level = ApprovalLevel::Ask;
        // Created after this run started, so it belongs to a later run.
        worker.created_at = started + chrono::Duration::seconds(1);
        // A completion-time update must not reorder the assignments.
        worker.updated_at = started + chrono::Duration::minutes(30);

        assert!(plan_mode_for_run(&run, std::slice::from_ref(&planner)));
        assert!(plan_mode_for_run(&run, &[planner.clone(), worker.clone()]));
        assert!(!plan_mode_for_run(&run, &[]));

        // A later run sees the worker assignment as the latest one.
        let mut later = Run::new(TaskId::new("task"), AgentId::new("agent"));
        later.started_at = started + chrono::Duration::minutes(2);
        assert!(!plan_mode_for_run(&later, &[planner, worker]));
    }
}
