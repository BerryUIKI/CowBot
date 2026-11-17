# CowBot Template Format

> **Status**: Draft v0.1
> **Date**: 2026-11-03
> **Related**: `docs/ADR/0002-template-format-and-distribution.md`

## Overview

CowBot templates describe reusable agent and team configurations. They are JSON documents with a versioned schema, stored as `.cowtemplate` files when exported. Built-in templates are compiled into the binary; user templates are stored in SQLite and can be imported or exported without a server.

## Document Schema

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schema_version` | integer | no (default `1`) | Must equal the current supported version |
| `kind` | `"agent"` \| `"team"` | yes | Agent or team template |
| `id` | string | yes | Stable, unique template identifier |
| `name` | string | yes | Human-readable name |
| `description` | string | no | Short summary |
| `version` | string | no (default `"1.0.0"`) | Template content version |
| `tags` | string[] | no | Search/filter tags |
| `agents` | `AgentTemplate[]` | yes | See below |
| `tasks` | `TaskTemplate[]` | no | Team templates only |

### AgentTemplate

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Unique within the template |
| `role` | string | yes | Role description |
| `goal` | string | yes | Primary objective |
| `backstory` | string | no | Persona context |
| `model` | string \| null | no | Model identifier override |
| `tools` | string[] | no | Tool names available to the agent |
| `allow_delegation` | boolean | no | Whether the agent may delegate |
| `max_iterations` | integer | no (default `15`) | Must be greater than zero |

### TaskTemplate

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | string | yes | Unique within the template |
| `description` | string | yes | Work to perform |
| `expected_output` | string | yes | Deliverable definition |
| `agent` | string | yes | Must match an agent `name` |
| `depends_on` | string[] | no | Task IDs that must complete first |

## Validation Rules

- `schema_version` must match the supported version.
- Template `id`, `name`, and `version` must be non-empty.
- `kind: "agent"` requires exactly one agent and no tasks.
- `kind: "team"` requires at least one agent.
- Agent names must be unique; agent `role` and `goal` must be non-empty.
- Agent `max_iterations` must be greater than zero; tool names must be non-empty.
- Task IDs must be unique; `description` and `expected_output` must be non-empty.
- `task.agent` must reference an existing agent name.
- `depends_on` must reference existing task IDs, must not include the task itself, and must not form a cycle.

Validation failures are reported as configuration errors and prevent saving.

## Built-in Templates

The following templates are compiled into the binary and are read-only:

| ID | Kind | Purpose |
| --- | --- | --- |
| `builtin-software-team` | team | Planner, coder, and reviewer workflow |
| `builtin-research-team` | team | Planner, researcher, and writer workflow |
| `builtin-general-assistant` | agent | CowBot, the primary assistant created by default in an empty desktop workspace |

Users can copy a built-in template, modify it under a new ID, and save it as a user template.

## Import and Export

- Export produces a single pretty-printed JSON file with the `.cowtemplate` extension.
- Import parses, validates, and persists the document as a user template.
- Built-in IDs cannot be overwritten or deleted through the template service.
- Templates only reference tool names and model identifiers; they never embed executable code.

## Example

```json
{
  "schema_version": 1,
  "kind": "team",
  "id": "user-docs-team",
  "name": "Documentation Team",
  "description": "A small team for writing and reviewing documentation.",
  "version": "1.0.0",
  "tags": ["docs"],
  "agents": [
    {
      "name": "Writer",
      "role": "Technical Writer",
      "goal": "Produce clear documentation.",
      "tools": ["read_file", "write_file"],
      "max_iterations": 15
    },
    {
      "name": "Reviewer",
      "role": "Documentation Reviewer",
      "goal": "Check accuracy and clarity.",
      "tools": ["read_file"],
      "max_iterations": 15
    }
  ],
  "tasks": [
    {
      "id": "draft",
      "description": "Draft the documentation.",
      "expected_output": "A complete documentation draft.",
      "agent": "Writer",
      "depends_on": []
    },
    {
      "id": "review",
      "description": "Review the draft.",
      "expected_output": "Review comments and approval.",
      "agent": "Reviewer",
      "depends_on": ["draft"]
    }
  ]
}
```
