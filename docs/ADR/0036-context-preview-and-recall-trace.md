# ADR 0036: Context preview and recall trace

- Status: Accepted
- Date: 2026-11-12
- Discussion: [#177](https://github.com/BerryUIKI/CowBot/issues/177)
- Supersedes: None. Refines [ADR-0030](0030-conversation-sessions-and-context.md), [ADR-0032](0032-group-conversations-and-routing.md), [ADR-0033](0033-agent-memory-and-shared-notes.md), and [ADR-0035](0035-model-aware-token-counting.md).

## Context

The context meter showed four buckets — system, tools, history, and the latest
message — plus one memory estimate. The memory recall preview returned only the
rendered block and the selected entry ids. Neither could answer the questions a
governed or debugging user asks:

- Which memory candidates were considered, which were selected, and why were
  the rest skipped?
- Which parts of the system prompt — agent profile, platform context,
  participant roster, default responder, peer convention — consume the budget?
- Which messages and tool schemas will be sent, in order?
- Why did an agent ignore a fact that exists in workspace memory?

Rebuilding the request in the desktop would duplicate the service's assembly
logic and drift from what the reply path actually sends.

The behavioral reference is CatCup Tavern's context inspector and world-info
trace at commit `0223205`. CatCup is AGPL-3.0-only; this decision reimplements
the idea independently in Rust and TypeScript and copies no code, comments, or
test fixtures.

## Decision

### 1. `PromptPlan` is a read-only value

`cowbot-core::prompt` defines `PromptKind`, `PromptRole`, `PromptSlotName`,
`PromptSlot`, `PromptMessage`, `PromptTool`, `PromptMemory`, and `PromptPlan`.
A plan lists the named slots, every message in send order with its token cost,
the memory block, the tool schemas, the context projection, the model, and the
compaction settings. It is serializable and owns no store handle, so the
desktop renders it directly and a future remote client can return it unchanged.

`PromptTool::plan` prices each serialized schema and the serialized array; the
array count is what the request actually sends, so it matches the projection's
tool segment. `PromptPlan::request_tokens` adds the memory message to the
projection total.

### 2. System prompts are built from named slots

`conversation_system_prompt` and `group_system_prompt` build their text through
a slot writer. Slots are `agent`, `platform`, `roster`, `default_responder`,
`peer_convention` (group) and `agent`, `platform`, `conversation_convention`
(single). Each slot's token count is the difference between the counted
prefixes, so the slots add up exactly to the token count of the prompt text.
The system message costs message framing on top. The assembled prompt text is
unchanged, so this is a refactor of how it is described, not a prompt rewrite.

### 3. Recall selection returns a trace

`cowbot-core::memory::plan_recalled_entries` returns a `RecallPlan` with every
ranked candidate, its rendered-bullet cost, whether it was selected, and a
`RecallSkipReason` (`inactive` or `over_budget { remaining_tokens }`).
`select_recalled_entries` delegates to it, so which entries are injected does
not change. `MemoryService::preview_recall_trace` exposes the trace together
with the rendered block, both budgets, and the notes' estimated share.

The trace is computed without the recall-statistics write: previewing never
increments `recall_count` or sets `last_recalled_at`.

### 4. Conversation previews rebuild the next request

`PreviewService::conversation` loads the participant, its branch, settings, and
the workspace agents, and assembles the same system prompt and messages the
reply path would send. A draft message passed by the caller is added to the
projection and to the end of the message list, matching the reply path's
append-before-assemble order, and is never persisted. Memory is recalled for
the draft (or for an empty query when there is no draft). The platform slot
uses the caller's extra environment text, which group sends include and single
conversations do not. When the turn runs in Plan mode, the preview adds the
same instruction message the agent runtime inserts after the system prompt.

### 5. Task-run previews rebuild the first iteration

Every run stores the agent profile that executed it, because the standard
Planner/Coder/Reviewer crew agents are transient profiles that are never
written to the agent table; without the snapshot a crew run could not name its
agent. `PreviewService::task_run` uses that snapshot (falling back to the
stored agent for older runs), the model recorded on the run's first LLM call
(falling back to the caller's model), the optional Plan-mode instruction, the
memory recall preview, the task description and expected output, prerequisite
task outputs, and the approved plan when applicable.

Runs that executed a conversation or channel turn are not rebuilt: their actual
request came from the conversation or channel path, not from a task prompt, so
the service returns an explanatory error and the desktop hides the action for
those runs. Group turns are tagged `ConversationTurn` like single turns, so the
task list hides them and the preview can tell them apart.

Prompts are not persisted per iteration (that is issue #178), so later
iterations are not shown, and the task's *current* plan and memory settings
shape the reconstruction even for an older run. Runs do not record the
assignment that produced them; the Plan-mode instruction is included when the
assignment for the same task and agent created latest before the run started
ran in Plan mode. The UI labels the preview as rebuilt.

### 6. The desktop renders the preview in the right inspector

The existing right inspector gains a Context tab with Prompt, Recall, and
Budget sub-tabs and an over-budget state. The context popover's **Inspect**
action opens it for the active participant with the composer's current draft;
each group participant can be inspected from its own tab. A task run's
**Inspect context** action opens the same tab. The desktop never assembles a
prompt: it renders `PromptPlan` and only adds window metadata the backend
resolves from the provider configuration.

## Alternatives considered

- **More aggregate buckets on `ContextProjection`.** Cheap, but it still cannot
  answer "why was this memory entry skipped" or "which prompt part is large".
- **Rebuild the prompt in the desktop from existing client methods.** Duplicates
  assembly logic and drifts from the reply path. Rejected.
- **Persist every prompt and inspect history only.** Best for audit, but it is
  the separate audit issue (#178) and too heavy for a preview. Deferred, and
  the task-run preview documents that historical iterations are unavailable
  until then.
- **Match each run to its assignment by adding a `run_id` column.** Exact for
  Plan-mode attribution, but it adds a relation for one preview attribute. The
  run's agent-profile snapshot is the only schema addition this decision needs,
  because transient crew agents would otherwise have no stored identity; the
  assignment is matched by task, agent, and creation order instead.
- **Recount slots by each slot's content alone.** Simpler, but the parts do not
  add up to the whole after tokenization boundaries. Prefix differences keep
  the breakdown exact.

## Consequences

- Users can see the exact next request for a conversation participant,
  including a pending draft, and the recall decision behind its memory block.
- The memory trace makes an ignored fact explainable: inactive entries and
  entries that did not fit the remaining budget are listed with their cost.
- Previews are read-only. No context entry, recall statistic, or model call is
  written; tests assert the branch length and recall counters are unchanged.
- Task-run previews cover the initial request only. After #178 persists prompt
  snapshots, the preview can show each iteration's actual request and the plan
  and memory state that applied at run time instead of a reconstruction.
- Runs now carry an optional agent profile snapshot in storage and in
  `runs.jsonl`; older databases gain the column through the migration, and
  older packages import without it, falling back to the stored agent.
- The estimator still decides which memory entries fit, because persisted entry
  costs are model-independent (ADR-0035). The preview shows the exact cost of
  the resulting block as a message, and the recall tab labels the selection
  budget separately.
- `PreparedConversationTurn` and the reply path are unchanged; the preview uses
  the same prompt builders and message conversion.
- Tests cover slot accounting, prefix sums, the recall trace's selected,
  inactive, and over-budget paths, preview non-mutation, group roster slots,
  task-run reconstruction with and without Plan mode, a transient crew agent
  resolved from the run snapshot, the recorded run model, conversation-turn
  refusal, the client boundary, and the desktop view's over-budget and
  unknown-window states.
