# ADR 0039: Side questions as an ephemeral turn

- Status: Accepted
- Date: 2026-11-13
- Discussion: [#193](https://github.com/BerryUIKI/CowBot/issues/193). Builds on [ADR-0030](0030-conversation-sessions-and-context.md), [ADR-0038](0038-slash-commands.md).
- Supersedes: None.

## Context

Every composer send in CowBot has two effects: it posts a `ChannelMessage`, and
in a user-created conversation it appends a `ContextEntry` to the active branch
(ADR-0030). Both are deliberate — the timeline is the record, and the branch is
what the model sees.

They are also the wrong shape for a quick clarification. Asking an agent "what
does the compaction threshold mean?" and then continuing the real work leaves a
turn in the branch that the next request pays for, that the next compaction
summarises, and that the agent may reference as if it were part of the task.

The slash command surface (ADR-0038) has 15 actions, and all of them are pure UI
actions: they never post, never append. A side question is the first command
that needs to *run a turn* without touching either record.

## Decision

### 1. `/btw <question>` runs one turn outside the conversation

`ChannelAgentService::ask_side_question` assembles a request from the agent
system prompt plus a side-question instruction and calls
`AgentInstance::execute_messages`, the same entry point a conversation turn
uses. It deliberately skips every write the normal path performs:

- no `ChannelMessage` is posted, so the timeline, the sidebar preview, and the
  unread state are untouched;
- no `ContextEntry` is appended, so the branch — and therefore the next turn's
  token count, the next compaction, and the agent's memory of the session — is
  identical before and after;
- no `ConversationTurn` row is written, so routing history shows nothing.

The answer returns to the caller as a `SideAnswer`. The desktop renders it in a
`SideAnswerPanel` above the composer, which is dismissed or replaced, never
reloaded. Nothing about the exchange survives the panel.

### 2. The agent loop is reused, not reimplemented

Because the call goes through `AgentInstance`, a side question gets the same
tool registry, the same approval gate, and the same usage recording as any other
turn. A side question that reads or edits a workspace still asks for approval;
it simply leaves no conversation artefact behind.

The gate is scoped to a synthetic assignment id derived from the channel, so an
approval card still names the chat it came from even though no assignment row
exists.

### 3. Usage is recorded, the content is not

Each side question writes one `llm_calls` row tagged
`LlmCallKind::SideQuestion`, carrying the channel and the normalized usage. The
cost stays visible in Settings → Audit, which is the point: an unrecorded call
would make the cheapest interaction invisible to the person paying for it.

Prompt snapshots are *not* captured for a side question. A snapshot exists so an
operator can inspect what produced a visible turn; a side answer produces no
turn, and storing its prompt would recreate, in the audit log, exactly the
record the feature exists to avoid.

### 4. One responder, chosen deterministically

A side answer has one voice. Resolution order:

1. an explicit `agent_id` from the client;
2. a single `@mention` in the question;
3. the conversation's stored default responder, or its only participant;
4. a direct conversation's agent;
5. otherwise a notice that asks the user to mention an agent.

A question that mentions two or more agents is refused rather than fanned out —
two answers to one side question is not the interaction anyone asked for.

The mention that selected the responder is stripped from the request, so the
model answers the question rather than addressing itself. A mention in the
middle of the question is left alone, because it may be part of what is asked.

### 5. The model comes from the conversation, or from the caller

A user-created conversation counts against a fixed model, so its settings win.
An agent chat and a group channel have no stored model, so the desktop supplies
the one the chat is using. A side question on a group channel therefore runs on
the composer's current model rather than a participant's.

## Alternatives considered

**Post the answer as a system message.** Rejected: it fills the timeline with an
exchange the user explicitly asked to keep to the side, and a system message is
still a `ChannelMessage` in the preview.

**Append to the branch but mark the entries excluded.** Rejected: the branch is
append-only and compaction reads it; an excluded entry is still a row, still
exported in a package, and still a second concept the user has to understand.

**A separate short-lived conversation per question.** Rejected: it creates a
channel (and a sidebar row) for something that should leave no trace, and the
cleanup would have to be invented.

**Route through the scheduler as a task.** Rejected: the scheduler's job is to
own assignments and persist their outcome. Using it for a call that must not
persist anything would mean adding a "do not persist" mode to the one component
whose contract is to persist.

## Consequences

- The `llm_calls` table gains a sixth call kind; readers that enumerate kinds
  must handle it, and unknown kinds must keep failing soft.
- A side question cannot use conversation history. That is the point, but it
  means the question has to stand alone; the panel says so in a footnote.
- Tools are available, so a side question can change the workspace while
  leaving no conversation record. Approval is the guardrail, not history.
- The feature deliberately has no history, no recall, and no export. Adding any
  of them would turn it back into the thing it avoids.
- `cowbot-client` exposes `ask_side_question` as a local-only method for the
  same reason as `reply_to_mentions`: the LLM handle lives in the desktop
  runtime, not in the workspace database.
