# ADR 0057: The Mattermost adapter

- Status: Accepted
- Date: 2026-11-22
- Discussion: `S2` of the [P3 plan](../plans/P3-im-integrations.md), tracked as
  [#271](https://github.com/BerryUIKI/CowBot/issues/271) under the
  [IM integration umbrella #268](https://github.com/BerryUIKI/CowBot/issues/268).
- Depends on: [ADR-0056](0056-im-integrations.md). This is the first
  implementation of that contract.

## Context

`cowbot-integrations` defined a contract and a pipeline, and nothing had ever
used them. An adapter design that has only been tested against a fake is a
design, not a working feature — and the acceptance criteria for the whole
integration workstream ("a message in a channel produces that agent's answer with
no desktop running") cannot be met by a fake.

Mattermost is the right first platform to prove it on. It is self-hosted like
`cowbot-server`, so it targets the operator CowBot already has. Its concepts map
almost one-to-one — channels, threads, posts — so translation is cheap and a
failure is unlikely to be blamed on the model. And the bot dials *out*: no
reverse proxy, no public DNS, no TLS termination, no signature verification, no
replay window. The deployment's exposure is the bot's own session token.

## Decision

### 1. The socket authenticates in-band, and that shapes the module

Mattermost's WebSocket does not take a token in the handshake. The client
connects, then sends `{"seq":1,"action":"authentication_challenge","data":{"token":…}}`
as its first frame, and the server answers with a `status` event.

So a connect is not "done" when the handshake completes. `socket.rs` treats the
`status` frame as the readiness signal, and a `status: "FAIL"` returns an error
that names the token rather than retrying: a revoked token will never start
working, and a reconnect loop around it looks exactly like a broken server.

### 2. Identity and limits are discovered, not configured

The configuration an operator stores is two values: `base_url` and `bot_token`.
The bot's own user id, its username, and the server's `MaxPostSize` are read from
the server at startup.

The alternative — asking an operator to type them in — makes three values they
can get wrong, and the post limit in particular is a server setting that will
change under them. Reading it means a deployment that raised the limit gets the
larger posts without a redeploy.

`base_url` is refused unless it starts with `http://` or `https://`, and a `wss://`
origin is refused specifically: the socket URL is derived from the base, so an
operator who pasted the socket address gets a doubly derived one and a confusing
connection failure.

### 3. The adapter supplies the three things only Mattermost knows

Everything else is ADR-0056's. What lives in `cowbot-mattermost`:

- **A `posted` event's post is a JSON string inside the envelope**, not an object.
  `posted()` parses it, and an unparsable one is dropped rather than panicking.
- **A mention is `@username`, and a bot's username ends in ` [bot]`.** Only the
  bot's own handle is translated, to the name with that suffix removed. Rewriting
  a teammate's `@alice` would address the wrong reader.
- **A thread is a `root_id`.**

The mention matcher is word-bounded on both sides, so `user@example.com` is not a
mention and `@botanist` does not address a bot called `bot`. Both are tested,
because both are what a naive `contains` gets wrong.

### 4. Addressing means a mention of the bot

Mattermost delivers every post in a channel the bot belongs to. Answering all of
them is the busy-room problem: a bot in a fifty-person channel would run a turn
for every message.

So `addresses_bot` is true only for a mention. A message that does not address
the bot is still stored — an operator reading the CowBot channel sees the
conversation — but starts no turn. The pipeline already distinguishes those two
(`Ingested::Posted::starts_turn`), so this is a value, not a branch.

### 5. Echo suppression happens twice, on purpose

The pipeline drops any message from a bot. The adapter drops its own post before
it reaches the pipeline. The second is not redundant: without it the shared
dedupe table fills with the bot's own traffic, and a bot in a busy channel would
write a row per answer it gave.

Before the socket reports the bot's id, the fallback is the ` [bot]` username
suffix. That window is a fraction of a second, and the fallback is conservative:
an unrecognised author is treated as a person, which at worst costs one turn
rather than losing a message.

### 6. A binding is checked, at bind time and at startup

A binding row says a conversation feeds a channel. It does not say the platform
will deliver anything, and Mattermost delivers posts only for channels the bot
has joined. The result is the worst kind of fault: nothing is broken, nothing is
logged, and the channel is simply silent.

So `Integration::check_binding` exists, with a default that reports
`BindingHealth::Unknown` rather than `Reachable`. An adapter that cannot check has
not confirmed anything, and a green light it did not earn is worse than an honest
"I cannot tell" — which is why `Unknown` and `Reachable` are different variants
and why `Unknown` is not treated as success anywhere.

It is asked twice, because the two moments serve different needs:

* **On bind, in the response.** The operator is on the screen and can go add the
  bot to the channel. Deferring this to the next restart means they bind, see a
  201, and hear nothing.
* **At startup, as a notice in the CowBot channel.** A bot removed from a channel
  after binding, or a deployment upgraded while it was away, has no bind to hang
  the answer on. The notice goes where the person who would fix it is already
  looking rather than into a log file.

A list deliberately omits the verdict: checking every binding would turn a cheap
read into N round trips, and the startup notice already covers it.

### 7. Startup, and only when a provider exists

`integration_startup::start_all` runs at server start, not from a route: a bot has
to be running before anyone messages it, and a route that starts one would need a
supervisor of its own to notice a restart.

It is skipped entirely when the deployment configured no model provider. A bot
that stores a question and never answers is worse than one that is not running,
because the IM user sees their message accepted and waits.

A workspace whose adapter fails to start does not stop the others, and a
configuration that does not parse is logged and skipped rather than taking the
server down. The failure names the field.

## Alternatives considered

- **Incoming webhooks or slash commands.** Write-only or command-only. The agent
  could not answer a conversation, which is the entire feature.
- **An OAuth app with a public callback URL.** Needs a reverse proxy, public DNS,
  and TLS termination for a deployment that already runs both ends. The
  WebSocket bot avoids all of it.
- **Hard-coding the 16 384-byte post limit.** Simpler, and wrong for any
  deployment that changed it. The server is asked.
- **Translating every `@username` into a CowBot name.** Requires a lookup per
  handle per message, and rewrites a person's mention into an agent's. Only the
  bot is translated.

## Consequences

**Benefits.** The adapter contract has now run against a real protocol. A message
in a bound Mattermost channel produces the agent's answer with no desktop
running, which is the acceptance criterion the whole workstream was for. The
`supported` flag on an integration row becomes `true` for Mattermost and stays
`false` for the other three, so an operator can see what this build can run.

**Costs and risks.**

- **The membership check costs one REST call per binding at startup, and one per
  bind.** Cheap, but not free on an integration with many bindings.
- **A binding removed from the channel after startup is not re-checked.** The
  check runs once when the adapter starts; a bot kicked from a channel later
  fails silently until the next restart. A periodic re-check is the obvious next
  step and is not built.
- **A user lookup per unseen author.** A mention's display name costs one REST
  call, cached afterwards. A channel with a large rotating audience pays one call
  per new person.
- **A reconnect re-reads the socket but not the past.** Posts sent while the
  socket was down are not recovered; Mattermost has no resume-from-id over this
  transport. A gap is a lost message, not a duplicated one.
- **`is_bot` is always false on an inbound author.** Mattermost does not mark a
  post as a bot's, and the bot's own posts are already dropped; a *second* bot in
  the channel is treated as a person, so two bots would answer each other. The
  shared pipeline's bot check cannot help because it is given `false`. This is a
  real gap, mitigated only by it being unusual to run two bots in one channel.
- **No start or stop control.** Integrations start with the server and are
  retired through the API; a change takes effect at the next restart.

**Follow-ups.** A periodic re-check of the bindings, so a bot removed from a channel after startup is noticed; Telegram (S3);
approvals over IM (S5), where Mattermost's interactive message actions are the
reference; attachments (S7).
