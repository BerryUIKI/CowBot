# ADR 0043: A self-hosted server with password accounts and bearer sessions

- Status: Accepted
- Date: 2026-11-13
- Discussion: Phase 2 milestone 2.1 in [the roadmap](../ROADMAP.md).
- Supersedes: None. The first record for Phase 2.

## Context

Phase 1 is complete and the roadmap's next milestone is a self-hosted team
server. `cowbot-core::identity` has carried `User`, `Role`, `Membership`,
`Session`, `ApiToken`, and `Invite` since the initial schema, and
`cowbot-store` has repositories for every one of them — but nothing has ever
called them. The local runtime bootstraps one user with an empty password hash
and there is no HTTP surface at all.

Two questions had to be answered before any code: what the server reuses from
local mode, and how a user proves who they are.

## Decision

### 1. The server reuses the local schema and repositories

`cowbot-server` is a new workspace crate that opens the same SQLite database
`cowbot-store` already defines and calls the same repositories. A team
deployment is not a second data model: the channels, tasks, assignments,
approvals, conversations, memory entries, and prompt snapshots of a server
workspace are the rows the desktop already reads.

This keeps the migration between local and server mode trivial — a `.cowpkg`
import is the same code path — and means a server milestone adds an API and a
permission layer, not a persistence rewrite. The server is the only writer to
its database; a deployment is one process over one file.

### 2. Passwords use Argon2id, sessions are opaque bearer tokens

- A password is hashed with Argon2id through the `argon2` crate's default
  parameters. A password shorter than ten characters is refused at bootstrap
  and at change.
- Sign-in compares against a fixed dummy hash when the email is unknown, so an
  unknown email and a wrong password cost the same work and return the same
  response. The endpoint cannot be used to enumerate accounts.
- A session token is 256 random bits, hex-encoded. Only its SHA-256 hash is
  stored, so reading the database cannot mint a live session. The token is
  returned once, in the sign-in response.
- Sessions expire after two weeks by default. An expired session is deleted on
  the read that finds it.
- Changing a password drops every session for that user and re-issues the
  caller's, so a token obtained before the change stops working.
- Failed sign-ins are counted per source and refused after ten in fifteen
  minutes. The source is `X-Forwarded-For` when a reverse proxy sets it and a
  single shared bucket otherwise, which is the conservative direction: a
  deployment that does not proxy limits all clients together rather than nobody.

### 3. Identity is an extractor, not a middleware

`AuthenticatedUser` implements `FromRequestParts`, so a handler's signature says
whether it needs a signed-in user. A route that forgets to require one cannot
compile a handler that uses it, and the difference between "public" and
"authenticated" is visible at the route table rather than hidden in layer
ordering.

### 4. The error model is one JSON body with a stable code

Every failure returns `{ "code", "message" }` with an HTTP status that matches.
A remote client branches on `code`. A storage error is logged with its detail
and reported as a generic 500, because a database message can name a table or a
row; `message` is otherwise safe to show a user.

### 5. The first deployment print one credential

A server with no users creates the owner account and a workspace and prints the
generated password to stdout once. There is no mail delivery, so this is the
only way in; the operator changes it from the first session. An existing
database is left alone.

### 6. This milestone exposes accounts only

Routes so far: `GET /health`, `POST /api/v1/auth/sign-in`,
`POST /api/v1/auth/sign-out`, `POST /api/v1/auth/password`, `GET /api/v1/me`.
Channel, task, and approval routes arrive with membership enforcement in 2.2,
so no endpoint yet lets one user act inside a workspace they have not proved a
role in.

## Alternatives considered

**JWT sessions.** Stateless and no session table, but revocation becomes a
compromise: a password change or sign-out cannot reliably invalidate an
already-issued token without a denylist, which is the session table again with
less clarity. Opaque tokens with a hashed row are simpler to reason about and to
revoke.

**Reuse the local `password_hash` column with a passphrase-free model.** The
local runtime deliberately stores an empty hash; treating that as a valid
credential would let anyone sign in as the owner of a migrated workspace.

**A separate server schema.** Duplicating channels, tasks, and approvals would
make `.cowpkg` migration and every future feature a two-sided change, for no
capability the shared schema lacks.

**Put authentication in middleware.** It works, but then every handler that
reads the identity has to trust that a layer ran. The extractor makes the
requirement part of the type.

## Consequences

- The server crate is small and testable without a network: handlers are tested
  through `tower::ServiceExt::oneshot`, and the sign-in rate limiter is unit
  tested directly.
- Argon2id is deliberately slow (~100 ms a hash); sign-in is the only hot path
  and it is user-driven.
- `AuthService` keeps its rate-limit counters in memory, so a multi-process
  deployment would not share them. One server per database is the deployment
  model this milestone commits to; a shared store is a later change if the
  process model ever changes.
- `cowbot-store` gained `list_memberships`, `update_password`, and
  `delete_user_sessions`. The first two had no caller before because nothing
  signed in; the third exists so a password change can revoke.
- The desktop's remote mode (2.3) is the first client of this API. Until it
  exists, the API is exercised by its own tests and by `curl`.
