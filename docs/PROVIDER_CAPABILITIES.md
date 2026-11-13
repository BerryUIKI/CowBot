# Model provider capabilities

**Applies to:** CowBot 0.1.9. Cross-links [ADR-0016](ADR/0016-model-provider-protocols.md)
and [ADR-0021](ADR/0021-vendor-first-model-providers.md).

CowBot speaks three wire protocols. This page records what each one supports
today and where behaviour differs, so you can predict what a provider will do
before you configure it.

## The three protocols

| Protocol | Used for | Endpoint shape |
| --- | --- | --- |
| **OpenAI Chat Completions** | OpenAI, DeepSeek, Ollama, LocalAI, most compatible relays | `POST {base}/chat/completions` |
| **Anthropic Messages** | Claude, and Claude-compatible relays | `POST {base}/v1/messages` |
| **OpenAI Responses** | Codex-style endpoints | `POST {base}/responses` |

A provider is a base URL, an optional key, and a protocol. You choose the
protocol when you add the provider; CowBot never guesses it from the URL.

## Capability matrix

| Capability | Chat Completions | Anthropic Messages | OpenAI Responses |
| --- | --- | --- | --- |
| **Streaming** | Yes, native SSE | Yes, native SSE | Yes, native SSE |
| **Cancel mid-stream** | Yes, drops the response | Yes, drops the response | Yes, drops the response |
| **System messages** | `system` role in the message list | Hoisted into the request's top-level `system` field; several system messages are joined with a blank line | `system` role in the input list |
| **Tool calls** | `tools` + `tool_calls` deltas | `tools` + `content_block_delta` tool input | `tools` + `output_item` deltas |
| **Parallel tool calls** | Streamed per index, accumulated | Streamed per block index | Streamed per item |
| **Reasoning output** | `reasoning_content` or `reasoning` (relay-style) | `thinking` blocks | Reasoning items |
| **Reasoning tokens in usage** | `completion_tokens_details.reasoning_tokens` | Not reported (0) | `output_tokens_details.reasoning_tokens` |
| **Cache hit/miss usage** | `prompt_tokens_details.cached_tokens`, plus DeepSeek's `prompt_cache_hit_tokens`/`prompt_cache_miss_tokens` | `cache_read_input_tokens` / `cache_creation_input_tokens` | `input_tokens_details.cached_tokens` |
| **Model listing** | `GET {base}/models` | `GET {base}/v1/models` | `GET {base}/models` |
| **Max output tokens** | Request field | Request field; required by the API | Request field |

The raw provider usage object is preserved beside the normalized fields for
every protocol, so nothing the provider reports is lost (ADR-0029).

## Known deviations

- **A provider without streaming still works.** `chat_stream` has a default that
  runs the non-streaming call and emits the whole answer as one text chunk, then
  the tool calls, then the usage. The turn is correct; it just does not appear
  incrementally. Every built-in provider overrides this with a native stream.
- **Reasoning is only as good as the endpoint.** Chat Completions reasoning is
  read from `reasoning_content` (DeepSeek, vLLM) or `reasoning` (OpenRouter);
  an endpoint that invents another field name will show no reasoning.
- **Anthropic reports no reasoning tokens.** The Messages API does not break
  thinking out of the output count in usage, so CowBot records 0 rather than
  guessing.
- **Cache fields are optional at every layer.** A provider that omits them
  yields zero cache hit tokens, which the desktop displays as "no cache data"
  rather than 0%.
- **Model listing depends on the endpoint.** Ollama and many relays do not
  implement it; Fetch models then returns an error, and you type the model name
  by hand. A relay that lists a model it cannot actually serve will fail at the
  first request, not at configuration.
- **Token counting is protocol-independent but vocabulary-dependent.** Only
  OpenAI-compatible `o200k_base` and `cl100k_base` models are counted exactly;
  every other model uses a labelled character estimate (ADR-0035).
- **SDK-specific parameters are not forwarded.** CowBot builds a minimal
  request. Provider-specific options such as `top_k`, `service_tier`, or
  Anthropic's `container` are not exposed in settings.

## Choosing a protocol

- Use **Chat Completions** for anything that advertises OpenAI compatibility —
  it is the widest surface and the most forgiving about optional fields.
- Use **Anthropic Messages** for Claude and for relays that present the
  Messages API. Do not point it at an OpenAI-compatible endpoint; system
  messages and tool schemas will not line up.
- Use **OpenAI Responses** only for endpoints that genuinely implement
  `/responses`. Some "Responses-compatible" proxies forward `/chat/completions`
  instead and will fail.

See the [user guide](USER_GUIDE.md) for the provider setup workflow.
