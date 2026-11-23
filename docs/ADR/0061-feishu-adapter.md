# ADR 0061: The Feishu adapter

- Status: Accepted
- Date: 2026-11-24
- Related: [ADR-0056](0056-im-integrations.md),
  [ADR-0057](0057-mattermost-adapter.md),
  [ADR-0058](0058-telegram-adapter.md),
  [ADR-0059](0059-slack-adapter.md),
  [ADR-0060](0060-im-approvals.md)
- Discussion: `S6` of the [P3 plan](../plans/P3-im-integrations.md).

## Context

The plan ordered Feishu last because it was "the one that forces the crypto work":
an Encrypt Key, AES-256-CBC with `SHA-256(encrypt_key)` as the key, PKCS#7
padding, an HMAC-SHA256 signature over `timestamp + nonce + encrypt_key + body`,
and a `url_verification` handshake answered inside one second. All of that is true
of Feishu's **webhook** transport. It is also the reason the plan expected S6 to
be the most expensive adapter in the set.

Feishu also has a **long connection** transport, and it removes every one of those
requirements. CowBot POSTs its app credentials to `/callback/ws/endpoint`, receives
a `wss://` address, and dials out. The frames after the handshake are plaintext
that Feishu has already authenticated: no public URL, no TLS certificate of our
own, no AES, no HMAC, no replay window.

What is genuinely hard about Feishu is not the crypto. It is the protocol.

## Decisions

### 1. The long connection, not the webhook

Same reasoning ADR-0058 applied to Telegram: the transport that needs no inbound
surface is the one a self-hosted deployment can actually run. An operator who
needs a webhook — a cloud function, or a tenant that forbids long connections — is
served by a later change, not by a half-built one.

This is why `FeishuConfig` does **not** accept `encrypt_key` or
`verification_token`. In this transport they are dead configuration, and the type
refuses to hold a secret nothing reads: a stored credential looks like it is in
force.

### 2. The frames are protobuf, and the codec is hand-written

The transport is not JSON. Each frame is a `pbbp2.Frame`: a varint length prefix,
then fields — `seq_id`, `log_id`, `service`, `method`, `headers` (a repeated
submessage of key/value), `payload_encoding`, `payload_type`, `payload`,
`log_id_new`.

The adapter needs nine field numbers of one message type, all of them varints or
length-delimited strings. `frame.rs` decodes and encodes them by hand, and that is
a deliberate choice rather than a shortcut: `prost` would bring a build-time code
generator and a `protobuf-src` toolchain requirement for one struct, and the
format is fully specified by the SDK's `pbbp2.pb.go`. The decoder is tolerant —
an unknown field is skipped by its wire type rather than rejected, because Feishu
adds fields and a frame that fails to parse is a dropped message with nothing in
the log to explain it.

Two details that cost time:

- `decode` strips a length prefix **only** when one is present and matches the
  remaining length; a body whose first byte happens to look like a matching
  length must not lose its first field. `decode_body` exists for a caller that has
  already split the stream.
- a frame's `method` field is a varint: `0` is control (ping, pong) and `1` is
  data. Reading only the `type` header would treat a pong as an event.

### 3. A handler has three seconds, so nothing waits

The answer to a data frame goes back on the same socket, as a frame whose payload
is JSON `{code, headers, data}` with `data` being base64 of the JSON the client
reads. A card callback's toast or card replacement is that base64 string.

Nothing in `handle_frame` waits for the pipeline: `sink.accept` stores the message
and returns, and the turn runs behind it. The answer is written at once, and a
handler that failed still answers — an unanswered frame leaves the person looking
at an interaction error and tells Feishu nothing.

### 4. The dedupe key is the message id, not the event id

This is the one place Feishu contradicts every other platform in this repository.
Its own documentation says repeated pushes can occur and that idempotency must use
`message_id`. `header.event_id` is unique per *delivery*, so keying on it lets one
message become two turns and two charges. The adapter uses
`message.message_id`.

### 5. `content` is JSON inside JSON, and mentions are placeholders

`message.content` is a **string** containing JSON, so the text does not exist
until it has been parsed twice. A `post` message nests its runs a level deeper
again, keyed by locale.

In `text` content a mention is the bare token `@_user_1`; the name is only in
`message.mentions[]`, joined by `key`. Stored unchanged, every message reads as
machine tokens and no agent is ever addressed. `resolve_mentions` maps each key to
its name, and handles `@_all` first — a key-to-name loop that treated the
broadcast mention like any other produces `@` with no name, which
`parse_mentions` reads as an unknown agent.

Addressing is decided by `mentions[].id.open_id` against the bot's own `open_id`,
not by the resolved text: a name is not unique.

### 6. The bot's own `open_id` is discovered, not configured

It is not in the config and not in a message. `GET /open-apis/bot/v3/info` is the
only source, and without it the adapter cannot tell "the bot was mentioned" from
"a bot was mentioned" — the entire addressing rule. Startup fails if the answer
has no `open_id`.

### 7. Only `sender_type == "user"` is a person

Feishu reports `bot` in a message event and `app` in the send API for the same
thing. Treating `app` as a person is an echo loop; treating `bot` as one is the
same loop with a different label.

### 8. `p2p` is a direct conversation and `group` needs a mention

`chat_type` is the routing decision, and reading it as "group by default" makes
the bot silent in direct messages — CowBot's direct-conversation mode is exactly
`p2p` (ADR-0024). The same split as Telegram's private chat and Mattermost's
channel, arrived at from a different field.

### 9. A reply is a Markdown post, not text

Sending an answer as `msg_type: text` shows the reader literal `**` and `#`:
Feishu's `text` does not render Markdown. The adapter sends a `post` whose single
run is `{"tag":"md","text":…}`, which does.

### 10. Business failures are HTTP 200

Every OpenAPI call answers 200 and puts its outcome in `code`. An
HTTP-status-only check reads a failed send as a success, so `api.rs` reads `code`
on every call. A rate limit is `99991400` or an `x-ogw-ratelimit-reset` header,
and the documented recovery is to sleep exactly that long.

## Alternatives considered

- **The webhook transport, and the crypto the plan expected.** Rejected for this
  stage: it needs a public URL and TLS, and it costs AES-256-CBC, an HMAC over the
  raw body, and a one-second handshake deadline — all to arrive at the same
  events the long connection already delivers over an outbound socket.
- **`prost` and a build-time `.proto`.** Rejected in §2.
- **Storing `encrypt_key` and `verification_token` "in case".** Rejected in §1: a
  secret nothing reads is a false assurance.
- **Keying principals on `sender_id.user_id`.** Rejected: it is only returned with
  `contact:user.employee_id:readonly`, so most tenants get an empty id and every
  principal collides into one bridge user. `open_id` is the only id always
  present.
- **Updating a decided card with the callback's `c-` token.** Rejected as the
  primary path: that token is usable twice, for thirty minutes, and only *after*
  the callback has been answered. The shared mirror path — revise on
  `ApprovalDecided` — works for every platform and does not depend on a token
  whose lifetime is a trap.

## Consequences

- **One adapter process per app.** Feishu's long connection delivers each event
  to one of the app's connections at random, not to all of them. Two CowBot
  processes on one app token each receive a subset, so half the messages are
  answered by a process with the wrong binding table, or twice. This is a
  deployment constraint and is recorded in `docs/SERVER.md`.
- **Self-built apps only.** Feishu documents long connections for 企业自建
  applications; a store app cannot use this transport.
- **The card is revised through the shared mirror, not by the callback handler.**
  The callback answers with a toast; the card update happens when
  `ApprovalDecided` reaches the mirror. That keeps one code path for all four
  platforms, at the cost of the card changing a moment after the toast.
- **A file's name is not reported by a message.** Feishu gives a `file_key` and
  nothing else, so the stored attachment takes a generated name and its type is
  taken from the message kind rather than a MIME type.
- **No `encrypt_key` support means no webhook support**, and an operator who
  needs one has to wait for a change that adds it. That is the trade the plan's
  own ordering was built to avoid, and it turned out not to be needed.
