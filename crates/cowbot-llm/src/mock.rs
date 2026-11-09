use crate::provider::LlmProvider;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse};
use async_trait::async_trait;
use cowbot_core::error::CowResult;
use cowbot_core::usage::{estimate_json_tokens, estimate_tokens, TokenUsage};
use std::sync::Mutex;

/// Mock LLM provider returning predefined responses or echo responses for tests.
pub struct MockLlmProvider {
    responses: Mutex<Vec<ChatCompletionResponse>>,
}

impl MockLlmProvider {
    pub fn new(responses: Vec<ChatCompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }

    pub fn echo() -> Self {
        Self::new(Vec::new())
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

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    #[tokio::test]
    async fn echo_reports_deterministic_usage() {
        let provider = MockLlmProvider::echo();
        let request = ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![ChatMessage::user("Hello CowBot")],
            tools: Vec::new(),
            temperature: None,
        };

        let first = provider.chat_complete(&request).await.unwrap();
        let second = provider.chat_complete(&request).await.unwrap();

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

        let request = ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![ChatMessage::user("Hi")],
            tools: Vec::new(),
            temperature: None,
        };

        let response = provider.chat_complete(&request).await.unwrap();
        assert!(response.usage.is_none());
    }
}
