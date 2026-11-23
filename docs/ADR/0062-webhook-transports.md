# ADR 0062: Webhook transports, and the inbound surface they need

- Status: Accepted
- Date: 2026-11-24
- Related: [ADR-0056](0056-im-integrations.md),
  [ADR-0057](0057-mattermost-adapter.md),
  [ADR-0058](0058-telegram-adapter.md),
  [ADR-0059](0059-slack-adapter.md),
  [ADR-0060](0060-im-approvals.md),
  [ADR-0061](0061-feishu-adapter.md)
- Discussion: the transport half of `S4` and `S6` in the
  [P3 plan](../plans/P3-im-integrations.md).

## Context

Every adapter this build shipped dialled **out**. Mattermost and Slack open a
WebSocket, Telegram long-polls `getUpdates`, Feishu runs its long connection. That
property was deliberate and it bought three things:

- no public URL, so no reverse proxy and no certificate to manage in front of
  CowBot;
- no signature scheme to implement;
- no replay window to reason about.

Two holes were left open on purpose. ADR-0059 accepted Slack's signing secret and
left it unread, calling the Events API "a deployment choice, not a second
implementation". ADR-0061 chose Feishu's long connection partly because it needs
no cryptography, and named the webhook as the transport that would.

There are deployments the sockets cannot serve: a cloud function or a container
platform that scales to zero, a tenant whose security policy forbids a
long-lived outbound connection, an ingress that only speaks HTTP. For those, the
only way to receive anything is a webhook.

This record adds that transport for Slack, Telegram, and Feishu — without changing
the default for any of them.

## Decisions

### 1. The inbound surface is one route, and it names the integration

```
POST /api/v1/integrations/{integration_id}/webhook
```

On the **public** router, with no bearer token: a platform has no CowBot session.
Authentication is the adapter's own verification of the body.

The URL names the integration rather than the platform. That choice is what makes
per-request verification tractable: the secret checked against the body is
unambiguously that integration's, so verification never has to work out *whose*
body it is holding before it can check it. A per-platform URL would need the body
parsed to find an app id first, which inverts the ordering — an unverified body
would be parsed before it was trusted.

### 2. The adapter verifies; the route does not

`Integration` gained two defaulted members:

```rust
fn accepts_webhook(&self) -> bool { false }

async fn webhook(&self, ctx: &IntegrationContext, request: InboundRequest<'_>)
    -> CowResult<WebhookReply> { Err(..) }
```

`InboundRequest` carries the **raw** bytes, because every scheme in this ADR signs
what arrived, not what a parser would produce from it. A body re-serialised
through `serde_json` changes whitespace and the signature stops matching — the
classic bug in this kind of code.

Both default to an honest refusal, the shape `check_binding` and `post_approval`
already established. An adapter that only dials out is never asked to accept
anything.

### 3. Three cases answer 404, and say nothing more

No such row, a revoked one, and a row whose kind or configuration this build
cannot run all read the same way. The last is deliberate: a configuration an
adapter refuses would fail identically on every retry, so a 5xx would build a
retry loop against a broken row, and a 401 would tell an attacker their guess was
wrong.

### 4. The transport is a configuration field, and the default is unchanged

| Crate | Field | Default | What the webhook needs |
| --- | --- | --- | --- |
| `cowbot-slack` | `transport: socket_mode \| events_api` | `socket_mode` | `signing_secret`, now required for that transport |
| `cowbot-telegram` | `transport: long_poll \| webhook` | `long_poll` | a `secret_token`, validated against Telegram's own rules |
| `cowbot-feishu` | `transport: long_connection \| webhook` | `long_connection` | `verification_token`, plus an optional `encrypt_key` |

A required secret is refused at **parse** time, not at startup: the adapter is the
only thing that knows which field a transport needs, and a configuration that
would accept unsigned traffic must not get as far as running.

In webhook mode `run()` does the work the transport still needs — discover the
identity, check the bindings — then waits on the cancellation signal. The
supervisor, the mirror, and `Running` are untouched, which is the point: transports
differ in how events arrive, not in what happens afterwards.

### 5. Each platform's verification is its own, and the differences matter

**Slack** — HMAC-SHA256 over `v0:{timestamp}:{body}`, compared in constant time,
with a five-minute window. The window is not optional: the signature covers the
timestamp, so without an age check a captured request would verify for ever.

**Telegram** — no signature scheme at all. Its whole authentication story for a
webhook is the secret given to `setWebhook`, echoed in
`X-Telegram-Bot-Api-Secret-Token`. That comparison **is** the verification, and it
is constant time because a short-circuiting `==` leaks a secret one byte at a
time.

**Feishu** — the cryptographic half, and the reason this record exists separately
from the Slack and Telegram changes. A delivery is decrypted first (nothing is
readable otherwise), the `url_verification` handshake is answered as soon as the
envelope opens (Feishu answers it in plaintext even when the request was
encrypted, and it arrives *before* an operator saves anything), and the signature
is checked before anything parses.

### 6. The AES is written by hand, and pinned by a published vector

`cowbot-feishu/src/crypto.rs` implements AES-256 decryption directly rather than
adding an `aes`/`cbc` dependency, for the same reason `frame.rs` decodes protobuf
by hand: the algorithm is small and fully specified, and the alternative is a
dependency for two functions.

The risk of hand-written cryptography is real, so it is mitigated twice:

- the implementation is checked against **FIPS-197 Appendix C.3**, the published
  AES-256 vector, in both directions;
- a round-trip test is not treated as proof. It passes with a symmetric bug in
  both directions, which is exactly the bug a self-taught implementation has.

### 7. Events and card callbacks sign differently, and that is not a bug

Feishu uses two schemes that were never unified:

- an **event** is SHA-256 over `timestamp + nonce + encrypt_key + body`;
- a **card callback** is SHA-1 over `timestamp + nonce + verification_token +
  body`.

A different hash and a different secret. An implementation that checks one for
both refuses every delivery of one kind with no clue why, so each is checked with
its own scheme and the distinction has a test.

## Alternatives considered

- **A per-platform URL (`/webhooks/{kind}`).** Rejected in §1: it forces the body
  to be parsed to find which integration it belongs to, which means parsing an
  unverified payload.
- **Verifying in the server route.** Rejected: the schemes share nothing —
  HMAC-SHA256, a header comparison, and AES-CBC with two signature variants — and
  a route that knew all four would be a fourth place to get them wrong.
- **The `aes`/`cbc` crates.** Rejected by the user in favour of the hand-written
  implementation, with FIPS-197 as the safety net. `ring` is already in the tree
  but exposes AES-GCM only, not CBC, so it could not have been used either way.
- **Requiring a webhook secret always.** Rejected: a polling deployment would
  store a secret nothing reads, which is the false assurance ADR-0061 refused for
  `encrypt_key`.
- **Polling and a Telegram webhook together.** Not a choice: Telegram answers
  `409 Conflict` to whichever asks second, so webhook mode must not poll.

## Consequences

- **A webhook deployment must expose CowBot.** That is the whole trade. It needs a
  public hostname, TLS, and a reverse proxy — `deploy/` already has Caddy
  terminating TLS in front of a loopback-bound server, so the pieces exist.
- **The endpoint is unauthenticated by design**, and its only gate is the
  adapter's verification. A deployment that misconfigures a transport without a
  secret cannot start, which is why every one of the three refuses that at parse
  time.
- **A webhook endpoint is a new load path.** A delivery is stored and answered
  before the turn runs, so a slow model cannot make a platform time out; but an
  attacker who can reach the URL can make CowBot read and parse a body, bounded
  only by whatever the ingress allows. The reverse proxy's request-size limit is
  the control, and `docs/SERVER.md` says so.
- **Feishu's decryption is the only hand-written cryptography in the tree.** It is
  covered by a published test vector, and it is decryption only — the encrypt
  direction exists solely for the round-trip test.
- **The three transports are not tested against the live platforms.** The unit
  and route tests cover what can be verified locally; a deployment is the first
  place a real signature is checked end to end.
