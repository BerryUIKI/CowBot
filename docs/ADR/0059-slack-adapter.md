# ADR 0059: The Slack adapter

- Status: Accepted
- Date: 2026-11-24
- Related: [ADR-0056](0056-im-integrations.md),
  [ADR-0057](0057-mattermost-adapter.md),
  [ADR-0058](0058-telegram-adapter.md),
  [ADR-0060](0060-im-approvals.md),
  [ADR-0061](0061-feishu-adapter.md)
- Discussion: `S4` of the [P3 plan](../plans/P3-im-integrations.md).

## Context

Slack is the third platform and the second reached over a WebSocket. The plan
([`docs/plans/P3-im-integrations.md`](../plans/P3-im-integrations.md)) named
Socket Mode as the transport and the Events API as "a deployment choice, not a
second implementation". It also predicted that Slack would be the most expensive
adapter in the set, because of Block Kit and the signature scheme.

Half of that was right. Block Kit is real work and is written here. The signature
scheme is not: **Socket Mode has no inbound request to authenticate.** The app
opens a socket using an app-level token, and Slack answers `apps.connections.open`
with a single-use `wss://` address that expires in thirty seconds. Everything
after the handshake is already authenticated by the connection itself.

## Decisions

### 1. Socket Mode only, and the signing secret is optional

The signing secret exists for the Events API, which verifies an HMAC over
`v0:{timestamp}:{raw_body}` on an inbound POST. This build has no inbound
surface, so nothing reads the secret. It is accepted and kept for the transport
that will need it, and it is **not required** — asking for a credential nothing
reads is worse than not asking, because it looks like it is in force.

### 2. The acknowledgment replaces the signature

Every envelope — `events_api`, `interactive`, `slash_commands` — must be
acknowledged within **three seconds** by echoing its `envelope_id`. An
acknowledged envelope is never redelivered; an unacknowledged one is, and enough
of them make Slack close the socket with a `disconnect` whose reason is
`warning`.

So the ack is sent **before** the handler runs, on its own, and the handler's
result only reaches a log line. This is the opposite of the Mattermost socket,
where a post is stored before anything else happens, and it is the single
ordering that matters in this crate. Slack's own `warning` reason is reported
plainly rather than as a network fault, because the fix is the ordering and not
the connection.

### 3. `apps.connections.open` is called per dial, never cached

The address is single-use and expires after thirty seconds. Caching it would
work until the first reconnect and then fail with a 401 that reads like a token
problem. It is fetched immediately before every dial.

### 4. mrkdwn is unescaped and mentions are translated before storage

Slack sends `&amp;`, `&lt;`, `&gt;` and encodes a mention as `<@U123>`. Both have
to be undone before the message is stored, or the CowBot timeline shows entities
and opaque ids. Two details:

- the unescape order is `<`/`>` first and `&` **last**, because doing `&` first
  turns `&amp;lt;` into `<`;
- the addressing check reads the **raw** text, because after translation the
  mention is a name and a name is not unique.

`translate_mentions` keeps the id when Slack sent no display name
(`<@U123>` with no `|name>`). An id reads as an id; an invented name would
address the wrong reader.

### 5. A subtyped message is not a message

Slack delivers `channel_join`, `bot_message`, `message_changed` and a dozen more
as `type: "message"` with a `subtype`. A `message_changed` in particular carries
a **new** `ts`, so treating it as a message stores every edit as a second turn.
The adapter drops anything with a subtype.

### 6. The dedupe key is `channel:ts`

A `ts` is unique within a channel, and two channels can hand out the same value.
The event id is the pair, so a redelivery is caught and a coincidence across
channels is not.

### 7. The chunk budget stays at 4 000 characters

Slack's `text` accepts 40 000, but a Block Kit message is limited to 50 blocks
and a reader loses their place long before either. The shared budget is kept so
one answer reads the same in every IM.

### 8. A file's host is checked before the bot token is attached

`url_private` arrives inside an event. The adapter refuses to download from a
host that is not `slack.com` or `*.slack.com`, and does not read a userinfo
section as the host — `https://slack.com@evil.example/` is refused. Without that
check a crafted event could have the adapter send its bot token wherever the
sender chose.

## Alternatives considered

- **The Events API as well.** Rejected for this stage: it needs an inbound
  surface, signature verification, and a replay window, and it does not make any
  deployment easier that Socket Mode does not already serve. The signing secret
  is accepted so the transport can be added without a configuration change.
- **A Block Kit model in Rust.** Rejected: a card Slack must render exactly is
  JSON, and a Rust type would be a second place to get it wrong. `blocks.rs`
  builds the two documents inline and a test asserts their shape.
- **A `Sender::External` variant for the author.** Rejected in ADR-0056 and still
  wrong: it touches three `Sender` matches and leaves the approval audit with no
  joinable identity.
- **Translating every mention to a name.** Rejected: only the bot's mention needs
  translating for addressing, and another person's mention has no CowBot name to
  translate to.
- **Requiring `signing_secret`.** Rejected: it is a credential nothing in this
  build reads, and a stored secret that is not in force is a false assurance.
- **A `prost` dependency for Block Kit.** Not applicable — Block Kit is JSON. See
  ADR-0061, where the same question is real.

## Consequences

- **The ack ordering is the correctness boundary.** A change that moves the ack
  after the handler looks harmless and produces redeliveries, then a `warning`
  disconnect that reads as a network problem.
- **`allowed_updates` has an analogue here that Telegram's does not.** A missing
  subscription (an app not subscribed to `message.channels`) produces no error
  and no events. This is the same silent failure as Mattermost's un-joined
  channel, and the binding health check is what reports it: the adapter asks
  `conversations.info` and `conversations.members` and reports `Unreachable` when
  the bot is not listed.
- **A decided approval card is revised with `chat.update`**, which replaces the
  whole block array. The decided revision therefore carries no actions block at
  all, rather than a disabled one.
- **A rate limit is read from the `Retry-After` header**, because Slack sends a
  429 with no usable body.
- **Nothing carries the bot's own display name.** `auth.test` reports the bot's
  user name, and the adapter uses it only for its own log line and its own
  message filter. An IM user's display name is not reported by the message event
  in general, so a principal is provisioned under its id and a readable name is
  derived by the shared layer.
- **A file download can fail for a reason the sender controls** — a private file
  the bot cannot read, a file deleted between the event and the fetch. Both are
  reported in the message text as "not kept" rather than dropping the message.
