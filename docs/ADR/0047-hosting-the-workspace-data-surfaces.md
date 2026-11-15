# ADR 0047: Hosting the workspace data surfaces

- Status: Accepted
- Date: 2026-11-15
- Discussion: Phase 2 milestone 2.5 in [the roadmap](../ROADMAP.md).
- Supersedes: None. Completes [ADR-0045](0045-collaboration-api-and-remote-client.md), which deferred these surfaces.

## Context

ADR-0045 served the collaboration surface over HTTP and left five areas local:
memory and notes, conversation creation and context, context previews, prompt
snapshots, and templates. `RemoteClient` answered each with an empty result or a
`local_only` error, so a team on a server had no memory, no context meter, no
recorded prompts, and no templates. Milestone 2.5 also named package transfer
and a version-compatibility story.

Two of those surfaces could not simply be forwarded. Templates had no
`workspace_id` at all — local mode has one workspace, so sharing was harmless —
and a raw prompt-snapshot payload was readable by id from any workspace. Both
would have been cross-tenant leaks the moment they gained a route.

## Decision

### 1. Every new route resolves the workspace, then the role

The pattern from ADR-0045 holds: the caller's role in the workspace that owns
the row decides, and a row owned by another workspace reads as `404` rather than
`403` so existence cannot be probed. The role per surface:

| Surface | Read | Write |
| --- | --- | --- |
| Memory notes, entries | any member | write |
| Memory settings | any member | administer |
| Conversation settings, creation, clearing context | any member | write |
| Context meter and previews | any member | — |
| Prompt snapshots (list, read, settings, purge) | administer | administer |
| Templates | any member | administer (save, delete, instantiate) |
| Package export | administer | — |
| Package import | any signed-in user | — |

Entry scope is validated by `MemoryService` against the workspace it was built
with, so an entry cannot name another workspace's agent or channel even if the
caller asks.

### 2. Prompt snapshots are workspace-scoped in the store

`get_prompt_snapshot` and `get_prompt_snapshot_detail` now take the workspace.
A snapshot id is an LLM call id, and call ids are visible in usage rows, so an
unscoped read would have let any admin read another tenant's prompt by id. The
scope is a `WHERE` clause, not a post-fetch check, so a mismatch reads as
missing.

### 3. Templates gain a workspace, backfilled to the only one that existed

Migration `0019` adds a nullable `workspace_id` and backfills it from the single
workspace in the database — which is exactly what a pre-2.5 database has, since
the local bootstrap creates one. `upsert_template`, `get_template`,
`list_templates`, and `delete_template` all take the workspace, and the conflict
clause never reassigns `workspace_id`, matching `upsert_agent`. Built-in
templates stay compiled in and are visible everywhere.

Conversation, memory, prompt-snapshot, and context rows already carried a
workspace, so this is the only schema change the milestone needed.

### 4. Previews take their tool schemas from the caller

A preview reconstructs the request a turn *would* send, including the tool
schemas. The server has no tool registry — tools run on the client's machine —
so the schemas travel as a JSON-encoded query parameter. A malformed value is a
`400` rather than an empty tool list, because a silent empty list would make the
preview quietly wrong.

### 5. Package import is workspace-level, and the importer owns the result

`POST /api/v1/packages/import` is not nested under a workspace: it creates one,
exactly as `POST /workspaces` does, so any signed-in user may call it and becomes
the Owner. The package crate creates a placeholder owner so a package always
lands with an accountable member; on a server that placeholder cannot sign in, so
the route replaces it with the importing user rather than leaving a second Owner
nobody holds. Export is `can_administer` on the workspace and streams the archive
as a download. A package failure — corrupt, wrong checksum, unsupported version,
colliding ids — is a `400` carrying the package's own message; a store failure
stays the generic `500`.

### 6. What still needs the machine

Worktrees, in-process crew execution, live-turn cancellation, and process-local
session decisions remain `local_only`. They are not unimplemented server routes;
they are operations of the runtime that owns the files and the in-flight turn.
Every other `Client` method now works over HTTP.

## Alternatives considered

**Re-key every packaged id on import, so a collision is impossible.** Participants,
context entries, task assignees, and memory rows all name agent ids, and the
package format documents that conversation rows keep their ids. Re-keying means
rewriting the whole id graph inside the archive; refusing the collision (which
`cowbot-package` already does) is honest and small.

**Give templates a global scope on the server.** Simpler, but one workspace's
template would be every workspace's, and instantiating it would write agents into
whichever workspace asked. The migration is four lines.

**Filter prompt snapshots in the handler after an unscoped read.** The payload
would already have crossed the trust boundary into the process that renders it;
scoping the query is the boundary.

## Consequences

- `docs/SERVER.md` documents the new routes and what each role can do.
- `RemoteClient` reports an error only for the four machine-local operations;
  a desktop on a server now shows memory, context meters, recorded prompts, and
  templates instead of empty screens.
- A template written before this migration belongs to the first workspace in the
  database. A database that never ran the local bootstrap has no templates to
  attribute.
- Package import through the server always yields a workspace owned by the
  importing user, and the packager's original owner is not recreated as an
  account, because packages never carry credentials (ADR-0004).
