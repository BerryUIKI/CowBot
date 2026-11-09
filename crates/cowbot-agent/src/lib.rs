pub mod agent;
pub mod crew;
pub mod roles;

pub use agent::AgentInstance;
pub use agent::ConversationRecorder;
pub use crew::Crew;
pub use roles::StandardRoles;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cowbot_bus::EventBus;
    use cowbot_core::agent::AgentId;
    use cowbot_core::error::{CowError, CowResult};
    use cowbot_core::task::Task;
    use cowbot_core::tool::{ToolAuthorization, ToolCall, ToolGate};
    use cowbot_core::usage::{LlmCallKind, LlmCallUsage, LlmUsageRecorder, TokenUsage};
    use cowbot_llm::{
        ChatCompletionResponse, ChatMessage, ChatRole, LlmToolCall, MockLlmProvider,
    };
    use cowbot_tools::{CodeExecutionTool, SubprocessSandbox, ToolRegistry};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct DenyGate {
        calls: Arc<AtomicUsize>,
    }

    struct CollectingUsage {
        calls: Mutex<Vec<LlmCallUsage>>,
    }

    impl CollectingUsage {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<LlmCallUsage> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LlmUsageRecorder for CollectingUsage {
        async fn record_llm_call(&self, call: &LlmCallUsage) -> CowResult<()> {
            self.calls.lock().unwrap().push(call.clone());
            Ok(())
        }
    }

    struct FailingUsage;

    #[async_trait]
    impl LlmUsageRecorder for FailingUsage {
        async fn record_llm_call(&self, _call: &LlmCallUsage) -> CowResult<()> {
            Err(CowError::Storage("usage store unavailable".to_string()))
        }
    }

    #[async_trait]
    impl ToolGate for DenyGate {
        async fn authorize(
            &self,
            _agent_id: &AgentId,
            _call: &ToolCall,
        ) -> Result<ToolAuthorization, CowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolAuthorization::Denied {
                reason: "denied in test".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn test_crew_sequential_execution() {
        let bus = EventBus::default_bus();
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(CodeExecutionTool::new(Arc::new(
            SubprocessSandbox::default(),
        ))));

        let mock_llm = Arc::new(MockLlmProvider::echo());

        let planner_profile = StandardRoles::planner();
        let planner_id = planner_profile.id.clone();
        let planner = AgentInstance::new(
            planner_profile,
            mock_llm.clone(),
            tools.clone(),
            "mock-model",
        );

        let coder_profile = StandardRoles::coder();
        let coder_id = coder_profile.id.clone();
        let coder =
            AgentInstance::new(coder_profile, mock_llm.clone(), tools.clone(), "mock-model");

        let mut crew = Crew::new();
        crew.add_agent(planner);
        crew.add_agent(coder);

        let task1 = Task::new("Create implementation plan", "List of steps").assign_to(planner_id);
        let task1_id = task1.id.clone();

        let task2 = Task::new("Implement the code", "Working source code")
            .assign_to(coder_id)
            .with_context_task(task1_id.clone());

        crew.add_task(task1);
        crew.add_task(task2);

        let results = crew.kickoff(&bus).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains_key(&task1_id));
    }

    #[tokio::test]
    async fn tool_gate_can_deny_tool_calls() {
        let bus = EventBus::default_bus();
        let responses = vec![
            ChatCompletionResponse {
                content: Some("trying".to_string()),
                tool_calls: vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "execute_code".to_string(),
                    arguments: json!({ "command": "echo hi" }),
                }],
                finish_reason: Some("tool_calls".to_string()),
                usage: None,
                raw_usage: None,
            },
            ChatCompletionResponse {
                content: Some("finished".to_string()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: None,
                raw_usage: None,
            },
        ];
        let llm = Arc::new(MockLlmProvider::new(responses));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );

        let calls = Arc::new(AtomicUsize::new(0));
        agent.set_tool_gate(Some(Arc::new(DenyGate {
            calls: calls.clone(),
        })));

        let task = Task::new("Run a command", "Output");
        let output = agent.execute_task(&task, &[], &bus).await.unwrap();
        assert_eq!(output, "finished");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn records_usage_for_every_completion() {
        let responses = vec![
            ChatCompletionResponse {
                content: Some("calling".to_string()),
                tool_calls: vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "missing_tool".to_string(),
                    arguments: json!({}),
                }],
                finish_reason: Some("tool_calls".to_string()),
                usage: Some(TokenUsage {
                    uncached_input_tokens: 100,
                    output_tokens: 10,
                    ..TokenUsage::default()
                }),
                raw_usage: Some(json!({"prompt_tokens": 100})),
            },
            ChatCompletionResponse {
                content: Some("done".to_string()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: Some(TokenUsage {
                    uncached_input_tokens: 150,
                    output_tokens: 5,
                    ..TokenUsage::default()
                }),
                raw_usage: None,
            },
        ];
        let llm = Arc::new(MockLlmProvider::new(responses));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let recorder = Arc::new(CollectingUsage::new());
        agent.set_usage_recorder(Some(recorder.clone()));

        let task = Task::new("Do the work", "An answer");
        let output = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await
            .unwrap();

        assert_eq!(output, "done");
        let calls = recorder.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_kind, LlmCallKind::Agent);
        assert_eq!(calls[0].model, "mock-model");
        assert_eq!(calls[0].usage.as_ref().unwrap().uncached_input_tokens, 100);
        assert_eq!(calls[1].usage.as_ref().unwrap().uncached_input_tokens, 150);
    }

    #[tokio::test]
    async fn records_missing_usage_as_none() {
        let llm = Arc::new(MockLlmProvider::new(vec![ChatCompletionResponse {
            content: Some("done".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        }]));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let recorder = Arc::new(CollectingUsage::new());
        agent.set_usage_recorder(Some(recorder.clone()));

        let task = Task::new("Do the work", "An answer");
        agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await
            .unwrap();

        let calls = recorder.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].usage.is_none());
        assert!(calls[0].raw_usage.is_none());
    }

    #[tokio::test]
    async fn usage_recorder_failure_does_not_fail_the_task() {
        let llm = Arc::new(MockLlmProvider::echo());
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        agent.set_usage_recorder(Some(Arc::new(FailingUsage)));

        let task = Task::new("Do the work", "An answer");
        let output = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await
            .unwrap();

        assert!(output.starts_with("Mock response to:"));
    }

    struct CollectingConversation {
        messages: Mutex<Vec<ChatMessage>>,
    }

    impl CollectingConversation {
        fn new() -> Self {
            Self {
                messages: Mutex::new(Vec::new()),
            }
        }

        fn messages(&self) -> Vec<ChatMessage> {
            self.messages.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ConversationRecorder for CollectingConversation {
        async fn record_message(&self, message: &ChatMessage) -> CowResult<()> {
            self.messages.lock().unwrap().push(message.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn execute_messages_uses_the_caller_history_and_records_the_loop() {
        let responses = vec![
            ChatCompletionResponse {
                content: Some("reading".to_string()),
                tool_calls: vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({ "path": "missing.txt" }),
                }],
                finish_reason: Some("tool_calls".to_string()),
                usage: None,
                raw_usage: None,
            },
            ChatCompletionResponse {
                content: Some("final answer".to_string()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: None,
                raw_usage: None,
            },
        ];
        let llm = Arc::new(MockLlmProvider::new(responses));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let recorder = Arc::new(CollectingConversation::new());
        agent.set_conversation_recorder(Some(recorder.clone()));

        let task = Task::new("Reply in the conversation", "An answer");
        let messages = vec![
            ChatMessage::system("system prompt"),
            ChatMessage::user("earlier question"),
            ChatMessage::assistant("earlier answer"),
            ChatMessage::user("latest question"),
        ];
        let output = agent
            .execute_messages(&task, messages, &EventBus::default_bus())
            .await
            .unwrap();

        assert_eq!(output, "final answer");
        let recorded = recorder.messages();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].role, ChatRole::Assistant);
        assert!(recorded[0].tool_calls.is_some());
        assert_eq!(recorded[1].role, ChatRole::Tool);
        assert_eq!(recorded[1].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(recorded[2].role, ChatRole::Assistant);
        assert_eq!(recorded[2].content, "final answer");
    }
}
