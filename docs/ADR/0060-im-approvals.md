# ADR 0060: Approvals and attachments over IM

- Status: Accepted
- Date: 2026-11-24
- Related: [ADR-0041](0041-turn-streaming-and-cancellation.md),
  [ADR-0055](0055-server-side-turn-execution.md),
  [ADR-0056](0056-im-integrations.md),
  [ADR-0057](0057-mattermost-adapter.md),
  [ADR-0058](0058-telegram-adapter.md),
  [ADR-0059](0059-slack-adapter.md),
  [ADR-0061](0061-feishu-adapter.md)
- Discussion: `S5` and `S7` of the [P3 plan](../plans/P3-im-integrations.md).

## Context

Two stages of the integration plan are about something other than text.

**S5 — approvals.** An agent that wants to run a tool above the policy's
threshold needs a person. In the desktop that person gets a card. In an IM they
get nothing, so the turn sits in `ApprovalService::wait_for_decision` until it
expires and is denied. The plan described this as "the most invasive part of the
whole feature", expecting an async approval wait and a pending-resolver handle in
`cowbot-service`.

**S7 — attachments.** Somebody sends a photo or a CSV. The message arrives with
no text, or with text that refers to something the agent cannot see.

## Decisions

### 1. The approval wait does not change

Investigation found the invasive change unnecessary.
`ApprovalService::authorize` publishes `ApprovalRequested`, then re-reads the
approvals row every 200 ms until it is decided or the deadline passes
(`crates/cowbot-service/src/approvals.rs`). The decision path is
`ApprovalService::decide`, which updates the row and publishes `ApprovalDecided`.

So an adapter needs **no resolver handle, no rewired wait, and no new trait in
`cowbot-core`**. It calls `decide` from its own inbound loop and the waiting turn
sees the result within one tick. A process-global `Notify` registry was rejected:
`ApprovalService` is constructed per gate and per request, so a resolver handle
would have to be threaded through four constructors to save at most 200
milliseconds.

The plan's other shape — the `ToolGate` calls `Integration::ask_approval` — was
rejected for a structural reason: `cowbot-integrations` depends on
`cowbot-service` and never the reverse, so it would need a new trait in
`cowbot-core` and an HTTP round trip inside the authorization critical path.

### 2. The mirror carries approvals, not the gate

`cowbot-integrations`' `Mirror` already subscribes to the bus for
`MessagePosted`. It now also reads `ApprovalRequested` and `ApprovalDecided`:

- a request whose task belongs to a channel **this integration is bound to** is
  posted as a card (`approvals::prompt_for` decides that, so no adapter can post
  a card into a room it does not serve);
- a decision revises the card the mirror posted, which it remembers in a small
  in-memory map keyed by approval id.

The map is memory because a card is a message in a chat: a restart cannot revise
one it has forgotten, which is the same limitation a platform without an edit call
has permanently.

### 3. The tool call travels on the event

`ApprovalRequested` carried only `approval.tool_call_id`, not the call. The
desktop closes that gap in its frontend by caching `ToolCallRequested`, and the
store cannot help: `tool_calls` rows are written only *after* execution, so no row
exists while the approval is pending. An adapter subscribes from a different task
and a lagged subscriber drops events, which would leave a card naming a bare id.

`ApprovalRequested` therefore gained an optional `call: Option<ToolCall>`,
defaulted so an older event still parses and every existing consumer (which
matches with `..`) is unchanged. This is the only change to `cowbot-core` this
whole milestone needed.

### 4. Any IM participant could approve, so the decider is checked

**This is the decision that makes an IM approval worth anything.**
`Role::can_write()` is `true` for every role, and `resolve_principal` provisions
every first-time IM author as `Member`. A card sits in a shared room. Without a
check, one person's `execute_code` request could be approved by anybody present —
which turns a prompt injection into a one-tap compromise.

`approvals::may_decide` allows the decision for:

- the account that asked, resolved through the task's assignments; and
- a workspace administrator (`Role::can_administer`).

Everyone else gets `Refused::NotAllowed`, which each adapter shows to the person
who tapped: a Telegram toast, a Slack log line and an unchanged card, a Feishu
toast.

The decision is attributed to the **CowBot `UserId`** of the IM principal
(`bridge:{platform}:{integration}:{external}`), not to its display name, because
`tool_calls.approved_by` and the audit actor are joined against `users.id` by
every existing caller. The cost is that the audit view renders the bridge id; the
name is on the bridge user row for a later UI change.

### 5. The server's approval timeout is fifteen minutes, and configurable

`ApprovalService::new` hard-codes 300 seconds and `with_timeout` had **no
production caller**. Five minutes is right for a desktop — the card is on screen —
and wrong for a phone. `COWBOT_SERVER_APPROVAL_TIMEOUT_SECS` sets it, defaulting
to 900, and it is threaded through `ChannelTurnOptions` and `CrewRunOptions` so
the desktop and CLI keep their own default.

### 6. The server expires approvals left pending by an earlier process

`reconcile_on_startup` existed and was called only by the desktop's
`LocalClient::new`. The server never called it, so a row whose waiting turn died
stayed `pending` — its card stayed tappable, and a tap would "succeed" against a
turn nobody was waiting for. The server now reconciles every workspace at
startup.

### 7. Attachments are stored under the tool root and named in the text, and that
is all a model gets

**A model in this build cannot be sent an image.** `ChatMessage.content` is a
`String` in `cowbot-llm`, and all three protocol encoders send it as a bare JSON
string, so there is no content-part type to put a picture into. What a turn *can*
do is run a tool. So an attachment is written below the deployment's tool root
and its **relative path** is appended to the message text:

```
Files sent with this message:
[file] rows.csv: attachments/i1/F1-rows.csv (text/csv)
```

An agent reads a text file with `read_file` and inspects any other with
`execute_code`. A whiteboard photo is stored and named, not understood. Image
understanding is a change to the LLM type layer and all three encoders, and it is
not this milestone. `docs/LIMITATIONS.md` says so in the operator's words.

### 8. The file name is generated, the size is capped while reading, and the
fetch happens after the dedupe claim

Three rules, each for a specific failure:

- **The stored name is generated** — the platform's file id, then the sender's
  name reduced to `[A-Za-z0-9._-]`, with `.`/`..`/empty refused. A name that
  reached a path could escape the root, and `../../etc/passwd` is the first thing
  anyone tries. The write uses `create_new`, so an attachment never replaces a
  file a previous turn wrote.
- **The size cap is passed into the download** (`MAX_ATTACHMENT_BYTES` = 20 MB,
  Telegram's own bot-download ceiling) rather than checked afterwards, because
  buffering a two-gigabyte file to reject it is the denial of service the cap
  exists to prevent. The platform's declared size is checked first, so an
  obviously oversized file costs no bandwidth.
- **Fetching happens after `claim_integration_event`**, so a platform retry
  downloads nothing. The order is in `inbound::ingest` and is the reason
  `store_all` is called there rather than in the adapter.

The pipeline re-checks the returned length, and after the write it canonicalizes
the path and refuses one that resolved outside the root — the belt to the
traversal braces, because a symlinked directory an earlier `execute_code` turn
created would otherwise let a write land elsewhere.

A file that cannot be stored is named as **not kept** and the message is still
posted: a question with a broken attachment is still a question.

### 9. Outbound attachments are out of scope, and the reason is the trigger

An agent's answer is text. There is no rule for *which* of the files a turn wrote
is the deliverable — a tool-argument heuristic is wrong and guessing is worse —
and `OutboundContent` carries only text. Sending files back needs an artifact
model, an upload call on all four adapters, and a per-platform size and type
policy. The hard part is the trigger, not the upload, so it is a follow-up rather
than a half-built feature.

## Alternatives considered

See §1 for the two rejected approval shapes, and §9 for outbound. Two more:

- **A capability flag `supports_approvals()` next to `post_approval`.** Rejected
  as a second mechanism for one fact: `ApprovalPosting { buttons }` carries the
  answer and its consequence in the value, which is the shape
  `BindingHealth::Unknown` established for `check_binding`.
- **Polling `list_pending_approvals_for_workspace` from the adapter instead of
  reading the bus.** Rejected: the mirror already subscribes, so the event is
  free, and polling would race the desktop's own card.
- **Attributing the decision to the display name** so the audit view reads
  nicely. Rejected: the trail joins on `users.id`, and the name is recoverable
  from the bridge user row.

## Consequences

- **A card sits in the room after the decision.** It is revised to say who
  decided, and the buttons are gone. On a platform with no edit call this does not
  happen, and a late tap fails at `decide` with "not pending" — which the adapter
  reports to the tapper.
- **Two integrations bound to one channel each post a card.** The first tap wins;
  the second fails the store's `status = 'pending'` guard and is reported. No
  deduplication is attempted, because the store's guard already makes it safe.
- **An attachment is readable by any turn on the same server.** The tool root is
  per deployment, not per workspace, and the file tools are rooted rather than
  confined. This is documented rather than fixed: making the tools confine an
  agent is a sandbox change, not an attachment change.
- **A refused decision is invisible in Slack.** Slack's interaction socket has no
  channel for a toast without an inbound request to answer, so the refusal is
  logged and the card is left unchanged. Telegram and Feishu show it.
- **The eviction of an approval from `Mirror`'s card map is implicit**: a decision
  removes the entry, and a card whose approval is never decided stays until the
  process ends. The map is small — one entry per pending approval this
  integration posted.
