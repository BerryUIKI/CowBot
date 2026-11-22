# ADR 0058: The Telegram adapter

- Status: Accepted
- Date: 2026-11-23
- Discussion: `S3` of the [P3 plan](../plans/P3-im-integrations.md), tracked as
  [#272](https://github.com/BerryUIKI/CowBot/issues/272) under the
  [IM integration umbrella #268](https://github.com/BerryUIKI/CowBot/issues/268).
- Depends on: [ADR-0056](0056-im-integrations.md). Follows
  [ADR-0057](0057-mattermost-adapter.md), which proved the contract against a
  self-hosted system.

## Context

Mattermost proved the adapter contract against a self-hosted platform the
operator already runs. It did not prove it against a SaaS one, where every
assumption about transport, rate limits, and identity is different.

Telegram is the cheapest way to find out. It needs no public ingress, no HMAC, no
decryption, and no handshake; its default privacy mode already is the
mention-driven turn CowBot implements. Doing it before Slack means learning the
SaaS lessons against a request/response loop instead of Block Kit and Socket
Mode.

## Decision

### 1. Long polling, and no inbound surface at all

`getUpdates` is an HTTPS POST the server makes outward with an `offset` and a
30-second `timeout`. There is no webhook, no reverse proxy, no DNS name, no TLS
certificate, and no signature to verify. The bot token in the URL path is the
entire authentication story.

The trade is one request every 30 seconds per bot on an idle deployment, which
is cheaper than operating an endpoint. A deployment that wants a webhook can add
one later; nothing in the adapter assumes either.

### 2. The offset is the acknowledgement, so the claim happens before the post

Telegram confirms nothing directly. Sending `offset = update_id + 1` on the next
poll tells it the batch was handled, and it will not deliver those updates again.

That makes the failure mode clear: a crash between storing a message and the next
poll replays the batch. The shared pipeline already claims the platform event id
before posting, so a replay is a no-op — and the event id here is the **update
id**, not the message id. An edit reuses a message id, a batch replay reuses
update ids, and only the latter is the thing being deduplicated.

The offset advances before the message is handled, deliberately: whether it was
stored, skipped or refused, this delivery has been dealt with, and the dedupe
table is what protects the replay.

### 3. Three failures are terminal, and none of them retries

The poll loop distinguishes three cases that a stringly-typed error cannot:

* **`409 Conflict`** — a webhook is registered, or another process polls the same
  token. This never fixes itself, and a retry loop around it is a silent spin
  that fills the log. The error names `deleteWebhook` and the other process.
* **`401 Unauthorized`** — the token was revoked or replaced. Same reasoning.
* **`429 Too Many Requests`** — Telegram says `retry_after`, so the loop waits
  exactly that long and then backs off from there. Nothing else is guessed.

Every other failure backs off from one second to a minute, capped so a platform
that comes back after an outage is noticed inside a minute.

### 4. Entity offsets are UTF-16 code units, and that is not a detail

Telegram measures an entity's `offset` and `length` in UTF-16 code units. An
emoji is two units and one character, so an answer containing one shifts every
later offset. Treating the offset as a character index slices the mention in the
wrong place and reads `@co` as the handle — which silently stops the bot from
answering the message that mentioned it.

`update::slice_utf16` converts, and an offset that lands inside a surrogate pair
returns `None` rather than panicking: Telegram cannot produce one, but a
malformed payload must not take the poll loop down.

The same unit question decides the outbound limit. `sendMessage` caps at 4 096
**UTF-16 units**, so `cowbot-integrations` grew `chunk_units` beside `chunk` and
a `ChunkLimit` that names which of the two applies; an answer that is 4 000
characters of emoji is 8 000 units and would be refused outright.
`Integration::chunk_limit` is how an adapter declares it — the trait's default is
the shared 4 000 characters, Telegram overrides it with 4 096 units, and the
mirror asks the adapter rather than assuming. The *unit* is the only thing an
adapter supplies: the boundaries and the ordering stay shared, so one answer
reads the same on every platform.

### 5. Addressing has three forms, and only one of them is text

A message addresses the bot when:

* a `mention` entity names its handle;
* a `text_mention` entity's `user.id` is the bot's — that is how a client inserts
  a mention of an account with no username;
* it replies to one of the bot's messages.

A `/command` counts too, but only when it carries no `@handle` or carries this
bot's: `/help@other_bot` is that bot's command, and answering it would be wrong.

A private chat needs none of this. Every message in it is addressed to the bot,
which is exactly CowBot's direct conversation (ADR-0024), so `addresses_bot` is
true by definition there and the entity check only applies to groups.

**Privacy mode is the reason this matters.** Telegram's default delivers a group
bot only what mentions it, replies to it, or commands it — which is ADR-0022's
mention-driven turn, arrived at without a switch. A deployment that disables
privacy mode makes the bot see everything; the adapter then stores every message
and starts a turn only for the ones that address it. That is the same
store-but-do-not-answer split Mattermost has, so nothing new is needed.

### 6. The answer is rendered to HTML, and falls back to plain text

An agent's reply is Markdown. `sendMessage` accepts MarkdownV2 or HTML; HTML is
the one that can be produced correctly without a full parser, because the only
thing that must be right is escaping.

`render::to_html` escapes first and adds tags after — the other order would let
a `<b>` inside a code block become markup, and an unclosed tag breaks the whole
message. It understands fenced code, inline code, bold and italic, and leaves
anything else as literal text. A stray marker stays a character rather than
becoming an unclosed tag.

If Telegram still refuses the result with a parse error, the adapter posts the
chunk as plain text. An unformatted answer beats one lost to a formatting quirk.

### 7. A binding is checked where the platform can actually answer

`getChat` is the check that works for every chat kind, and its refusal is
specific. Two of Telegram's answers are facts about the binding:

* `400 chat not found` — the bot was never added to the group, or the person in a
  private chat has never messaged it. A bot cannot open a private chat, which is
  the fix this reason names.
* `403` — the bot is no longer allowed in the chat.

A `401` or a `5xx` is *not* an answer about the binding, and reporting it as
unreachable would send an operator to fix something that is fine. It reads as
`Unknown`, exactly as it does for Mattermost.

A **channel** is the one chat kind where being present is not enough: a bot that
is a plain member can read a channel and its posts are refused. For a channel the
adapter also asks `getChatMember` and requires `administrator`, and the reason
says so.

### 8. Shared rules moved into the shared crate

A second adapter is the first evidence that a rule is platform-independent. Two
Mattermost pieces moved to `cowbot-integrations`:

* the binding-health notice loop — same sentence, same destination, on every
  platform — as `notice::report_binding_health`;
* the `(2/3)` chunk label, as `outbound::label_chunk`, because two adapters
  formatting it differently would make one answer read differently in two IMs.

`MattermostIntegration` lost both and kept its platform-specific
`binding_health` function.

### 9. A person's token is refused

`getMe` reports `is_bot`. A human account's token would make the adapter answer
as that person, so a non-bot identity refuses to start. BotFather is named in
the error because that is the fix.

## Alternatives considered

- **A webhook.** Needs a public URL, TLS, a reverse proxy, and a `secret_token`
  check — for a deployment whose whole selling point in the Mattermost adapter
  was needing none of that. Available later as a choice, not required now.
- **The message id as the dedupe key.** Simpler-looking, and wrong: an edited
  message reuses its id, so a legitimate second delivery would be dropped.
- **MarkdownV2.** Requires escaping eighteen characters that Markdown itself
  uses, and every one of them is a way to produce a `400`. HTML needs three.
- **Counting characters for the split.** Correct for Mattermost and Slack, and
  refuses messages on Telegram as soon as an answer contains an emoji.
- **Requiring the bot to have privacy mode off**, so it sees every group
  message. That would make every message the bot can read a turn when nobody
  addressed it — the busy-room problem Mattermost has to filter, adopted as a
  requirement.

## Consequences

**Benefits.** A second platform, and a SaaS one: a system-tested proof that the
adapter contract holds where there is no self-hosted server and no socket. The
deployment needs no inbound surface of any kind to run a bot. Telegram rows now
report `supported: true`, so Settings → Integrations offers it. The split-by-unit
work is shared, so Slack and Feishu inherit it.

**Costs and risks.**

- **One request per bot every 30 seconds while idle.** Small, but it is traffic
  that a webhook deployment would not have.
- **A bot cannot read history.** It sees updates delivered while it was polling;
  a restart before the offset was confirmed replays one batch (deduplicated), but
  anything Telegram dropped while no process polled is lost. Telegram is not a
  message archive and cannot be backfilled into a CowBot channel.
- **A bot cannot open a private chat.** A person must message it once. The bind
  check says so, but the binding cannot make it happen.
- **A bind checks the token's shape, not itself.** The configuration refuses a
  token that does not look like BotFather's, but only `getMe` proves it works,
  and that happens at startup.
- **Attachments are dropped.** A photo or document carries no `text`, so the
  message is skipped as empty. S7 is where that changes.
- **A `429` on `sendMessage` is not retried.** The poll backs off correctly, but a
  delivery refused for rate limiting fails the chunk and the supervisor logs it.
  A mirrored answer longer than Telegram's per-chat limit could lose a chunk.
- **The plain-text fallback loses formatting silently.** It is the safer failure,
  but an operator does not see that it happened; only a debug log line says so.

**Follow-ups.** Attachments (S7); approvals over IM (S5), where Telegram's inline
keyboard and its 64-byte `callback_data` cap are the reference implementation;
Slack (S4); a periodic binding re-check, still the obvious answer to a bot removed
from a chat after startup.
