# ADR 0056: IM integrations

- Status: Accepted
- Date: 2026-11-22
- Discussion: [IM integration umbrella #268](https://github.com/BerryUIKI/CowBot/issues/268),
  `S1` of the [P3 plan](../plans/P3-im-integrations.md), tracked as
  [#270](https://github.com/BerryUIKI/CowBot/issues/270).
- Depends on: [ADR-0055](0055-server-side-turn-execution.md). An adapter is only
  useful once a deployment answers a message without a desktop running.

## Context

A user should be able to ask a CowBot agent a question in the IM their team
already uses, and read the answer there. The model already fits: channels have
threads, messages have an author, `@name` starts a turn, and a turn posts its
answer back through the event bus. What was missing was the translation layer,
and four specific gaps made it more than a thin shim.

**Identity.** [`Sender::User(String)`](../..//crates/cowbot-core/src/channel.rs)
carries a CowBot `UserId`. The server hard-codes the authenticated user and
ignores any body field — correct for a browser, wrong for a bridge that must
attribute a message to the person who wrote it in the IM.

**Mentions are provider-specific.** `parse_mentions` reads `@` plus letters,
digits, `_`, and `-`. Slack sends `<@U024BE7LH>`; Feishu sends
`<at user_id="ou_...">name</at>` and a separate `mentions` array; Telegram sends
plain text with an entity metadata array.

**No external id on a message.** Slack retries a delivery up to three times and
Feishu retries on any non-2xx. Without somewhere to record the platform's event
id, a retry runs a second turn and spends the model's money twice.

**No mapping from an IM conversation to a channel.**

## Decision

### 1. A CowBot user per IM account, provisioned on first sight

The alternative was `Sender::External { platform, external_id, display_name }`.
It is honest, and it changes every match on `Sender` — three in the service, the
desktop renderer, the generated bindings — and still leaves the approval audit
with no name to show.

Instead, `resolve_principal` creates a real `User` row: the id is derived from
the integration and the account (`bridge:{platform}:{integration}:{external}`),
so two messages arriving at once compute the same id and the second insert is a
no-op rather than a duplicate account. The row carries **no password hash**, so
it can never sign in, and a `Member` membership, because a message in a channel
the workspace owns is the workspace's work and refusing to store it would lose
the question.

Deriving the id rather than generating one is the point: the insert is a
race-free "create if absent" without a lock or a transaction.

**Cost.** A busy tenant slowly fills the user table with accounts nobody signs
in as. That is visible in `list_users` and would need a retention action in a
large deployment.

### 2. One trait, four implementations, one pipeline

```rust
pub trait Integration: Send + Sync {
    fn kind(&self) -> IntegrationKind;
    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()>;
    async fn deliver(&self, target: &OutboundTarget, content: &OutboundContent)
        -> CowResult<DeliveredId>;
}
```

An adapter owns only what the platform forces: verifying a delivery and speaking
the platform's payload shape. The rules that are easy to get wrong are written
once in `cowbot-integrations` and tested once against a fake adapter:

- **Dedupe before posting.** `claim_integration_event` is an
  `INSERT … ON CONFLICT DO NOTHING`, so a retry that lands while the first copy
  is still posting is dropped. A failed post releases the claim, so a delivery
  that never landed can still be retried.
- **A bot's message is dropped.** A platform delivers another bot's message
  happily, and two bots mentioning each other is an unbounded loop that costs
  real money.
- **Only `Sender::Agent` is mirrored out.** Echoing a user's message back would
  duplicate it in the IM that already displayed it.
- **Chunking counts characters, not bytes.** Every platform caps a message
  around 4 000 characters; a byte budget would split Chinese far earlier than
  the platform requires and could split mid-codepoint.

`IntegrationContext` carries the `Store` and a `MessageSink`, not an
`LlmProvider`. Running the turn is ADR-0055's job, injected, so an adapter never
learns how a turn executes and is testable without a model.

### 3. Ack before the turn

Slack and Feishu give three seconds to answer a delivery; a model turn takes far
longer. The sink stores the message and starts the turn through a `TurnStart`
handle without awaiting it, so the adapter acknowledges at once. This is the same
ordering the desktop already uses.

A deployment with no executor still records the message — the question is not
lost, the operator sees it unanswered.

### 4. Mention translation is per-binding

A group channel needs the bot's mention turned into `@AgentName`, because
`parse_mentions` understands CowBot names and not platform markup. Which agent
that is comes from `integration_channels.agent_id`, not from the message: only
the bot is bound, so only the bot has a CowBot name. A message that already names
the agent is left alone, so `@CowBot` does not become `@CowBot @CowBot`.

A direct conversation needs none of this — it answers every message — which is
why `agent_id` is nullable and why `addresses_bot` alone starts a turn there.

### 5. Deployment state, never in a package

Four tables, one additive migration (`0021`): `integrations`,
`integration_channels`, `integration_principals`, `integration_events`.

None is exported in a `.cowpkg`. A package would carry a bot token and a dedupe
history that only mean something to the deployment that produced them, and a
re-keyed binding is worse than none. An operator re-binds after an import,
exactly as they re-register a git project (ADR-0031).

Configuration is behind `Store::integration_config`, a method named for what it
returns, and `IntegrationRecord` — the type every read hands out — has no
configuration field at all. `NewIntegration` is write-only and its `Debug` prints
`[sealed]`, so no accidental log or serialization carries a token. This is a
weaker guarantee than the sealed snapshots of ADR-0037: the configuration is
stored as plain JSON, and a database reader sees it. Encrypting it at rest needs
a key-management story this milestone does not have.

### 6. Retired integrations stay readable

`revoke_integration` sets `revoked_at` rather than deleting the row. The bindings
and principals remain, so an operator can read what a retired bot was connected
to, and `list_active_integrations` is what a supervisor starts.

## Alternatives considered

- **A `Sender::External` variant.** Described above; rejected for the blast
  radius and because it still leaves the audit without a name.
- **Adapters as separate binaries talking HTTP.** Duplicates auth and workspace
  resolution, and each would need a token with `can_write` — a wider credential
  surface than an in-process adapter.
- **A desktop-hosted bridge.** Works for one always-on machine with a local
  database and cannot see messages a remote user posts, because those never reach
  that database. Named in the P3 plan as a demo at best.
- **Per-platform pipelines.** Four copies of dedupe, echo suppression, chunking,
  and principal mapping, drifting apart. Rejected; the fake adapter in the tests
  is what proves the shared pipeline is enough.

## Consequences

**Benefits.** Four platform crates each implement two methods. The rules that
cost money when wrong — a duplicated turn, an echo loop, a reply in the wrong
room — are in one place with one set of tests. An adapter is testable without a
model, a network, or an IM tenant.

**Costs and risks.**

- Provisioning a user per IM account grows the user table in a large tenant, and
  there is no retention action yet.
- Configuration is stored unencrypted; a database reader sees a bot token.
- The mirror duplicates a channel's agent replies into the IM. An agent that
  answers a channel for two reasons at once posts twice, because CowBot has no
  message grouping.
- A crash between claiming an event id and posting loses that one delivery. The
  platform will not retry a delivery it saw acknowledged, so this is the better
  failure, but it is a silent lost message.
- A user row with no password hash is a valid `Sender::User`. If a future build
  ever treats a missing hash as "no password set" during sign-in rather than as
  "cannot sign in", an IM account becomes an account. The server's
  `verify_password` already treats a malformed hash as a failed attempt, and a
  test pins it.

**Follow-ups.** The platform adapters (S2 Mattermost, S3 Telegram, S4 Slack, S6
Feishu); approvals over IM (S5); attachments (S7); a Settings surface for
creating and binding an integration; encryption at rest for the configuration.
