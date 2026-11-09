# ADR 0029: Token usage accounting

- Status: Accepted
- Date: 2026-11-09
- Plan: [M1 — usage accounting foundation](../plans/M1-usage-accounting.md)
- Discussion: None yet
- Supersedes: None

## Context

CowBot calls language models from several paths: the agent loop, channel
mention turns, crew runs, and the desktop's task execution. The runtime
currently discards the usage object every provider returns. `ChatCompletionResponse`
carries only content, tool calls, and a finish reason, and the `runs` table
records lifecycle timestamps without token counts.

The next context-management features need this data:

- a conversation context meter that shows how full a model's context window is;
- cache hit-rate reporting for providers with prompt caching;
- cost visibility;
- compaction decisions that need to know how many tokens a branch uses.

A run can issue many completions. The agent loop sends another request after
every tool iteration, so one `runs` row can correspond to several prompts of
different sizes. A per-run total cannot answer "how full is the context now?"
or "how much of the last request was served from cache?". Usage must be
recorded per model call.

Providers report usage with different semantics:

- OpenAI Chat Completions reports `prompt_tokens`, which already includes
  cached tokens, plus `completion_tokens`.
- OpenAI Responses reports `input_tokens` and `output_tokens`, with cached
  tokens under `input_tokens_details.cached_tokens`.
- Anthropic Messages reports `input_tokens` excluding cached tokens, plus
  separate `cache_read_input_tokens` and `cache_creation_input_tokens`.
- DeepSeek reports `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`
  alongside the OpenAI-compatible `prompt_tokens`.
- OpenAI-compatible relays and local servers may omit usage entirely, or return
  only part of it.

DeepSeek is the primary provider for the first release of this work. Its
context caching is automatic and best-effort: CowBot can influence it only by
keeping the prompt prefix stable, and can observe it only through the usage
fields.

## Decision

### 1. A normalized usage type lives in `cowbot-core`

`crates/cowbot-core/src/usage.rs` defines the domain vocabulary shared by the
LLM clients, the agent runtime, the store, and future UI:

- `TokenUsage` with `uncached_input_tokens`, `cached_input_tokens`,
  `cache_write_input_tokens`, `output_tokens`, and `reasoning_tokens`.
  `total_input_tokens()` is the sum of the three input fields; `total_tokens()`
  adds `output_tokens`. `reasoning_tokens` is a subset of `output_tokens` and
  must never be added a second time.
- `LlmCallKind`: `agent`, `router`, `summary`, `compact`, `memory_extract`.
  The additional kinds are reserved so later routing, compaction, and memory
  work can record calls without a schema change.
- `LlmCallUsage`, the data a recorder receives for one completion.
- `LlmCallRecord`, the persisted row.
- `LlmUsageRecorder`, an async trait mirroring `ToolCallRecorder`.
- `estimate_tokens`, a deterministic character-based estimate used when a
  provider returns no usage and by later context projections. It is explicitly
  approximate and never replaces provider-reported values.

### 2. LLM clients parse usage and preserve the raw object

`ChatCompletionResponse` gains `usage: Option<TokenUsage>` and
`raw_usage: Option<serde_json::Value>`. Each provider parses its `usage`
object leniently, normalizes it into `TokenUsage`, and keeps the original JSON
for future fields and diagnostics. Missing or malformed usage becomes `None`,
never an error: a completion that produced text or tool calls is still valid.

Normalization rules:

| Provider | Uncached input | Cached input | Cache write | Output | Reasoning |
| --- | --- | --- | --- | --- | --- |
| OpenAI Chat Completions | `prompt_tokens - cached_tokens` | `prompt_tokens_details.cached_tokens` | 0 | `completion_tokens` | `completion_tokens_details.reasoning_tokens` |
| OpenAI Responses | `input_tokens - cached_tokens` | `input_tokens_details.cached_tokens` | 0 | `output_tokens` | `output_tokens_details.reasoning_tokens` |
| Anthropic Messages | `input_tokens` | `cache_read_input_tokens` | `cache_creation_input_tokens` | `output_tokens` | 0 |
| DeepSeek (OpenAI-compatible) | `prompt_cache_miss_tokens`, else `prompt_tokens - prompt_cache_hit_tokens` | `prompt_cache_hit_tokens`, else `prompt_tokens_details.cached_tokens` | 0 | `completion_tokens` | `completion_tokens_details.reasoning_tokens` |
| Mock | estimated from the request messages | 0 | 0 | estimated from the response content | 0 |

Subtractions saturate at zero. If a provider reports an inconsistent object,
the raw JSON is still preserved so the normalization can be corrected later
without losing data.

### 3. Usage is persisted per call, not per run

Migration `0005_llm_calls.sql` adds an `llm_calls` table. Each row describes one
provider completion:

- `id`, `call_kind`, `model`, `created_at`;
- optional `run_id`, `task_id`, `channel_id`, `agent_id`;
- nullable normalized token columns;
- nullable `raw_usage` JSON.

Rows are written as soon as a completion returns, before tools execute. A run
that fails after several iterations therefore keeps the usage of every prompt
it already paid for. Router and summarizer calls later use rows without a
`run_id` and with their own `call_kind`.

The table distinguishes two questions that must not be conflated:

- **context occupancy** is the total input of one call (the latest or the peak
  in a run);
- **billed input** is the sum of inputs across all calls in a run.

Aggregating queries must state which one they mean. The context meter uses
occupancy; cost reporting uses billed sums.

Rows that reference a run are deleted with it (`ON DELETE CASCADE`). Rows
without a run are kept because they are not owned by a task.

### 4. Recording is best-effort and does not change agent behavior

`AgentInstance` accepts an optional `LlmUsageRecorder` alongside its tool gate
and tool recorder. After every successful completion it reports the model, the
call kind, the normalized usage (or `None`), and the raw object. The scheduler
binds a `RunUsageRecorder` to the current run, task, and agent, following the
same pattern as `RunToolCallRecorder`.

A recorder error is logged and ignored. Usage is observability data; a failed
insert must not fail the user's task. This differs deliberately from tool-call
recording, which is part of the audit trail.

### 5. Model metadata is explicit and may be unknown

Context-window and output limits are model properties, not token-usage
properties. The desktop provider configuration gains an optional
`model_metadata` map keyed by model id, with `context_window` and
`max_output_tokens`. Both fields are optional, and the map is empty by default.

No built-in table is introduced in this milestone. Model catalogues change,
endpoints can alias names, and a wrong window is worse than an unknown one.
Missing metadata is a supported state: later UI shows absolute token counts
and omits percentages until the user supplies a window.

### 6. Estimation is a fallback, not a source of truth

`estimate_tokens` uses a fixed character heuristic (ASCII characters divided
by four, CJK characters counted individually, other non-ASCII characters
weighted between the two). It is used for the mock provider and for later
projections before a request is sent. Provider-reported usage always wins once
a call completes.

## Alternatives considered

- **Aggregate usage on `runs` only.** Smaller schema, but it loses the tool-loop
  breakdown and cannot support an accurate context meter. Rejected.
- **Keep provider-specific usage types and normalize in the UI.** Leaks
  protocol details into the desktop and duplicates the same arithmetic in every
  consumer. Rejected.
- **Use a tokenizer as the source of truth.** Tokenizers differ per model,
  require extra dependencies and model files, and cannot replace what the
  provider actually billed. Kept only as the estimator fallback.
- **Store usage in memory or logs only.** No restart history, no cost
  aggregation, and nothing for the later context meter to read. Rejected.
- **Fail the run when recording fails.** Turns an observability problem into a
  user-visible failure. Rejected.
- **Hardcode context windows for known models.** Provider aliases changed
  repeatedly in the existing catalogue work, and the current DeepSeek docs do
  not expose a stable window table. Rejected in favor of explicit metadata.

## Consequences

- A new migration and repository module are required; earlier migrations stay
  untouched (ADR-0025).
- `ChatCompletionResponse` grows two fields, so existing struct literals in
  tests and the mock provider must be updated mechanically.
- The data enables the conversation context meter, cache hit-rate reporting,
  and cost views planned for later milestones.
- Provider differences remain observable through `raw_usage`; normalization
  can be refined without a schema change.
- Usage rows are best-effort. Counts may be incomplete when an endpoint omits
  usage or when a database write fails; consumers must treat missing values as
  missing, not as zero-cost calls.
- Cached tokens and cache writes are recorded separately. A high cached share
  is not automatically cheaper unless cache-write pricing is also considered.
- Model metadata is user-configured until a trustworthy catalogue exists.
  Context percentages may be unavailable for a new model.
