# 0054: Checking the server's API version and retrying transient reads

- Status: Accepted
- Date: 2026-11-19
- Discussion: follow-up to Phase 2 milestone 2.3, tracked in the [roadmap](../ROADMAP.md) known-gaps list. Builds on [ADR-0045](0045-collaboration-api-and-remote-client.md) and [ADR-0053](0053-desktop-server-mode.md).
- Supersedes: None

## Context

ADR-0043 gave the server a `GET /health` that reports `api_version`, and its
comment said a client would check it before speaking. Nothing did. `sign_in` and
`resume` opened with an authenticated request, so a desktop pointed at a server
of another API version got whatever the shape mismatch produced — a parse error
naming a field, in a code path that has nothing to do with versions. That is the
one failure a user cannot diagnose alone: both sides are "CowBot", and neither
message says which one to update.

The same code sent every request exactly once. A desktop holding a WebSocket that
reconnects with backoff (ADR-0053) still failed the ordinary HTTP call it made
next if the server was restarting or a proxy dropped the connection, and the
user saw a dialog for something that would have worked a second later.

The server has no rate limiting (that remains a gap), so `429` is not something
this deployment currently produces; it is handled anyway because a deployment
behind a proxy may impose one.

## Decision

### 1. The version is checked before the first authenticated request

`sign_in` and `resume` call `/health` first. A reachable server that reports
another version is refused with both numbers and the instruction to update the
older side. A server that answers but does not report a version is refused as
"check that this address runs CowBot Server", which is what a wrongly-typed
address usually is.

The client's version lives in `cowbot-client` as `API_VERSION`, beside the
server's constant in `cowbot-server`'s `api` module. They are compared for
equality rather than a range: nothing in the protocol is additive yet, and a
range would claim a compatibility the routes have not been designed for. When a
version becomes backward-compatible, the comparison becomes a range in one
place.

`/health` is unauthenticated, so the check runs before any credential is sent.
That ordering is deliberate: a mismatched server should not receive a password
or a session token at all.

### 2. Reads retry; writes do not

A read is retried up to three times with a 200 ms → 400 ms backoff when the
failure is transient:

- the connection never opened, or a timeout;
- `5xx`;
- `429`.

A `4xx` that names a problem with the request is not retried — repeating it
produces the same answer. A write is always sent once: the API has no
idempotency key, so a write the server applied before the connection broke would
be applied twice, and a duplicated message, task, or approval decision is worse
than an error the user can retry by hand.

The retry lives in one helper (`send_with_retry`) so the rule is stated once.
The `/health` check uses it too, since it is a read.

### 3. The socket keeps its own reconnection

The event stream already retries with a longer backoff (2 s → 60 s) and
announces recovery with `StreamResumed`, because a gap in events needs the window
to reload rather than a quiet retry. HTTP reads have nothing to reload, so a
short retry and then an error is the right shape there.

## Alternatives considered

**Check the version only at first failure.** Cheaper by one round trip, and it
puts the diagnosis in the same place as the failure. Rejected because it sends a
credential to a server that cannot use it and because the failure is exactly the
one a clear message prevents.

**A compatible range instead of equality.** Nothing in the routes is additive
yet; a range would be a promise the protocol has not made. Equality, changed to a
range when one version actually keeps working across a bump.

**Retry writes as well.** Simple to write and hard to make safe without
idempotency keys on every write route, which is a protocol change rather than a
client change. A user-visible error is the honest alternative.

**Retry with a longer backoff.** Three attempts over 600 ms covers a restart or a
dropped proxy connection, which is the case this addresses, and keeps a broken
server from hiding behind a spinner. The socket's 60 s ceiling is for a gap that
has to survive, not for a call the user is waiting on.

## Consequences

- A version mismatch is reported once, with both numbers, before any credential
  leaves the client.
- A read survives a server restart or a dropped connection; a write reports the
  failure and is not silently duplicated.
- `cowbot-client` gains `axum` as a dev-dependency so the tests can run stub
  servers. A real server only ever reports its own version and only fails when a
  client makes it fail, so the mismatch and retry paths cannot be reached
  against it.
- Still open: no server-side rate limiting, and no idempotency keys that would
  make retrying a write safe.
