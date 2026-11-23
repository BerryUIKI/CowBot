# P3 — IM integrations (research and proposal)

- Status: **Built.** S0 (server-side turns) is
  [ADR-0055](../ADR/0055-server-side-turn-execution.md); S1 (the shared adapter
  layer) is [ADR-0056](../ADR/0056-im-integrations.md); S2 (Mattermost) is
  [ADR-0057](../ADR/0057-mattermost-adapter.md); S3 (Telegram) is
  [ADR-0058](../ADR/0058-telegram-adapter.md); S4 (Slack) is
  [ADR-0059](../ADR/0059-slack-adapter.md); S5 and S7 (approvals and
  attachments) are [ADR-0060](../ADR/0060-im-approvals.md); S6 (Feishu) is
  [ADR-0061](../ADR/0061-feishu-adapter.md).

  Three things the plan did not anticipate, all found while building it:

  * S3 promoted the binding-health notice loop and the `(2/3)` chunk label into
    the shared layer, and added a UTF-16 splitter for a platform that measures
    its limit that way.
  * **S5 needed no change to the approval wait.** The service already polls the
    row and `decide` already wakes it, so the adapter calls `decide` and the turn
    notices within 200 ms. The plan's "pending-resolver handle" is unnecessary;
    the one `cowbot-core` change is an optional `call` field on
    `ApprovalRequested`.
  * **S6 needed no crypto.** Feishu's long connection removes the Encrypt Key,
    the HMAC, and the `url_verification` deadline in one step, exactly as long
    polling removed Telegram's webhook. What S6 does need is a hand-written
    protobuf codec.
- Date: 2026-11-20
- Related: [ADR-0043](../ADR/0043-self-hosted-server-auth.md),
  [ADR-0045](../ADR/0045-collaboration-api-and-remote-client.md),
  [ADR-0053](../ADR/0053-desktop-server-mode.md),
  [ADR-0022](../ADR/0022-channel-mention-turns.md),
  [ADR-0041](../ADR/0041-turn-streaming-and-cancellation.md),
  [Roadmap Phase 3 workstream 3.3](../ROADMAP.md)
- Follow-ups: server-side turn execution (blocking, see §3); IM approval actions;
  attachment ingestion.

## 1. Goal

Let a user talk to a CowBot agent from an existing IM (Mattermost, Slack,
Feishu), and let the agent's answers appear back in that IM — without a second
cowbot UI. The roadmap names Mattermost, Slack, and Feishu as the first three
adapters; Telegram was added during this research because it is cheaper than all
three and maps onto the existing direct/mention split almost exactly (§4.1).

### Non-goals

- Replacing the IM. CowBot stays the workspace of record.
- Federation between workspaces. One integration maps to one channel in one
  workspace.
- Voice, video, or presence.

## 2. What the seam looks like today

The good news is that the model already fits an external chat surface closely.
Channels, messages, threads, mentions, and turns all exist:

| Need | Existing piece | Location |
| --- | --- | --- |
| A conversation to bind to | `Channel` with `ChannelKind::{Channel, Direct, Conversation}` | `crates/cowbot-core/src/conversation.rs:27` |
| A message with an author | `ChannelMessage { sender: Sender::{Agent, User, System} }` | `crates/cowbot-core/src/channel.rs:32,114` |
| Threading | `ChannelMessage.thread_id: Option<String>` | `crates/cowbot-core/src/channel.rs:119` |
| Turning `@name` into agent work | `parse_mentions` / `is_broadcast_mention` | `crates/cowbot-core/src/mention.rs` |
| Running the turn | `ChannelService::post_thread_message` then `ChannelAgentService::reply` | `crates/cowbot-service/src/channels.rs:242`, `channel_agents.rs` |
| Live event feed | `EventBus` + workspace WebSocket `GET /api/v1/workspaces/{id}/stream` | `crates/cowbot-bus`, `crates/cowbot-server/src/stream.rs` |
| Credential for a bot | API tokens, workspace-scoped, owner-bound | `crates/cowbot-server/src/tokens.rs` |
| Approvals | `Approval` + `ApprovalService`, process-local `SessionDecisions` | `crates/cowbot-core/src/approval.rs:103` |

Four things are missing or wrong for an IM client, and they are not equally
difficult.

### 2.1 Sender identity has no external principal

`Sender::User(String)` carries a CowBot `UserId`. The server hard-codes
`Sender::User(user.user.id.0)` and ignores any body field, so a caller cannot
post as someone else — correct for browsers, wrong for a bridge that must
attribute an IM message to the *IM* user who wrote it.

An IM tenant has its own user directory. Options:

1. **Synthetic users.** Provision a CowBot `User` per external IM user on first
   sight (email unavailable, so `name` from the profile, `email` derived as
   `{platform}:{external_id}@bridge.local`). Reuses every existing path; costs a
   user row and makes sign-in impossible for those rows, which is fine — they
   have no password.
2. **A new `Sender` variant.** `Sender::External { platform, external_id,
   display_name }`. Honest, but touches every match on `Sender` (there are
   several: `channel_agents.rs:323`, `:657`, `:772`, and the desktop renderer),
   changes the bindings, and does not give the approval audit a name to show.
3. **A principal mapping table only.** Keep `Sender::User`, add
   `integration_principals(workspace, integration_id, external_id, user_id)`.

Recommendation: **option 1 plus the mapping table from option 3**. Provisioning
a `User` lets approvals, audit, and usage all attribute to a real row with no
type changes; the mapping table makes lookup reversible and lets an operator
pre-bind a known IM handle to an existing member.

### 2.2 Mentions are provider-specific

`parse_mentions` accepts `@` followed by letters, digits, `_`, `-`. Slack sends
`<@U024BE7LH>`; Feishu sends `@_user_1` plus a `mentions` array in the event
body naming the id and display name. The adapter, not the core, must translate
to a plain `@AgentName` before the message is stored — otherwise the message
text in the CowBot timeline is full of opaque ids.

### 2.3 Messages have no external id

Slack retries an event up to three times (`X-Slack-Retry-Num`), and Feishu
retries on any non-2xx or timeout. Without deduplication, one IM message can
produce two agent turns. `ChannelMessage.id` is a generated UUID with nowhere to
record the source event.

### 2.4 A turn cannot run on the server

**This is the blocking constraint, and it is worth stating plainly.** The server
has no LLM client at all — `crates/cowbot-server/Cargo.toml` does not depend on
`cowbot-llm`, and nothing in `crates/cowbot-server/src` references
`LlmProvider`. The desktop's send path says so in a comment
(`desktop/src-tauri/src/commands.rs`):

```rust
// The reply runs in-process, so server mode posts without a local turn.
let Some(client) = state.local_client.clone() else {
    return Ok(message);
};
```

Consequently, a user signed in to a server can post a message that mentions an
agent and **nothing answers**. Model providers live in the desktop's
`app_data_dir()/config.json` with keys that never cross to the frontend
(`desktop/src-tauri/src/config.rs`), so the server has no provider to call even
in principle.

IM requires the opposite of the current topology: a bot answers a message at
3 a.m. with nobody's desktop open. **No IM adapter can be useful until the
server can run a turn.**

## 3. The prerequisite: server-side turn execution

The service layer is already close. `ChannelAgentService::reply` takes
`Arc<dyn LlmProvider>`, a `ToolRegistry`, and a `&str` model
(`crates/cowbot-service/src/channel_agents.rs`), and the CLI constructs all
three itself (`crates/cowbot-cli/src/main.rs:561-590`). So the work is not in
the turn logic; it is in giving the server a provider and a place to run.

Minimum viable form:

1. Server provider configuration. Either environment variables
   (`COWBOT_SERVER_LLM_PROTOCOL`, `_BASE_URL`, `_API_KEY`, `_MODEL`) for a
   single-provider deployment, or a workspace-scoped table for parity with the
   desktop. Env-only is defensible for milestone one and mirrors how
   `crates/cowbot-server/src/config.rs` already treats deployment settings.
2. After `post_message`, spawn the same turn the desktop spawns, using the
   server's `AppState.store`, `bus`, `session_decisions`, and approvals. The
   turn publishes `MessagePosted` for each reply, which the existing workspace
   WebSocket already forwards — so clients see the answer with no new protocol.
3. Decide what `POST /messages` returns. Leaving it as-is (the message, then
   events) keeps `RemoteClient` compatible and matches what the desktop already
   does in local mode: reply in the background.
4. Pick a `ToolRegistry` for the server. The subprocess sandbox runs on the
   server host — that is a real security decision for a shared deployment, and
   likely wants a `ReadFile`/`WriteFile`-free profile at first.

Until step 1 and 2 exist, the honest alternative is a **desktop-hosted bridge**:
a local process that opens `LocalClient` on the same database and runs turns
(feasible because `reply_to_mentions` is `LocalClient`-only and takes the LLM
handles). It works for a single always-on machine on a home network. It does
not work for the team-mode deployment the roadmap puts this workstream behind,
and it cannot see messages a *remote* user posts, because those never reach the
local database. Treat it as a demo, not a milestone.

## 4. Platform survey

Everything below is from the published protocols; the official docs could not be
fetched from this environment (`open.feishu.cn` and `api.slack.com` were blocked),
so the exact field names should be checked against the current docs before
implementation.

| | Mattermost | Slack | Feishu / Lark | Telegram |
| --- | --- | --- | --- | --- |
| Deployment | Self-hosted, same audience as `cowbot-server` | SaaS | SaaS | SaaS, or a self-hosted Bot API server |
| Inbound transport | WebSocket from a bot account (`wss://host/api/v4/websocket`, auth challenge) — **no public URL needed** | Events API over HTTPS, or Socket Mode over WebSocket (`xapp-` app token) | HTTPS request URL only | `getUpdates` long polling, or `setWebhook` |
| Inbound auth | Bot session token | HMAC-SHA256 over `v0:{timestamp}:{raw_body}` with signing secret; reject timestamps older than 5 min | `Verification Token` equality, plus optional `Encrypt Key`: AES-256-CBC, key = SHA-256(encrypt key), IV = first 16 bytes, PKCS#7 padding | Token in the URL path. Long polling needs nothing else; a webhook adds `secret_token` echoed in `X-Telegram-Bot-Api-Secret-Token` — **no HMAC, no decryption** |
| Handshake | None | `url_verification` challenge | `url_verification` → echo `{"challenge": ...}` in plaintext even when the request was encrypted | None |
| Ack deadline | N/A (socket) | 3 s | 3 s | N/A for polling; a webhook should answer 200 promptly |
| Retries | N/A | up to 3, `X-Slack-Retry-Num` | on non-2xx/timeout | `getUpdates` confirms by advancing `offset`; a retried request can replay updates, so dedupe on `update_id` |
| Mention syntax | `@username` (plain) | `<@U123>` | `<at user_id="ou_...">name</at>` + `mentions` array | `@username` as a `mention` entity; `text_mention` carries the `user` object |
| Threading | `root_id` | `thread_ts` | `thread_id` / `parent_id` | `message_thread_id` (forum topics only), else `reply_to_message` |
| Outbound | REST `POST /api/v4/posts`, bot token | `chat.postMessage`, `chat.update` for edits | `im/v1/messages`, `PATCH` card for updates | `sendMessage`, `editMessageText` for edits |
| Interactive buttons | Interactive message actions, signed payload posted back | Block Kit actions → `block_actions` interaction over the same endpoint | Message cards with callback actions | Inline keyboard → `callback_query`, answered with `answerCallbackQuery` |
| Rate limits | Server-defined | Tiered per method (~1 msg/s per channel for chat.postMessage) | Per-app quotas | ~30 messages/s overall, ~20/min per group, `429` carries `retry_after` |

### 4.1 Reading of this table

- **Mattermost is the natural first adapter.** It is self-hosted like the
  server, the bot connects *out*, so no reverse proxy, no public DNS, and no
  signature verification is needed — a real reduction in attack surface for the
  deployment CowBot actually targets. It also mirrors CowBot's own concepts
  almost one-to-one (channels, threads, posts), which makes it the cheapest
  correctness test of the adapter design.
- **Telegram is the cheapest protocol and the best second adapter.** It needs no
  public ingress, no HMAC, and no decryption, and its default privacy mode is
  already the mention-driven turn the core implements. If Mattermost proves the
  adapter design against a self-hosted system, Telegram proves it against a SaaS
  one for a fraction of Slack's work — and it is the natural adapter for anyone
  who wants a bot without running a server at all.
- **Slack Socket Mode is the right transport** when Slack support arrives: it
  removes the public endpoint while keeping the same event payloads. The Events
  API then becomes a deployment choice, not a second implementation.
- **Feishu is the highest-value non-English market** and the one that forces the
  crypto work (AES decryption) and the hardest mention translation. Schedule it
  after the adapter interface has been proven on Mattermost and Telegram.

### 4.2 Telegram specifics that change the design

Telegram is the cheapest protocol of the four, and three of its quirks map
unusually well onto what CowBot already models.

**1. Long polling removes ingress entirely.** `getUpdates` with an `offset` and a
long `timeout` is a plain HTTPS call the server makes outward. No public URL, no
reverse proxy, no TLS certificate, no signature verification, no replay window —
the bot token in the URL path is the whole authentication story. It exercises the
same dial-out shape as a Mattermost WebSocket while being simpler to implement
(a request/response loop rather than a framed socket), which makes it the
cheapest place to prove the transport half of the adapter trait.

**2. Privacy mode matches the routing model exactly.** By default a bot in a
group receives only messages that mention it, are commands, or reply to it —
which *is* ADR-0022's mention-driven turn. An operator who wants the bot to see
everything must make it an admin or disable privacy mode; that is a deliberate
switch rather than the default. And a message in a private chat with the bot
needs no mention at all, which is precisely `ChannelKind::Direct` (ADR-0024).
So the two Telegram surfaces map onto the two CowBot modes already built, with
no new routing concept.

**3. Callback data is capped at 64 bytes.** An inline keyboard's `callback_data`
must carry only an approval id, never the payload. The payload lives in CowBot;
the button is a pointer. The adapter must also answer a `callback_query` with
`answerCallbackQuery` or the client shows a spinner until it times out.

Two constraints worth recording before anyone plans around them:

- **A bot cannot read history** and cannot enumerate chats. It sees only updates
  delivered after it was added. Backfilling an existing Telegram conversation
  into a CowBot channel is not possible.
- **A bot cannot open a private chat** — the user must message it first.
- **Telegram is not reachable from mainland China.** For a project that ships
  Simplified and Traditional Chinese interfaces, that makes Telegram and Feishu
  serve different audiences rather than competing ones.

## 5. Proposed architecture

### 5.1 One adapter trait, four implementations

```
crates/cowbot-integrations/        (new crate)
  ├── lib.rs          Integration trait, registry, supervisor
  ├── model.rs        Binding, ExternalMessage, OutboundTarget, DedupeKey
  ├── inbound.rs      normalisation: mentions, threads, author -> principal
  ├── outbound.rs     channel message -> platform payload
  └── adapters/
      ├── mattermost.rs
      ├── telegram.rs
      ├── slack.rs
      └── feishu.rs
```

```rust
/// One IM connection bound to one CowBot channel.
#[async_trait]
pub trait Integration: Send + Sync {
    fn kind(&self) -> IntegrationKind;
    /// Start the inbound loop until the token is cancelled.
    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()>;
    /// Post a reply/notice back to the conversation the message came from.
    async fn deliver(&self, target: &OutboundTarget, content: &OutboundContent)
        -> CowResult<DeliveredId>;
    /// Ask a human to decide an approval, returning when it is decided.
    async fn ask_approval(&self, target: &OutboundTarget, request: &ApprovalPrompt)
        -> CowResult<ApprovalDecision>;
}
```

`IntegrationContext` carries what the adapter needs and nothing more: the
`Store`, the `EventBus` (to subscribe to `MessagePosted`), the channel binding,
and a `TurnExecutor` — the same shape as the desktop's
`reply_to_mentions` call, injected rather than imported. When §3 lands, the
executor is the server's; today it can be a stub that logs "no executor
configured".

### 5.2 Inbound pipeline

```
platform event
  → verify (signature / token / decrypt)
  → dedupe on (integration_id, external_event_id)      ← else retries double-fire
  → drop if author is this bot                          ← else echo loop
  → resolve author to a CowBot user (map or provision)
  → translate mentions to @DisplayName
  → post_thread_message(channel, Sender::User(id), text, thread_id)
  → if the text mentions an agent, start the turn (today: hand to executor)
  → ack
```

Two rules matter more than the rest:

- **Ack before the turn.** Slack and Feishu give 3 seconds; an LLM turn takes
  far longer. Persist the message, return 200, run the turn in the background —
  which is exactly what the desktop already does.
- **Deduplicate before posting, not after.** Dedupe on the platform's own event
  id, stored with a unique index, so a retry is a no-op rather than a second
  message in the timeline.

### 5.3 Outbound mirror

Subscribe to the workspace `EventBus`. For each `MessagePosted` whose channel is
bound and whose sender is `Sender::Agent`, post it to the conversation the
inbound message came from. `Sender::User` messages are skipped: the IM already
shows them, and echoing would duplicate.

Long answers need chunking to the platform's message limit (Mattermost and Slack
both cap around 4 000 characters per post). Split on paragraph boundaries and
preserve order; a chunked reply is one logical answer, not several.

### 5.4 Approvals over IM

An agent that wants to delete a file in an IM conversation cannot draw a
desktop approval card. The message-card round trip:

1. The turn's `ToolGate` sees `RiskLevel` above the channel's policy and calls
   `Integration::ask_approval`.
2. The adapter posts an interactive message: tool name, arguments (truncated),
   risk, and Approve / Reject.
3. The same callback endpoint (or socket) receives the action, verifies the
   payload signature, and resolves the pending future.
4. Timeout uses the existing expiry sweep so a stale card cannot be clicked
   later.

The decision must be attributed to the deciding IM user, not the bot — the
`tool_calls.approved_by` column already exists for this. This requires the
approval record to hold a pending-resolver handle, so
`cowbot-service/src/approvals.rs` grows an async wait rather than the current
poll-or-card model. That is the most invasive part of the whole feature and
should be its own milestone, after text messages work.

## 6. Data model

One migration, additive:

```sql
CREATE TABLE integrations (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL,           -- mattermost | telegram | slack | feishu
    label         TEXT NOT NULL,
    -- credentials sealed the same way prompt snapshots are; never in exports
    config        TEXT NOT NULL,           -- JSON: endpoint, bot token, signing secret
    created_at    TEXT NOT NULL,
    revoked_at    TEXT
);

CREATE TABLE integration_channels (
    integration_id  TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_id     TEXT NOT NULL,         -- platform channel/conversation id
    channel_id      TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    agent_id        TEXT,                  -- optional: force one agent, skip mentions
    PRIMARY KEY (integration_id, external_id)
);

CREATE TABLE integration_principals (
    integration_id  TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_id     TEXT NOT NULL,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    display_name    TEXT NOT NULL,
    PRIMARY KEY (integration_id, external_id)
);

CREATE TABLE integration_events (
    integration_id   TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    external_event_id TEXT NOT NULL,
    received_at      TEXT NOT NULL,
    PRIMARY KEY (integration_id, external_event_id)
);
```

`.cowpkg` export must omit `integrations` and `integration_events` entirely —
credentials and dedupe state are deployment state, like `task_worktrees`
(ADR-0031). `integration_channels` and `integration_principals` may export as
configuration, but importing a binding that points at a channel id that exists
in the new workspace is only meaningful if the package also re-keys it; safest
is to omit all four and let an operator re-bind.

## 7. Staged plan

| Stage | Deliverable | Blocked by |
| --- | --- | --- |
| S0 | Server-side turn execution, single-provider env config, server tools profile | — |
| S1 | `cowbot-integrations` crate, adapter trait, bindings table, outbound mirror | S0 |
| S2 | Mattermost inbound (WebSocket bot), mention translation, dedupe, principal provisioning | S1 |
| S3 | Telegram inbound (`getUpdates` long polling, private chat → direct channel, group → mention turn). **Built** ([ADR-0058](../ADR/0058-telegram-adapter.md)); inline keyboards move to S5 with the other platforms' approval actions | S2 |
| S4 | Slack via Socket Mode. **Built** ([ADR-0059](../ADR/0059-slack-adapter.md)); the Events API remains a later deployment option | S3 |
| S5 | Approvals over IM. **Built** ([ADR-0060](../ADR/0060-im-approvals.md)); no change to the approval wait, but the decider is authorised | S3 |
| S6 | Feishu. **Built** ([ADR-0061](../ADR/0061-feishu-adapter.md)) over the long connection, so no AES and no signature | S5 |
| S7 | Attachments inbound. **Built** ([ADR-0060](../ADR/0060-im-approvals.md)) as files under the tool root; images are stored and named, not understood | S6 |

S0 deserves its own ADR; it is a topology change to the server, not an
integration detail. S1–S2 deserve one more, covering the adapter trait and the
identity mapping. Everything after that extends the same contract.

Telegram sits at S3 rather than later for two reasons: it is the smallest
adapter in the set, and it is the first *SaaS* adapter — proving the trait
against a remote platform with retries and rate limits before Slack's Block Kit
and Feishu's crypto arrive. Doing Slack first would mean learning the same
lessons with more surface area. Approvals move to S5 and now cover three
platforms at once, because Telegram's inline keyboard and Mattermost's
interactive actions have the same callback shape (one opaque id, one decision)
and are cheaper to build together than serially.

A desktop-hosted bridge could deliver S2 without S0 for a single-user setup. It
is worth building only if a user asks for it, and it should be named clearly as
a local-only mode so nobody deploys it for a team.

## 8. Security considerations

- **Credentials at rest.** Bot tokens and signing secrets belong in the same
  sealed, redacted treatment as prompt snapshots (ADR-0037), never in
  `.cowpkg`, never returned by an API read.
- **Verification is not optional.** Slack without signature checking and Feishu
  without the verification token are open relays: anyone who learns the URL can
  drive an agent that executes code. Verify against the raw body, constant-time
  compare, and reject stale timestamps. Telegram's long-polling mode removes the
  inbound surface entirely, which is why it is the cheapest adapter to secure;
  if an operator chooses a webhook instead, the `secret_token` header must be
  compared before the body is parsed, and `update_id` must be deduplicated —
  Telegram replays unconfirmed updates.
- **The bot acts as a workspace member.** Give it the weakest role that can post
  (`can_write`), not Admin. A compromised integration should be unable to mint
  tokens or read the audit log.
- **Echo and injection.** A message relayed from IM is untrusted user input into
  a tool-using agent. The existing approval gate is what stops a "please run
  this" message from executing code, which is the strongest argument for S5
  arriving before any adapter gets broad tool access.
- **Sandbox blast radius.** With server-side turns, the subprocess sandbox now
  runs beside other people's data. Ship S0 with the narrowest tool registry that
  still answers questions.

## 9. Open questions

1. Does an IM message need to appear in the CowBot timeline at all, or is the
   IM a second view of a channel that only agents and the bridge post to? The
   first is simpler; the second keeps the timeline clean.
2. Should one CowBot channel bind to one IM channel, or should an IM thread bind
   to a CowBot `thread_id`? Thread-level binding is closer to how the core model
   already stores `thread_id`, but it multiplies binding rows.
3. Where do IM users land on the role ladder? `can_write` for everyone who can
   see the channel, or an explicit allow-list of IM handles?
4. What does the agent see as its "platform context"? The channels-turn path
   already injects a `platform_context` string; an IM turn should say which
   platform it is in so the agent can format for it.
5. Does a Telegram private chat become a `Direct` channel (one per Telegram user,
   provisioned on first message) or a `Conversation` with a chosen participant?
   The first reuses `ensure_agent_channels`; the second is what an operator who
   wants a multi-agent bot would need.
6. Telegram allows one bot token to belong to many groups. One `integrations` row
   per binding, or one row per token with many `integration_channels`? The latter
   avoids duplicating a credential that is logically one bot.

## 10. Recommendation

Build **S0 first**. It is the largest single piece and the only one that makes
every later piece worth anything; without it an IM bot can only echo messages
into a database nobody answers from. Then take **Mattermost** as the reference
adapter — self-hosted, no public ingress, concepts that match CowBot closely.
**Telegram** should follow immediately: it is the smallest adapter in the set,
it needs no public ingress or crypto, and its private-chat/group split is
already `Direct` and mention routing. That ordering validates the trait against
a self-hosted system and a SaaS one before Slack's Block Kit and Feishu's
encryption demand real protocol work.

If only one adapter will ever be built, and the deployment is not in mainland
China, **Telegram is the best single choice**: it needs no server reachable from
the internet, no reverse proxy, and no signing infrastructure, and a user can
have a working agent bot in an evening.

The adapter interface should be written against the `EventBus` and
`ChannelMessage` that already exist rather than a new abstraction; the core
model is already the right shape, and the four gaps in §2 are all additive.
