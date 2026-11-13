use crate::stream::CancelSignal;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, CompletionOutcome, StreamChunk};
use async_trait::async_trait;
use cowbot_core::error::{CowError, CowResult};

/// Trait implemented by LLM backend providers.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider identification name (e.g. "openai", "deepseek", "ollama", "mock")
    fn name(&self) -> &str;

    /// Generates a chat completion.
    ///
    /// Callers that need incremental output use [`Self::chat_stream`] instead;
    /// this stays for short calls, provider probes, and tests.
    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse>;

    /// Streams a chat completion, invoking `on_chunk` for each normalized chunk.
    ///
    /// The default implementation adapts a non-streaming provider: it emits the
    /// whole answer as one [`StreamChunk::Text`], then every tool call, then the
    /// usage, so a provider never has to implement streaming to keep turns
    /// working. Providers with a native stream override this.
    ///
    /// Cancellation is cooperative: `cancel` is checked before the request
    /// starts and races the upstream call, and a tripped signal returns
    /// [`CowError::Cancelled`] rather than a provider error (ADR-0041).
    async fn chat_stream(
        &self,
        request: &ChatCompletionRequest,
        cancel: &CancelSignal,
        on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
    ) -> CowResult<CompletionOutcome> {
        crate::stream::ensure_live(cancel)?;
        let response = tokio::select! {
            response = self.chat_complete(request) => response?,
            () = cancel.cancelled() => return Err(CowError::Cancelled),
        };
        crate::stream::ensure_live(cancel)?;

        let mut outcome = CompletionOutcome::default();
        if let Some(content) = &response.content {
            if !content.is_empty() {
                let chunk = StreamChunk::Text(content.clone());
                outcome.apply(chunk.clone());
                on_chunk(chunk);
            }
        }
        for call in &response.tool_calls {
            let chunk = StreamChunk::ToolCall(call.clone());
            outcome.apply(chunk.clone());
            on_chunk(chunk);
        }
        if response.usage.is_some() || response.raw_usage.is_some() {
            let chunk = StreamChunk::Usage {
                usage: response.usage.clone(),
                raw_usage: response.raw_usage.clone(),
            };
            outcome.apply(chunk.clone());
            on_chunk(chunk);
        }
        outcome.finish_reason = response.finish_reason;
        if outcome.content.is_empty() {
            outcome.content = response.content.unwrap_or_default();
        }
        Ok(outcome)
    }

    /// Lists the model ids the endpoint reports.
    ///
    /// Providers without a usable catalogue keep the default, which reports
    /// model discovery as unsupported so callers can fall back to manual entry.
    async fn list_models(&self) -> CowResult<Vec<String>> {
        Err(CowError::Llm(format!(
            "{} does not support model discovery",
            self.name()
        )))
    }
}
