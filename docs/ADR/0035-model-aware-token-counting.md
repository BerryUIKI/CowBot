# ADR 0035: Model-aware token counting

- Status: Accepted
- Date: 2026-11-12
- Discussion: [#181](https://github.com/BerryUIKI/CowBot/issues/181)
- Supersedes: None. Refines the counting rules in [ADR-0029](0029-token-usage-accounting.md) and [ADR-0030](0030-conversation-sessions-and-context.md).

## Context

Before this decision every pre-request count came from `estimate_tokens`: an
ASCII character counts as a quarter token, a CJK character as one token, and
another non-ASCII character as half a token. The result is persisted in
`context_entries.tokens` and `memory_entries.tokens` when the row is written.
Four decisions read it:

- the context meter and the pre-send capacity block,
- the group capacity check and the automatic compaction threshold,
- the compaction split, which decides what is kept verbatim,
- the router's history budget and the memory recall budget.

The estimate is model-independent, so one number is shown and compared against
a model-specific window. Code, JSON tool schemas, emoji, and CJK drift furthest
from the real encoder, and message framing overhead is not counted at all. A
70%-of-window threshold can therefore fire late or early by a large margin.

Exact counts are model-specific, and persisted history cannot be rewritten
whenever a model changes. Anthropic publishes no licensed local tokenizer;
DeepSeek and local runtimes differ by model and quantization. The issue left
the storage policy open: keep persisted estimates and compute exact counts per
request, with or without a cache, or store per-model counts.

## Decision

### 1. One counter contract, two implementations

`cowbot_core::usage` adds `TokenSource { Tiktoken, Provider, Estimate }`, the
`MESSAGE_OVERHEAD_TOKENS` constant, and the object-safe
`TokenCounter { count, source, label, count_message }` trait.
`EstimateTokenCounter` wraps `estimate_tokens` and labels itself
`character estimate`.

`cowbot-llm` adds `token_counter_for_model`. It normalizes the model name
(case, a `vendor/` routing prefix, and an `ft:...` fine-tune suffix) and
returns an exact counter for `o200k_base` (GPT-5, GPT-4.1, GPT-4o, the
o-series) and `cl100k_base` (GPT-4, GPT-3.5-turbo, Azure aliases). Every other
name — Anthropic, DeepSeek, `gpt-oss`, local runtimes, unknown proxies — gets
the estimator. The vocabularies are compiled in by `tiktoken-rs`; no vendor
vocabulary is bundled by CowBot.

### 2. Persisted estimates, per-request exact counts

`ContextEntry.tokens` and `MemoryEntry.tokens` stay model-independent estimates
written at creation. They remain the history, the compaction-event totals, the
exported package values, and the estimator fallback. Everything that decides
or displays the *next request* recomputes with the conversation model's
counter:

- context projection (`project_context`, per entry plus framing),
- the group capacity check and the automatic compaction threshold,
- the compaction split (`compaction_split_by`),
- the router's history budget,
- the desktop's tool-schema segment.

Counts are recomputed per request and not cached. Entry content is immutable
after append, so a bounded cache keyed by `(entry_id, model)` is a safe
optimization if profiling shows a large branch makes the meter slow; none is
added here.

### 3. Every displayed total is labelled

`ContextProjection` carries `token_source` and `token_counter`. The desktop
context meter shows `Exact · o200k_base` or `Estimated from characters`, and
labels the memory segment as estimated whenever the context itself is exact,
because the next turn's memory block is still estimator-based. The meter never
presents a mixed total as though it came from one source.

### 4. Framing overhead is part of the count

`count_message` adds `MESSAGE_OVERHEAD_TOKENS` (4) per chat message. An entry's
cost includes it once, even when the entry carries tool calls. The system
prompt is one message, so it is counted with `count_message`; tool schemas are
not messages and are counted as text.

### 5. Provider-reported usage stays authoritative

`TokenUsage` recorded after a completion is unchanged. The counter governs
pre-request decisions and the meter; the two may differ slightly because
providers count their own framing.

## Alternatives considered

- **A per-model count column in the database.** Needs a migration, a package
  field, and invalidation whenever the model changes; the exact count of an
  imported branch would also be stale. Rejected.
- **An in-process cache keyed by `(entry_id, model)` now.** Safe in principle,
  but it adds a bounded-cache type and lifetime questions to the service before
  any profile shows the meter is slow. Deferred and documented as the upgrade
  path.
- **A provider-side token-count endpoint.** Adds network latency, is not
  available offline, and not every OpenAI-compatible endpoint implements it.
- **Bundle a Claude vocabulary like CatCup.** CatCup is AGPL-3.0-only and
  derives from SillyTavern (AGPL) and RisuAI (GPL), while CowBot is
  Apache-2.0. Copying its asset — or reproducing that derived vocabulary — is a
  licensing and maintenance risk. Rejected.
- **Tune the estimator constants.** Cheapest, but leaves the gap for code,
  JSON, CJK, and tool schemas, and keeps the meter unlabelled.

## Consequences

- For recognized OpenAI-compatible models the meter, threshold, split, and
  router budget are exact within provider framing differences. For every other
  model they remain estimates and say so.
- Switching to a recognized model changes the displayed totals and when
  automatic compaction fires, without rewriting history or packages.
- The service uses `compaction_split_by` with the model counter.
  `compaction_split` remains as the persisted-estimate wrapper for tests, CLI
  paths without model metadata, and callers that only need ordering.
- Context events keep the persisted estimate totals, because they are history
  and `CompactionOutcome` reports what the store wrote. The live meter can
  therefore show a slightly different before/after pair than the last
  compaction event.
- `gpt-oss` models are counted with the estimator even though `tiktoken-rs`
  exposes an `o200k_harmony` vocabulary: the harmony format adds special tokens
  CowBot never sends. Adding it later is a local change to the model map.
- Imported packages need no migration; their persisted estimates keep working
  as history and as the fallback for unknown models.
- Tests cover the counter contract and framing overhead, estimator fallback,
  model-name normalization, exact counts for known strings, exact-versus-
  estimated totals, entry costs with tool calls, a compaction split that
  changes with the counter, a usage state that changes with the counter, and
  the source fields in the service projection and the desktop view.
