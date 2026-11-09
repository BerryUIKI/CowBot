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
tasks.json
runs.jsonl
audit.jsonl
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
  agent's direct conversation (ADR-0024).
- `messages.jsonl`: one channel message per line, sorted by `created_at`.
- `tasks.json`: array of tasks.
- `runs.jsonl`: one run per line.
- `audit.jsonl`: one audit event per line, newest first.

## What is never written

Passwords and password hashes, API tokens, session tokens, model credentials,
and any environment variables. Credentials are reconfigured after import.

## Import semantics (v1)

- The archive is read completely, every path is checked against the v1 layout,
  and every checksum is verified before anything is written.
- A new workspace is created with a fresh id and the name suffixed
  `(imported)`. Messages and templates keep their original ids.
- Package files written before agent direct conversations existed have no
  `agent_id` on their channels; they import as group channels, and the desktop
  provisions the missing conversations on first launch.
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
