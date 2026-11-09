pub mod anthropic;
mod endpoint;
pub mod mock;
pub mod openai;
pub mod provider;
pub mod responses;
pub mod types;

pub use anthropic::AnthropicProvider;
pub use cowbot_core::usage::TokenUsage;
pub use mock::MockLlmProvider;
pub use openai::OpenAiCompatibleProvider;
pub use provider::LlmProvider;
pub use responses::ResponsesProvider;
pub use types::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole, LlmToolCall,
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
