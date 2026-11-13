use crate::provider::LlmProvider;
use crate::stream::CancelSignal;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, CompletionOutcome, StreamChunk};
use async_trait::async_trait;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::usage::{estimate_json_tokens, estimate_tokens, TokenUsage};
use std::sync::Mutex;
use std::time::Duration;

/// One scripted model call: the chunks it streams and how it behaves.
#[derive(Clone, Debug)]
pub struct MockTurn {
    /// Chunks streamed in order. A tool call is a chunk too, so a script can
    /// describe a whole agent loop.
    pub chunks: Vec<StreamChunk>,
    pub finish_reason: Option<String>,
    /// Delay between chunks, so a test can act while a turn is still running.
    pub chunk_delay: Option<Duration>,
    /// Return this error after the chunks above have been delivered.
    pub error_after_chunks: Option<String>,
}

impl MockTurn {
    /// A turn that streams `text` as one chunk and then finishes.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            chunks: vec![StreamChunk::Text(text.into())],
            finish_reason: Some("stop".to_string()),
            chunk_delay: None,
            error_after_chunks: None,
        }
    }

    pub fn with_chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = Some(delay);
        self
    }

    pub fn failing_after(mut self, message: impl Into<String>) -> Self {
        self.error_after_chunks = Some(message.into());
        self
    }
}

/// Mock LLM provider returning predefined responses or echo responses for tests.
pub struct MockLlmProvider {
    responses: Mutex<Vec<ChatCompletionResponse>>,
    turns: Mutex<Vec<MockTurn>>,
}

impl MockLlmProvider {
    pub fn new(responses: Vec<ChatCompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            turns: Mutex::new(Vec::new()),
        }
    }

    pub fn echo() -> Self {
        Self::new(Vec::new())
    }

    /// Queue scripted streaming turns, consumed in order.
    ///
    /// Consuming a turn removes it from the queue in the order it was given, so
    /// the script mirrors the agent loop's call sequence.
    pub fn with_turns(turns: Vec<MockTurn>) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            turns: Mutex::new(turns),
        }
    }
}

fn estimate_input_tokens(request: &ChatCompletionRequest) -> u32 {
    let mut input = 0_u32;
    for message in &request.messages {
        input = input.saturating_add(estimate_tokens(&message.content));
        if let Some(name) = &message.name {
            input = input.saturating_add(estimate_tokens(name));
        }
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                input = input.saturating_add(estimate_tokens(&call.name));
                input = input.saturating_add(estimate_json_tokens(&call.arguments));
            }
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            input = input.saturating_add(estimate_tokens(tool_call_id));
        }
    }
    for tool in &request.tools {
        input = input.saturating_add(estimate_tokens(&tool.name));
        input = input.saturating_add(estimate_tokens(&tool.description));
        input = input.saturating_add(estimate_json_tokens(&tool.parameters));
    }
    input
}

/// Split a string into word-sized deltas, so tests exercise incremental
/// rendering instead of one big chunk.
fn words(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        current.push(character);
        if character == ' ' {
            pieces.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        {
            let mut turns = self.turns.lock().unwrap();
            if !turns.is_empty() {
                let turn = turns.remove(0);
                if let Some(error) = turn.error_after_chunks {
                    return Err(CowError::Llm(error));
                }
                let mut outcome = CompletionOutcome::default();
                for chunk in turn.chunks {
                    outcome.apply(chunk);
                }
                outcome.finish_reason = turn.finish_reason;
                if outcome.usage.is_none() {
                    outcome.usage = Some(TokenUsage {
                        uncached_input_tokens: estimate_input_tokens(request),
                        output_tokens: estimate_tokens(&outcome.content),
                        ..TokenUsage::default()
                    });
                }
                return Ok(outcome.into_response());
            }
        }

        let mut queue = self.responses.lock().unwrap();
        if !queue.is_empty() {
            return Ok(queue.remove(0));
        }

        // Default echo behavior
        let last_msg = request
            .messages
            .last()
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let content = format!("Mock response to: {}", last_msg);
        Ok(ChatCompletionResponse {
            usage: Some(TokenUsage {
                uncached_input_tokens: estimate_input_tokens(request),
                output_tokens: estimate_tokens(&content),
                ..TokenUsage::default()
            }),
            raw_usage: None,
            content: Some(content),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
        })
    }

    async fn chat_stream(
        &self,
        request: &ChatCompletionRequest,
        cancel: &CancelSignal,
        on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
    ) -> CowResult<CompletionOutcome> {
        crate::stream::ensure_live(cancel)?;

        let scripted = self.turns.lock().ok().and_then(|mut turns| {
            if turns.is_empty() {
                None
            } else {
                Some(turns.remove(0))
            }
        });
        // Only the echo fallback invents usage. A scripted turn keeps the
        // chunks it was given, and a queued response reports its own usage,
        // including none.
        let mut inferred_usage = false;
        let scripted = match scripted {
            Some(turn) => Some(turn),
            None => {
                let mut queue = self.responses.lock().unwrap();
                if queue.is_empty() {
                    inferred_usage = true;
                    None
                } else {
                    let response = queue.remove(0);
                    Some(turn_from_response(response))
                }
            }
        };

        let Some(turn) = scripted else {
            // Echo: stream the answer word by word so the deltas are real.
            let last_msg = request
                .messages
                .last()
                .map(|m| m.content.as_str())
                .unwrap_or("");
            let content = format!("Mock response to: {}", last_msg);
            let mut outcome = CompletionOutcome::default();
            for piece in words(&content) {
                crate::stream::ensure_live(cancel)?;
                let chunk = StreamChunk::Text(piece);
                outcome.apply(chunk.clone());
                on_chunk(chunk);
            }
            let usage = StreamChunk::Usage {
                usage: Some(TokenUsage {
                    uncached_input_tokens: estimate_input_tokens(request),
                    output_tokens: estimate_tokens(&content),
                    ..TokenUsage::default()
                }),
                raw_usage: None,
            };
            outcome.apply(usage.clone());
            on_chunk(usage);
            outcome.finish_reason = Some("stop".to_string());
            return Ok(outcome);
        };

        let mut outcome = CompletionOutcome::default();
        let mut first = true;
        for chunk in turn.chunks {
            // The delay sits between chunks, so the first one is delivered at
            // once and a test can act while the rest is still pending.
            if let Some(delay) = turn.chunk_delay {
                if !first {
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = cancel.cancelled() => return Err(CowError::Cancelled),
                    }
                }
            }
            first = false;
            crate::stream::ensure_live(cancel)?;
            outcome.apply(chunk.clone());
            on_chunk(chunk);
        }

        if let Some(error) = turn.error_after_chunks {
            return Err(CowError::Llm(error));
        }
        outcome.finish_reason = turn.finish_reason;
        if inferred_usage && outcome.usage.is_none() {
            let usage = StreamChunk::Usage {
                usage: Some(TokenUsage {
                    uncached_input_tokens: estimate_input_tokens(request),
                    output_tokens: estimate_tokens(&outcome.content),
                    ..TokenUsage::default()
                }),
                raw_usage: None,
            };
            outcome.apply(usage.clone());
            on_chunk(usage);
        }
        Ok(outcome)
    }
}

fn turn_from_response(response: ChatCompletionResponse) -> MockTurn {
    let mut chunks = Vec::new();
    if let Some(content) = response.content {
        chunks.push(StreamChunk::Text(content));
    }
    for call in response.tool_calls {
        chunks.push(StreamChunk::ToolCall(call));
    }
    if response.usage.is_some() || response.raw_usage.is_some() {
        chunks.push(StreamChunk::Usage {
            usage: response.usage,
            raw_usage: response.raw_usage,
        });
    }
    MockTurn {
        chunks,
        finish_reason: response.finish_reason,
        chunk_delay: None,
        error_after_chunks: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![ChatMessage::user("Hello CowBot")],
            tools: Vec::new(),
            temperature: None,
        }
    }

    #[tokio::test]
    async fn echo_reports_deterministic_usage() {
        let provider = MockLlmProvider::echo();

        let first = provider.chat_complete(&request()).await.unwrap();
        let second = provider.chat_complete(&request()).await.unwrap();

        assert_eq!(first.usage, second.usage);
        let usage = first.usage.expect("mock usage should be present");
        assert!(usage.uncached_input_tokens > 0);
        assert!(usage.output_tokens > 0);
    }

    #[tokio::test]
    async fn queued_responses_are_returned_unchanged() {
        let provider = MockLlmProvider::new(vec![ChatCompletionResponse {
            content: Some("scripted".to_string()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        }]);

        let response = provider.chat_complete(&request()).await.unwrap();
        assert!(response.usage.is_none());
    }

    #[tokio::test]
    async fn streaming_echo_yields_more_than_one_delta() {
        let provider = MockLlmProvider::echo();
        let cancel = CancelSignal::new();
        let mut deltas = Vec::new();

        let outcome = provider
            .chat_stream(&request(), &cancel, &mut |chunk| {
                if let StreamChunk::Text(text) = chunk {
                    deltas.push(text);
                }
            })
            .await
            .unwrap();

        assert!(deltas.len() > 1, "the echo stream is split into words");
        assert_eq!(deltas.concat(), "Mock response to: Hello CowBot");
        assert_eq!(outcome.content, deltas.concat());
        assert!(outcome.usage.is_some());
    }

    #[tokio::test]
    async fn a_cancelled_signal_stops_before_streaming() {
        let provider = MockLlmProvider::echo();
        let cancel = CancelSignal::new();
        cancel.cancel();
        let mut called = false;

        let result = provider
            .chat_stream(&request(), &cancel, &mut |_chunk| called = true)
            .await;

        assert!(matches!(result, Err(CowError::Cancelled)));
        assert!(!called);
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_slow_scripted_turn() {
        let provider = MockLlmProvider::with_turns(vec![MockTurn {
            chunks: vec![
                StreamChunk::Text("first ".to_string()),
                StreamChunk::Text("second".to_string()),
            ],
            finish_reason: Some("stop".to_string()),
            chunk_delay: Some(Duration::from_millis(200)),
            error_after_chunks: None,
        }]);
        let cancel = CancelSignal::new();
        let mut seen = Vec::new();

        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            stopper.cancel();
        });

        let result = provider
            .chat_stream(&request(), &cancel, &mut |chunk| {
                if let StreamChunk::Text(text) = chunk {
                    seen.push(text);
                }
            })
            .await;

        assert!(matches!(result, Err(CowError::Cancelled)));
        assert_eq!(seen, vec!["first ".to_string()]);
    }

    #[tokio::test]
    async fn a_provider_error_after_partial_text_is_reported_after_the_deltas() {
        let provider = MockLlmProvider::with_turns(vec![MockTurn {
            chunks: vec![StreamChunk::Text("partial".to_string())],
            finish_reason: None,
            chunk_delay: None,
            error_after_chunks: Some("upstream reset".to_string()),
        }]);
        let cancel = CancelSignal::new();
        let mut seen = Vec::new();

        let result = provider
            .chat_stream(&request(), &cancel, &mut |chunk| {
                if let StreamChunk::Text(text) = chunk {
                    seen.push(text);
                }
            })
            .await;

        match result {
            Err(CowError::Llm(message)) => assert!(message.contains("upstream reset")),
            other => panic!("expected a provider error, got {other:?}"),
        }
        assert_eq!(seen, vec!["partial".to_string()]);
    }
}
