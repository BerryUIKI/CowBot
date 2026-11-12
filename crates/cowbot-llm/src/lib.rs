pub mod anthropic;
mod endpoint;
pub mod mock;
pub mod openai;
pub mod prompt_snapshot;
pub mod provider;
pub mod responses;
pub mod stream;
pub mod tokens;
pub mod types;

pub use anthropic::AnthropicProvider;
pub use cowbot_core::usage::TokenUsage;
pub use mock::MockLlmProvider;
pub use openai::OpenAiCompatibleProvider;
pub use prompt_snapshot::{seal_snapshot, snapshot_message, snapshot_request};
pub use provider::LlmProvider;
pub use responses::ResponsesProvider;
pub use stream::{CancelSignal, SseDecoder, SseFrame};
pub use tokens::{token_counter_for_model, tokenizer_for_model, ModelTokenizer, TiktokenCounter};
pub use types::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole, CompletionOutcome,
    LlmToolCall, StreamChunk,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider() {
        let provider = MockLlmProvider::echo();
        let req = ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![ChatMessage::user("Hello CowBot")],
            tools: Vec::new(),
            temperature: None,
        };
        let res = provider.chat_complete(&req).await.unwrap();
        assert_eq!(res.content.unwrap(), "Mock response to: Hello CowBot");
    }

    #[tokio::test]
    async fn providers_without_a_catalogue_report_model_discovery_as_unsupported() {
        let provider = MockLlmProvider::echo();

        assert!(provider.list_models().await.is_err());
    }
}
