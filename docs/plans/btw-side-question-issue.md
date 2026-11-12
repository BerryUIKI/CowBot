# Slash commands: add `/btw` for a side question that does not change context

Priority: P2

Depends on #185, #186, #187.

## Problem

The composer has no way to ask an agent something without that exchange becoming part of the working history. In Claude Code, `/btw` asks a question to the side: the answer is shown to the user and the question never enters the main transcript. CowBot has no equivalent. Every composer send posts a channel message and, in a conversation, appends a `ContextEntry` to the active branch (ADR-0030), so a quick clarification permanently shifts what the next turn sees and what compaction later summarises.

## Answer to "does CowBot support `/btw` today?"

**No.** Verified rather than assumed:

- `grep -rni '\bbtw\b'` over the whole repository returns nothing — no code, docs, ADR, or plan.
- `crates/cowbot-core/src/command.rs` (from #185) catalogues 15 commands; `btw` is not one of them.
- No ADR describes a side-question path. The closest behaviour is an agent conversation turn, and it has the opposite semantics: it posts a user message, posts the reply, and mirrors both into the branch.

So `/btw` needs to be designed, not merely added to the catalogue.

## Why this is not just another catalogue entry

The 15 commands in #185 are UI actions: they never become messages and never enter a context branch. `/btw` is different — it runs a turn, so it needs a delivery path that is neither a display message nor a context entry.

- Posting the answer as an ordinary channel message would show it in the transcript and refresh the sidebar preview. The user asked for a side answer, so it needs an ephemeral surface: a dismissible panel above the composer.
- The question itself must never be appended to the branch, or the next turn and the next compaction would see it.

## Proposed behaviour

- `/btw <question>` runs one agent turn through `execute_messages` with a caller-assembled request: the agent system prompt plus the question, **without** projecting the conversation branch. `cowbot-agent` already exposes `execute_messages` for caller-assembled requests (ADR-0030), so no agent-crate change is expected.
- Nothing is persisted: no `ChannelMessage`, no `ContextEntry`, no `ConversationTurn`, no sidebar preview update. The answer renders in a dismissible panel above the composer and is lost on dismiss or chat switch.
- The answer still records LLM usage so cost stays visible in Settings → Audit. Whether it reuses the existing channel attribution or adds a `LlmCallKind::SideQuestion` is a design choice for the ADR. The prompt snapshot follows the existing capture switch (ADR-0037).
- `/btw` requires the question: `/btw` alone inserts `/btw ` and waits, like `/task`.
- Group conversations: `/btw` addresses the default responder, or the single `@mentioned` agent. Mentioning several agents is rejected with an explanatory notice rather than fanning out.
- Because nothing is appended, `/btw` does not move the context meter; it must still respect the model's usable window and is refused with the normal over-budget notice when the assembled question alone exceeds it.

## Implementation sketch

1. **Core.** Add `btw` to `crates/cowbot-core/src/command.rs`: usage `/btw <question>`, one required `Text` argument, target `Chat`. Update the catalogue test that asserts the v1 name list (currently hard-coded in `catalogue_is_alphabetical_and_covers_v1`).
2. **Agent/service.** A service method that assembles the system prompt plus the question and calls `execute_messages` without a `ConversationRecorder`. If the conversation path is reused, pass a no-op recorder and skip the post-message and turn-row writes rather than adding a parallel code path.
3. **Tauri.** A command such as `ask_side_question(channel_id, agent_id, question)` returning the answer plus its usage summary. It must not call `post_message`.
4. **Desktop.** A side-answer panel above the composer and a handler in `runCommand` (the seam #186 leaves). The menu row comes for free from the catalogue.
5. **Docs.** An ADR numbered after the slash-command ADR — the slash-command ADR will be 0038 (0034 is compaction concurrency, 0037 is prompt snapshots), so `/btw` is **0039**. It covers the delivery path, the deliberate omission of persistence, and the interaction with prompt snapshots and usage accounting.

## Tests

- Core: `/btw` without a question is `MissingArgument`; with one it is an invocation carrying `raw_args`.
- Service: a side question posts no channel message, appends no context entry, creates no turn row, and leaves the conversation context identical before and after.
- Service: usage is still recorded and attributed.
- Desktop: the answer panel renders and dismisses; switching chats clears it.

## Out of scope

- Side questions that spawn tasks or assignments.
- Persisting side answers beyond the existing prompt snapshot.
- Several agents answering one side question.
- A side-answer history.
