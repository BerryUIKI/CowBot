# M1 — LLM usage accounting foundation

- Status: Implemented
- Date: 2026-11-09
- Related: [ADR-0029](../ADR/0029-token-usage-accounting.md)
- Follow-ups: M2 context meter and conversations, M3 group conversations and
  routing, M4 agent memory and shared notes, M5 prompt-cache optimisation for
  Anthropic and OpenAI Responses.

## Goal

Record normalized token usage for every model call CowBot makes, persist it per
call in SQLite, and capture model context-window metadata. This is the data
foundation for the conversation context meter, cache hit-rate reporting, and
cost visibility planned for later milestones.

M1 must not change prompt content, agent behavior, or any user-visible chat
flow. It only observes what providers already return.

DeepSeek is the first provider to validate end to end, but the parsing and
storage model are provider-neutral from the start.

## Scope

### In scope

- A normalized usage domain type and recorder trait in `cowbot-core`.
- Usage parsing for OpenAI Chat Completions (including DeepSeek-compatible
  fields), Anthropic Messages, OpenAI Responses, and the mock provider.
- Per-call persistence in a new `llm_calls` table.
- Wiring the recorder through the agent loop and the assignment scheduler.
- Optional model metadata (context window and max output) in the desktop
  provider configuration, plus the Settings fields needed to edit it.
- A deterministic token estimator used as a fallback and by the mock provider.

### Out of scope

- Context projection or the context meter UI.
- Conversation sessions, participants, routing, or the main-agent fallback.
- Context compaction, summaries, agent memory, notes, or FTS.
- Prompt-cache breakpoints, `cache_control`, `prompt_cache_key`, TTL, or
  keepalive pings.
- Cost pricing tables, currency display, or billing reports.
- Streaming usage parsing.
- Retention or pruning of old usage rows.

## Architecture context

The broader context-management design agreed for later milestones is:

- Agent contacts, direct conversations, and group channels remain
  agent-centred. Their context stays agent-managed and bounded; they do not
  show a context meter.
- A new user-created conversation feature (single-agent by default, plus
  multi-agent group conversations) gives each participant an independent
  append-only context branch, shared notes, and a visible context indicator.
- Agents get automatic summaries and long-term memory across surfaces.
- Compaction is manual by default, with an optional automatic mode at a
  configurable threshold.

M1 is deliberately independent of that feature set. It only needs to make the
usage data correct and queryable.

## Data flow

```
Provider response
  -> ChatCompletionResponse { usage, raw_usage }
  -> AgentInstance (after every completion, including tool iterations)
  -> LlmUsageRecorder
  -> llm_calls (SQLite)
```

Router, summary, compaction, and memory-extraction calls will use the same
recorder type later, with their own `LlmCallKind`, without a schema change.

## Work breakdown

### M1.1 `cowbot-core`: usage domain

New module `crates/cowbot-core/src/usage.rs`, exported from `lib.rs`.
Derive `Debug`, `Clone`, `PartialEq`, `Eq`, `Serialize`, and `Deserialize` on
the domain types; `TokenUsage` and `UsageTotals` also derive `Default`.

```rust
pub struct TokenUsage {
    pub uncached_input_tokens: u32,
    pub cached_input_tokens: u32,
    pub cache_write_input_tokens: u32,
    pub output_tokens: u32,
    pub reasoning_tokens: u32,
}

impl TokenUsage {
    pub fn total_input_tokens(&self) -> u32;
    pub fn total_tokens(&self) -> u32;
}

pub enum LlmCallKind {
    Agent,
    Router,
    Summary,
    Compact,
    MemoryExtract,
}

pub struct LlmCallUsage {
    pub model: String,
    pub call_kind: LlmCallKind,
    pub usage: Option<TokenUsage>,
    pub raw_usage: Option<serde_json::Value>,
}

pub struct LlmCallRecord {
    pub id: String,
    pub run_id: Option<RunId>,
    pub task_id: Option<TaskId>,
    pub channel_id: Option<ChannelId>,
    pub agent_id: Option<AgentId>,
    pub call_kind: LlmCallKind,
    pub model: String,
    pub usage: Option<TokenUsage>,
    pub raw_usage: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub struct UsageTotals {
    pub calls: u64,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}

#[async_trait]
pub trait LlmUsageRecorder: Send + Sync {
    async fn record_llm_call(&self, call: &LlmCallUsage) -> CowResult<()>;
}
```

Rules:

- `reasoning_tokens` is a subset of `output_tokens`; `total_tokens()` must not
  add it again.
- Token counts use `u32` in the domain and are stored as SQLite `INTEGER`.
- `LlmCallKind` serializes as `snake_case`.
- `LlmUsageRecorder` is best-effort by contract. Callers log failures and
  continue.

Token estimator:

```rust
pub fn estimate_tokens(text: &str) -> u32;
```

- ASCII characters count as one quarter token each.
- CJK characters (CJK Unified Ideographs, Hiragana, Katakana, Hangul) count as
  one token each.
- Other non-ASCII characters count as half a token.
- Return zero for an empty string, otherwise round up and return at least one.
- Add `estimate_json_tokens(&serde_json::Value) -> u32` by serializing with
  `serde_json::to_string` and applying the same estimate.

Tests: empty, English, Chinese, mixed text, deterministic repeated output,
JSON schema estimation.

### M1.2 `cowbot-llm`: parse and preserve usage

`ChatCompletionResponse` in `crates/cowbot-llm/src/types.rs` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub usage: Option<TokenUsage>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub raw_usage: Option<serde_json::Value>,
```

Parsing approach:

- Deserialize the provider `usage` object as `serde_json::Value` instead of a
  rigid struct, normalize it with a helper, and return the same value as
  `raw_usage`.
- A missing or malformed usage object produces `None`; it never fails a
  completion.
- Saturating subtraction protects against inconsistent OpenAI-style counts.

Per-provider changes:

- `openai.rs`: extend `OpenAiChatResponse` with `usage: Option<Value>`. Parse
  `prompt_tokens`, `completion_tokens`, `prompt_tokens_details.cached_tokens`,
  `completion_tokens_details.reasoning_tokens`. Also accept DeepSeek's
  `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`, preferring them
  when present.
- `anthropic.rs`: parse `usage.input_tokens`, `usage.output_tokens`,
  `usage.cache_read_input_tokens`, and `usage.cache_creation_input_tokens`.
  Anthropic input excludes cached tokens, so no subtraction is performed.
- `responses.rs`: parse `usage.input_tokens`, `usage.output_tokens`,
  `usage.input_tokens_details.cached_tokens`, and
  `usage.output_tokens_details.reasoning_tokens`.
- `mock.rs`: return deterministic usage derived from `estimate_tokens` over
  the request messages, tool schemas, and response content, so agent-loop
  tests can observe growth across tool iterations.
- `lib.rs`: re-export `TokenUsage` for convenience, or let callers import it
  from `cowbot_core`.

Test fixtures:

- OpenAI Chat with and without cached tokens.
- DeepSeek with `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`.
- Anthropic with cache read and cache creation.
- OpenAI Responses with cached and reasoning tokens.
- Missing usage and an empty `usage` object.
- Raw usage preserved exactly.

Mechanical updates: every `ChatCompletionResponse { ... }` literal outside the
providers gains `usage` and `raw_usage`. Known locations:

- `crates/cowbot-agent/src/lib.rs` tests
- `crates/cowbot-service/tests/crew_runner.rs`
- `crates/cowbot-service/tests/scheduler.rs`

### M1.3 `cowbot-agent`: record each completion

`AgentInstance` gains:

```rust
usage_recorder: Option<Arc<dyn LlmUsageRecorder>>,

pub fn set_usage_recorder(&mut self, recorder: Option<Arc<dyn LlmUsageRecorder>>);
pub fn clear_usage_recorder(&mut self);
```

After each successful `chat_complete` call and before executing tools:

```rust
if let Some(recorder) = &self.usage_recorder {
    let call = LlmCallUsage {
        model: self.model.clone(),
        call_kind: LlmCallKind::Agent,
        usage: response.usage.clone(),
        raw_usage: response.raw_usage.clone(),
    };
    if let Err(error) = recorder.record_llm_call(&call).await {
        warn!(error = %error, "Failed to record LLM usage");
    }
}
```

Behavior:

- One row per completion, including every tool iteration.
- A completion with no usage still produces a row with null token columns.
- A recorder error never fails the task.
- No change to messages, prompts, tool definitions, or iteration limits.

Tests:

- A mock provider that returns usage twice (tool call then final answer)
  produces two recorder calls.
- A usage-less mock response still produces a recorder call with `usage: None`.
- A recorder that always errors does not fail `execute_task`.

### M1.4 `cowbot-store`: migration and repository

New migration `crates/cowbot-store/migrations/0005_llm_calls.sql`. Earlier
migration files are not edited (ADR-0025).

```sql
CREATE TABLE llm_calls (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT,
    task_id TEXT,
    channel_id TEXT,
    agent_id TEXT,
    call_kind TEXT NOT NULL,
    model TEXT NOT NULL,
    uncached_input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_input_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_tokens INTEGER,
    raw_usage TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
);

CREATE INDEX idx_llm_calls_run ON llm_calls(run_id);
CREATE INDEX idx_llm_calls_task ON llm_calls(task_id);
CREATE INDEX idx_llm_calls_channel ON llm_calls(channel_id, created_at);
CREATE INDEX idx_llm_calls_agent ON llm_calls(agent_id, created_at);
```

Rules:

- When usage is absent, every token column is `NULL`. When usage is present,
  all five columns are non-null, and zero is a valid value.
- `raw_usage` stores the provider JSON as text.
- `call_kind` is validated in Rust; no SQL `CHECK` constraint is added so new
  kinds do not require a migration.
- `channel_id` is nullable in M1 because the scheduler does not know the
  channel at execution time. Channel-level queries join
  `llm_calls.task_id -> tasks.channel_id`; router and summary calls can set it
  directly when that work lands.
- Rows with a `run_id` cascade with the run. Rows without one are independent.

New `crates/cowbot-store/src/repos/usage.rs`, registered in `repos/mod.rs`,
with methods on `Store`:

```rust
pub async fn record_llm_call(&self, record: &LlmCallRecord) -> CowResult<()>;
pub async fn list_llm_calls_for_run(&self, run_id: &RunId) -> CowResult<Vec<LlmCallRecord>>;
pub async fn list_llm_calls_for_task(&self, task_id: &TaskId) -> CowResult<Vec<LlmCallRecord>>;
pub async fn sum_llm_usage_for_task(&self, task_id: &TaskId) -> CowResult<UsageTotals>;
```

Queries order by `created_at, id`. `UsageTotals` sums only rows with usage;
`calls` counts all rows matching the filter.

Tests:

- Insert and round-trip a row with all fields.
- Insert a row with null usage and read it back.
- Ordering across multiple calls.
- Summing ignores null usage.
- Deleting a run cascades its usage rows.
- A row with no run survives task/run deletion.

### M1.5 `cowbot-service`: bind the recorder to a run

In `crates/cowbot-service/src/scheduler.rs`:

```rust
struct RunUsageRecorder {
    store: Store,
    run_id: RunId,
    task_id: TaskId,
    agent_id: AgentId,
}

#[async_trait]
impl LlmUsageRecorder for RunUsageRecorder {
    async fn record_llm_call(&self, call: &LlmCallUsage) -> CowResult<()> {
        // Build LlmCallRecord from the bound run/task/agent and `call`,
        // then persist through Store::record_llm_call.
    }
}
```

- `AgentExecution::execute` gains an `Arc<dyn LlmUsageRecorder>` parameter and
  sets it on the `AgentInstance`.
- `execute_assignment` constructs the recorder next to `RunToolCallRecorder`
  and passes both.
- No channel id is bound in M1.
- Channel turns and crew runs both travel through the assignment scheduler, so
  both are covered by the same code path.

Tests:

- A channel turn with a mock provider records usage rows for the run.
- A task that fails after one successful completion keeps that completion's
  usage row.
- Usage recording does not alter task output or run status.

### M1.6 Desktop model metadata

Backend (`desktop/src-tauri/src/config.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}
```

- `ProviderConfig` gains
  `model_metadata: BTreeMap<String, ModelMetadata>` with `#[serde(default,
  skip_serializing_if = "BTreeMap::is_empty")]`. `BTreeMap` keeps the written
  configuration deterministic.
- Old `config.json` files load with an empty map.
- Metadata for a model that is temporarily removed from the model list is kept;
  re-adding the model restores its values.
- `ProviderView` and `ProviderDraft` carry the map so the value round-trips
  through Tauri. `ProviderView` serializes it as `model_metadata`; `ProviderDraft`
  already uses `#[serde(rename_all = "camelCase")]`, so the field is
  `modelMetadata` on the wire and `model_metadata` in TypeScript responses.
- `upsert_provider` and the `config::save_provider`/update helpers pass the map
  through when creating or editing a provider.
- Frontend files:
  - `desktop/src/types.ts`: add `ModelMetadata` and
    `ModelProvider.model_metadata`.
  - `desktop/src/ModelProviderSettings.tsx`: carry `modelMetadata` in
    `ProviderDraft`, populate it from the provider, and show two optional
    numeric inputs for the selected model. Blank means unknown.
  - `desktop/src/api.ts`: only if the typed draft shape needs an explicit
    field; the existing `invoke` call already passes the draft object.
- The Settings provider form exposes optional numeric fields for the selected
  model: context window and max output. Empty means unknown. No percentage or
  context meter is shown in M1.
- Do not invent model windows. No built-in model table is added.

Tests:

- Legacy JSON without `model_metadata` parses.
- Round-trip with metadata.
- Empty options are omitted when written.
- Invalid API-key handling is unchanged.

### M1.7 Documentation and gates

- ADR-0029 and this plan.
- ADR index entry.
- No architecture, roadmap, or user-guide change is required until the context
  meter becomes user-visible.

## Normalization reference

```text
TokenUsage.total_input  = uncached_input + cached_input + cache_write_input
TokenUsage.total_tokens = total_input + output_tokens
reasoning_tokens        is a subset of output_tokens
```

| Provider | Uncached input | Cached input | Cache write |
| --- | --- | --- | --- |
| OpenAI Chat | `prompt_tokens - cached` | `prompt_tokens_details.cached_tokens` | 0 |
| OpenAI Responses | `input_tokens - cached` | `input_tokens_details.cached_tokens` | 0 |
| Anthropic | `input_tokens` | `cache_read_input_tokens` | `cache_creation_input_tokens` |
| DeepSeek | `prompt_cache_miss_tokens` or `prompt_tokens - hit` | `prompt_cache_hit_tokens` or `prompt_tokens_details.cached_tokens` | 0 |
| Mock | estimator over request | 0 | 0 |

If a provider reports cached tokens larger than the prompt total, cached is
clamped to the prompt total and uncached becomes zero.

## Behavioral rules

1. Provider usage is authoritative; estimation is a fallback.
2. Missing usage is a valid state. Never substitute estimates in persisted rows
   when the provider omitted usage; keep the row's token columns null.
3. One row per completion, including tool-loop iterations.
4. Context occupancy is a single call's input; billed input is a sum across
   calls. Queries and future UI must label which one they show.
5. Recording is best-effort; errors are logged, not propagated.
6. `raw_usage` is preserved for fields not yet parsed.
7. Model windows are optional metadata and may be unknown.

## Test plan

```bash
cargo test -p cowbot-core
cargo test -p cowbot-llm
cargo test -p cowbot-agent
cargo test -p cowbot-store
cargo test -p cowbot-service
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

If the desktop configuration or Settings UI changes:

```bash
cd desktop
pnpm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Acceptance criteria

- [x] Every provider response that includes usage yields a normalized
      `TokenUsage`, including DeepSeek cache hit/miss fields.
- [x] The raw provider usage object is persisted unchanged.
- [x] The agent loop writes one `llm_calls` row per completion, including each
      tool iteration.
- [x] A response without usage still writes a row with null token columns.
- [x] A recorder failure is logged and the task still completes.
- [x] Usage can be listed by run and task, and summed per task.
- [x] Deleting a run cascades its usage rows; rows without a run are kept.
- [x] Existing databases migrate through a new `0005` migration only.
- [x] Old desktop `config.json` files parse; model metadata round-trips.
- [x] No prompt, chat, or task behavior changes.
- [x] Workspace and desktop gates pass.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Provider usage shapes drift or differ between relays | Deserialize as `Value`, normalize leniently, keep raw JSON |
| DeepSeek cache fields absent on some endpoints | Fall back to OpenAI cached fields, then null usage |
| Test churn from the two new response fields | Mechanical update listed above |
| Migration checksum incompatibility | Add `0005` only; never edit earlier migrations |
| Usage insert failures | Log and continue; the task is not failed |
| Model window guessed incorrectly | No built-in table; optional user metadata; unknown is supported |
| Large `raw_usage` payloads | Providers return small usage objects; truncate only if evidence appears |

## Deferred decisions

- Whether to store `channel_id` directly on agent calls or always join through
  tasks. M2 can decide once the conversation context meter needs channel-level
  aggregates.
- Retention policy for `llm_calls`. M1 keeps rows indefinitely.
- Pricing tables and cost display. M1 records tokens, not currency.
- Exact tokenizer integration. The heuristic estimator is sufficient until the
  context meter shows visible drift.

## Follow-on milestones

- **M2 — Single-agent conversations and context meter**: channel `kind`,
  participants, append-only per-agent context, context ring and popover,
  cache hit-rate display, manual and optional automatic compaction.
- **M3 — Group conversations and routing**: multi-agent conversations,
  mention-aware router, main-agent fallback, per-agent context tabs.
- **M4 — Agent memory and shared notes**: automatic summaries, long-term
  memory, FTS retrieval, workspace and conversation notes.
- **M5 — Prompt-cache optimisation**: Anthropic `cache_control`, OpenAI
  `prompt_cache_key`, TTL and keepalive, cache diagnostics.
