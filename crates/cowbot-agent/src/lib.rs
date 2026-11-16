pub mod agent;
pub mod crew;
pub mod roles;

pub use agent::AgentInstance;
pub use agent::{ConversationRecorder, TurnObserver};
pub use crew::Crew;
pub use roles::StandardRoles;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cowbot_bus::EventBus;
    use cowbot_core::agent::AgentId;
    use cowbot_core::cancel::CancelSignal;
    use cowbot_core::error::{CowError, CowResult};
    use cowbot_core::task::Task;
    use cowbot_core::tool::{ToolAuthorization, ToolCall, ToolGate};
    use cowbot_core::usage::{LlmCallKind, LlmCallUsage, LlmUsageRecorder, TokenUsage};
    use cowbot_llm::{
        ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole, LlmProvider,
        LlmToolCall, MockLlmProvider, MockTurn, StreamChunk,
    };
    use cowbot_tools::{CodeExecutionTool, SubprocessSandbox, ToolRegistry};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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

    struct RecordingLlm {
        requests: Mutex<Vec<ChatCompletionRequest>>,
    }

    impl RecordingLlm {
        fn new() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<ChatCompletionRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LlmProvider for RecordingLlm {
        fn name(&self) -> &str {
            "recording"
        }

        async fn chat_complete(
            &self,
            request: &ChatCompletionRequest,
        ) -> CowResult<ChatCompletionResponse> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(ChatCompletionResponse {
                content: Some("planned".to_string()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: None,
                raw_usage: None,
            })
        }
    }

    #[tokio::test]
    async fn mode_instruction_is_included_in_caller_assembled_messages() {
        let llm = Arc::new(RecordingLlm::new());
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm.clone(),
            ToolRegistry::new(),
            "mock-model",
        );
        agent.set_mode_instruction(Some("You are in Plan mode.".to_string()));

        let task = Task::new("Reply in the conversation", "An answer");
        let messages = vec![
            ChatMessage::system("system prompt"),
            ChatMessage::user("latest question"),
        ];
        agent
            .execute_messages(&task, messages, &EventBus::default_bus())
            .await
            .unwrap();

        let requests = llm.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].messages[0].role, ChatRole::System);
        assert_eq!(requests[0].messages[0].content, "system prompt");
        assert_eq!(requests[0].messages[1].role, ChatRole::System);
        assert_eq!(requests[0].messages[1].content, "You are in Plan mode.");
        assert_eq!(requests[0].messages[2].content, "latest question");
    }

    /// Records what the agent reports while it runs.
    #[derive(Default)]
    struct RecordingObserver {
        deltas: Mutex<Vec<String>>,
        tools_started: Mutex<Vec<String>>,
        tools_finished: Mutex<Vec<String>>,
    }

    impl TurnObserver for RecordingObserver {
        fn text_delta(&self, text: &str) {
            self.deltas.lock().unwrap().push(text.to_string());
        }

        fn tool_call_started(&self, call: &ToolCall) {
            self.tools_started.lock().unwrap().push(call.name.clone());
        }

        fn tool_call_finished(&self, result: &cowbot_core::tool::ToolResult) {
            self.tools_finished
                .lock()
                .unwrap()
                .push(result.call_id.clone());
        }
    }

    #[tokio::test]
    async fn text_deltas_are_reported_as_they_stream() {
        let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
            chunks: vec![
                StreamChunk::Text("Hello ".to_string()),
                StreamChunk::Text("there".to_string()),
            ],
            finish_reason: Some("stop".to_string()),
            chunk_delay: None,
            error_after_chunks: None,
        }]));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let observer = Arc::new(RecordingObserver::default());
        agent.set_observer(Some(observer.clone()));

        let task = Task::new("Say hello", "Greeting");
        let output = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await
            .unwrap();

        assert_eq!(output, "Hello there");
        assert_eq!(
            observer.deltas.lock().unwrap().clone(),
            vec!["Hello ".to_string(), "there".to_string()]
        );
        assert_eq!(agent.draft(), "Hello there");
    }

    #[tokio::test]
    async fn tool_calls_are_reported_around_their_execution() {
        let llm = Arc::new(MockLlmProvider::with_turns(vec![
            MockTurn {
                chunks: vec![
                    StreamChunk::Text("checking".to_string()),
                    StreamChunk::ToolCall(LlmToolCall {
                        id: "call-1".to_string(),
                        name: "missing_tool".to_string(),
                        arguments: json!({}),
                    }),
                ],
                finish_reason: Some("tool_calls".to_string()),
                chunk_delay: None,
                error_after_chunks: None,
            },
            MockTurn::text("done"),
        ]));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let observer = Arc::new(RecordingObserver::default());
        agent.set_observer(Some(observer.clone()));

        let task = Task::new("Use a tool", "Answer");
        let output = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await
            .unwrap();

        assert_eq!(output, "done");
        assert_eq!(
            observer.tools_started.lock().unwrap().clone(),
            vec!["missing_tool".to_string()]
        );
        assert_eq!(
            observer.tools_finished.lock().unwrap().clone(),
            vec!["call-1".to_string()]
        );
    }

    #[tokio::test]
    async fn cancelling_a_turn_keeps_the_partial_text() {
        let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
            chunks: vec![
                StreamChunk::Text("partial ".to_string()),
                StreamChunk::Text("answer".to_string()),
            ],
            finish_reason: Some("stop".to_string()),
            chunk_delay: Some(Duration::from_millis(400)),
            error_after_chunks: None,
        }]));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );
        let cancel = CancelSignal::new();
        agent.set_cancel(cancel.clone());
        let observer = Arc::new(RecordingObserver::default());
        agent.set_observer(Some(observer.clone()));

        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });

        let task = Task::new("Write a long answer", "Answer");
        let result = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await;

        assert!(matches!(result, Err(CowError::Cancelled)));
        assert_eq!(agent.draft(), "partial ");
        assert_eq!(
            observer.deltas.lock().unwrap().clone(),
            vec!["partial ".to_string()]
        );
    }

    #[tokio::test]
    async fn a_provider_error_after_partial_text_keeps_the_draft() {
        let llm = Arc::new(MockLlmProvider::with_turns(vec![MockTurn {
            chunks: vec![StreamChunk::Text("half".to_string())],
            finish_reason: None,
            chunk_delay: None,
            error_after_chunks: Some("upstream reset".to_string()),
        }]));
        let mut agent = AgentInstance::new(
            StandardRoles::coder(),
            llm,
            ToolRegistry::new(),
            "mock-model",
        );

        let task = Task::new("Fail mid-way", "Answer");
        let result = agent
            .execute_task(&task, &[], &EventBus::default_bus())
            .await;

        assert!(result.is_err());
        assert_eq!(agent.draft(), "half");
    }
}
