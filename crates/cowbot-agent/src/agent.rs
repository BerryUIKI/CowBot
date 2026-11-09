use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::task::Task;
use cowbot_core::tool::{ToolAuthorization, ToolCall, ToolCallRecorder, ToolGate};
use cowbot_core::usage::{LlmCallKind, LlmCallUsage, LlmUsageRecorder};
use cowbot_llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use cowbot_tools::ToolRegistry;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// An active agent runtime instance backed by an LLM provider and tool registry.
pub struct AgentInstance {
    pub profile: AgentProfile,
    pub llm: Arc<dyn LlmProvider>,
    pub tools: ToolRegistry,
    pub model: String,
    tool_gate: Option<Arc<dyn ToolGate>>,
    tool_recorder: Option<Arc<dyn ToolCallRecorder>>,
    usage_recorder: Option<Arc<dyn LlmUsageRecorder>>,
}

impl AgentInstance {
    pub fn new(
        profile: AgentProfile,
        llm: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        model: impl Into<String>,
    ) -> Self {
        Self {
            profile,
            llm,
            tools,
            model: model.into(),
            tool_gate: None,
            tool_recorder: None,
            usage_recorder: None,
        }
    }

    pub fn set_tool_gate(&mut self, gate: Option<Arc<dyn ToolGate>>) {
        self.tool_gate = gate;
    }

    pub fn clear_tool_gate(&mut self) {
        self.tool_gate = None;
    }

    pub fn set_tool_recorder(&mut self, recorder: Option<Arc<dyn ToolCallRecorder>>) {
        self.tool_recorder = recorder;
    }

    pub fn clear_tool_recorder(&mut self) {
        self.tool_recorder = None;
    }

    pub fn set_usage_recorder(&mut self, recorder: Option<Arc<dyn LlmUsageRecorder>>) {
        self.usage_recorder = recorder;
    }

    pub fn clear_usage_recorder(&mut self) {
        self.usage_recorder = None;
    }

    pub fn id(&self) -> &AgentId {
        &self.profile.id
    }

    pub fn name(&self) -> &str {
        &self.profile.name
    }

    /// Executes an assigned task using the agent loop (Thought -> Action -> Observation).
    pub async fn execute_task(
        &mut self,
        task: &Task,
        context_outputs: &[String],
        bus: &EventBus,
    ) -> CowResult<String> {
        info!(
            agent = %self.profile.name,
            task_id = %task.id,
            "Agent starting task execution"
        );

        let system_prompt = self.profile.build_system_prompt();
        let mut messages = vec![ChatMessage::system(system_prompt)];

        // Inject prerequisite context if any exists
        let mut prompt = format!(
            "Task Description:\n{}\n\nExpected Output Format:\n{}",
            task.description, task.expected_output
        );

        if !context_outputs.is_empty() {
            prompt.push_str("\n\nPrerequisite Context from Prior Tasks:\n");
            for (idx, ctx) in context_outputs.iter().enumerate() {
                prompt.push_str(&format!("{}. {}\n", idx + 1, ctx));
            }
        }

        messages.push(ChatMessage::user(prompt));

        let tool_definitions = self.tools.definitions_for(&self.profile.tools);

        let mut iterations = 0;
        let max_iters = self.profile.max_iterations;

        while iterations < max_iters {
            iterations += 1;
            debug!(
                agent = %self.profile.name,
                iteration = iterations,
                "Calling LLM completion"
            );

            let req = ChatCompletionRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tool_definitions.clone(),
                temperature: Some(0.2),
            };

            let response = self.llm.chat_complete(&req).await?;

            if let Some(recorder) = &self.usage_recorder {
                let call = LlmCallUsage {
                    model: self.model.clone(),
                    call_kind: LlmCallKind::Agent,
                    usage: response.usage.clone(),
                    raw_usage: response.raw_usage.clone(),
                };
                if let Err(error) = recorder.record_llm_call(&call).await {
                    warn!(
                        agent = %self.profile.name,
                        error = %error,
                        "Failed to record LLM usage"
                    );
                }
            }

            // If the model produced thought/text, publish to event bus
            if let Some(content) = &response.content {
                if !content.trim().is_empty() {
                    bus.publish(Event::new(EventPayload::AgentThought {
                        agent_id: self.profile.id.clone(),
                        thought: content.clone(),
                    }));
                }
            }

            // Check if tool calls were requested
            if response.tool_calls.is_empty() {
                // Task is finished
                let final_output = response
                    .content
                    .unwrap_or_else(|| "Task completed without output".to_string());
                info!(
                    agent = %self.profile.name,
                    task_id = %task.id,
                    "Task execution completed"
                );
                return Ok(final_output);
            }

            // Record the assistant's response with tool calls into conversation history
            let mut assistant_msg =
                ChatMessage::assistant(response.content.clone().unwrap_or_default());
            assistant_msg.tool_calls = Some(response.tool_calls.clone());
            messages.push(assistant_msg);

            // Execute each tool call and emit events to the bus
            for call in response.tool_calls {
                let tool_call = ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                };

                bus.publish(Event::new(EventPayload::ToolCallRequested {
                    agent_id: self.profile.id.clone(),
                    call: tool_call.clone(),
                }));

                let authorization = if let Some(gate) = &self.tool_gate {
                    gate.authorize(&self.profile.id, &tool_call).await?
                } else {
                    ToolAuthorization::Approved { approved_by: None }
                };

                let approved_by = match &authorization {
                    ToolAuthorization::Approved { approved_by } => approved_by.clone(),
                    ToolAuthorization::Denied { .. } => None,
                };

                let tool_res = match authorization {
                    ToolAuthorization::Denied { reason } => cowbot_core::tool::ToolResult::failure(
                        &call.id,
                        format!("Tool call denied by approval policy: {reason}"),
                    ),
                    ToolAuthorization::Approved { .. } => {
                        match self.tools.execute(&tool_call).await {
                            Ok(res) => res,
                            Err(e) => {
                                cowbot_core::tool::ToolResult::failure(&call.id, e.to_string())
                            }
                        }
                    }
                };

                bus.publish(Event::new(EventPayload::ToolCallExecuted {
                    agent_id: self.profile.id.clone(),
                    result: tool_res.clone(),
                }));

                if let Some(recorder) = &self.tool_recorder {
                    recorder
                        .record_tool_call(&tool_call, &tool_res, approved_by.as_deref())
                        .await?;
                }

                let tool_output = if tool_res.success {
                    tool_res.output
                } else {
                    format!("Error: {}", tool_res.error.unwrap_or_default())
                };

                messages.push(ChatMessage::tool(tool_output, call.id));
            }
        }

        Err(CowError::Agent(format!(
            "Agent '{}' exceeded maximum allowed iterations ({})",
            self.profile.name, max_iters
        )))
    }
}
