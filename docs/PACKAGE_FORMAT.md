# `.cowpkg` package format

A `.cowpkg` file is a ZIP container that moves a CowBot workspace between
installations. The decision record is
[ADR 0004](ADR/0004-import-export-package.md); this document describes the
concrete v1 layout.

## Layout

```
manifest.json
workspace.json
agents.json
templates.json
channels.jsonl
messages.jsonl
channel_participants.jsonl
conversation_settings.jsonl
context_entries.jsonl
context_events.jsonl
tasks.json
runs.jsonl
audit.jsonl
permission_rules.jsonl
```

JSON files hold a single document; `.jsonl` files hold one JSON value per line
for streaming. Dictionary keys use the snake_case names the Rust types
serialize.

## `manifest.json`

| Field | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `cowbot-package`. |
| `schema_version` | integer | `1` for this layout. |
| `kind` | string | `workspace` or `template`. |
| `app_version` | string | Version of the exporting build. |
| `created_at` | RFC 3339 timestamp | When the package was written. |
| `exported_by` | object | `user_id`, `user_name`, `workspace_id`, `workspace_name`. |
| `checksums` | object | Archive path to lowercase hex SHA-256 of the uncompressed file. |

## Content documents

- `workspace.json`: `id`, `name`, `created_at`.
- `agents.json`: array of agent profiles.
- `templates.json`: array of template documents.
- `channels.jsonl`: one channel per line; an optional `agent_id` marks the
  agent's direct conversation (ADR-0024), `pinned` records whether the user
  pinned it (ADR-0026), and `kind` distinguishes group channels, direct
  conversations, and user-created conversations (ADR-0030).
- `messages.jsonl`: one channel message per line, sorted by `created_at`.
- `channel_participants.jsonl`: one participant per line, keyed by channel and
  agent, with its ordered `position` in the roster; used by user-created
  conversations (ADR-0030, ADR-0032).
- `conversation_settings.jsonl`: one settings row per conversation: mode,
  fixed provider and model, router flag, default responder, automatic-compaction
  flag, and threshold.
- `context_entries.jsonl`: one append-only context entry per line, with its
  branch sequence, role, kind, content, optional tool calls, optional
  `author_agent_id` for mirrored peer replies, exclusion flag, estimated tokens,
  and timestamp. Token counts are the model-independent estimates written with
  each entry; the live context meter recomputes for the conversation's model
  and labels exact versus estimated counts (ADR-0035). Import shifts a branch
  whose minimum sequence is zero or negative so it starts at 1, preserving
  within-branch order (ADR-0034).
- `context_events.jsonl`: one compaction or clear event per line with token
  counts before and after.
- `tasks.json`: array of tasks (an optional `channel_id` keeps conversation
  attribution).
- `runs.jsonl`: one run per line, with the agent profile snapshot taken when
  the run started. Packages written before the snapshot omit it; import keeps
  the run and the preview falls back to the stored agent.
- `audit.jsonl`: one audit event per line, newest first; optional `source`
  records whether a decision came from a rule, model, session, human, policy,
  or the system (ADR-0003).
- `permission_rules.jsonl`: one workspace allow / ask / deny rule per line
  (ADR-0003). Rules carry their tool, optional command glob, decision, enabled
  flag, and source.
- `memory_notes.jsonl`: one note per line. A note without `channel_id` is the
  workspace note; one with it is that conversation's note. Content is redacted
  before export (ADR-0033).
- `memory_entries.jsonl`: one memory entry per line with its scope
  (`agent_id`, `channel_id`), kind, content, source, status, pinning, token
  estimate, optional source reference, and recall statistics. Content is
  redacted before export (ADR-0033).
- `memory_settings.jsonl`: one workspace memory settings row with the
  extraction and recall flags and both token budgets (ADR-0033).

## What is never written

Passwords and password hashes, API tokens, session tokens, model credentials,
and any environment variables. Credentials are reconfigured after import.

Local project registry entries (repository paths) and task worktree records
(checkout paths, branches, and status) describe one machine and are not
exported either; a task's optional `run_group` link is part of the task record
and does export. Worktrees are recreated only by running the task on a machine
where the project is registered.

Group conversation turn rows (`conversation_turns`) are the routing audit of one
machine and are not exported. Packages carry the participant roster, the default
responder, and authored peer entries, so an imported conversation keeps its
routing behavior and history; route chips may be absent until the imported
workspace produces new turns (ADR-0032).

LLM prompt snapshots (`llm_prompt_snapshots`) are device-local audit evidence
and are never written into a package, not even as a redacted digest. They expire
on their own retention clock instead (ADR-0037). A package that carries such a
file is rejected as an unexpected path, like any other path outside this layout.

## Import semantics (v1)

- The archive is read completely, every path is checked against the v1 layout,
  and every checksum is verified before anything is written.
- A new workspace is created with a fresh id and the name suffixed
  `(imported)`. Messages and templates keep their original ids.
- Package files written before agent direct conversations existed have no
  `agent_id` on their channels; they import as group channels, and the desktop
  provisions the missing conversations on first launch.
- Imported permission rules are created disabled and marked `imported`; the
  user reviews and enables each rule before it can decide a tool call.
- The four conversation files are optional on import. Packages written before
  ADR-0030 do not contain them; their channels import without conversation
  settings or context.
- Conversation participants, settings, context entries, and events keep their
  ids, sequence numbers, and timestamps on import. A channel whose imported
  participants all have position zero was written before group conversations;
  import re-sequences them in file order, which is the exporting machine's
  creation order (ADR-0032).
- Imported group conversations keep their ordered roster, default responder,
  and authored peer entries. Conversation turn rows are never imported because
  they are not packaged.
- The three memory files are optional on import. Notes and entries are re-keyed
  for the new workspace, redacted again, and dropped when their agent or channel
  is not part of the package. Unknown kinds normalize to `note` and unknown
  statuses to `archived`, so a foreign package cannot create active memory with
  an unrecognized state (ADR-0033).
- The importing user is created as the owner of the new workspace, with an
  address of the form `<user id>@imported.local` and no password.
- Existing local workspaces are never modified or merged.

## Limits

- A package may not exceed 512 MiB.
- Export includes at most the 10,000 newest messages per channel and the
  10,000 newest audit events; the limits are constants in `cowbot-package`.

## Compatibility

`schema_version` increases monotonically. The importer accepts only the version
it writes and rejects anything else with a clear error, so a future format
change must ship a migration path before older packages can be opened.
