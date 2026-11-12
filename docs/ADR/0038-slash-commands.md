# ADR 0038: Slash commands as desktop actions

- Status: Accepted
- Date: 2026-11-13
- Discussion: [#185](https://github.com/BerryUIKI/CowBot/issues/185), [#186](https://github.com/BerryUIKI/CowBot/issues/186), [#187](https://github.com/BerryUIKI/CowBot/issues/187)
- Supersedes: None. Extends the composer interaction from [ADR-0022](0022-channel-mention-turns.md).

## Context

The composer understands `@mentions` but treats a leading slash as ordinary
text. Every action a user reaches through a right-click menu, the context
popover, or Settings — compact a branch, clear context, rename or pin a chat,
switch the model, change the approval mode, remember a fact — lacks a
deterministic keyboard fast path.

Two properties matter. First, the command surface has to be shared: the CLI and
the future remote client should recognise the same names and argument rules, so
the catalogue and the parser cannot live in the React frontend. Second, a
command is not a message. Posting `/pin` into a channel would put it in the
timeline, in the sidebar preview, and, in a conversation, in the context branch,
where it would shift the next turn and the next compaction.

## Decision

### 1. The catalogue and parser live in `cowbot-core`

`crates/cowbot-core/src/command.rs` holds a static `COMMANDS` catalogue and a
pure, synchronous `parse_command`. It has no store, service, or state
dependency, so the CLI and a future remote client can adopt it unchanged. The
desktop executes commands; it does not decide what they are called or how their
arguments split.

Parsing recognises a command only at the start of the input, resolves names and
aliases case-insensitively, splits arguments quote-aware, and keeps `raw_args`
for free-text commands. It validates required-argument arity and nothing else:
value validation, completion, and error copy belong to the caller that has the
roster, the provider list, and the notice surface.

`CommandArg` serializes flat — `{placeholder, required, kind, choices}` — by
hand. A serde-derived internally tagged enum cannot encode the `Choice`
variant's slice, so `serde_json::to_string(command_catalogue())` failed with
`cannot serialize tagged newtype variant ... containing a sequence` and the
first Tauri command returning the catalogue would have failed at runtime.

### 2. The desktop fetches the catalogue once and parses once per Enter

`list_commands` returns the catalogue across IPC once per frontend session, and
`parse_command` runs once per Enter press while the draft starts with `/`.
Typing never costs a round trip: the menu filters the local copy.

A failed fetch is not cached, so a later attempt can recover. Until it succeeds,
slash mode stays off, drafts are sent as ordinary messages, and the failure is
reported once in the notice area.

### 3. A command is an action, never a message

`desktop/src/commandActions.ts` maps one parsed invocation to a
`CommandAction`. The mapping is pure, so it is testable without a renderer,
and the desktop stays the single place that changes state. No handler posts a
channel message; none appends a context entry; feedback uses the existing
notice and error surfaces.

Commands that read or write a branch require a user-created conversation,
because that is where the desktop loads conversation context. In an agent chat
or a group channel they return an explanatory notice instead of failing
silently.

Destructive commands keep the confirmations the UI already had: `/clear` asks
before clearing, `/delete` opens the existing dialog, `/stop` names the count
before cancelling, and `/approval trusted` keeps the every-time bypass warning.

### 4. Unknown input is never silently sent

A single unmatched `/token` shows an inline hint and is not sent. Escape marks
the draft as a plain message for the current contents, so the next Enter sends
it. A line with whitespace after an unknown first token is sent as an ordinary
message, so paths such as `/usr/bin/env python` still work.

### 5. The catalogue is the contract, not the implementation

The catalogue states each command's name, aliases, usage, summary, arguments,
and target. Execution is desktop-only in v1. When the CLI gains a first-class
command path, it adopts the parser; when remote mode lands, it adopts both and
supplies its own execution. Neither change alters the catalogue format.

## Alternatives considered

**Parse commands in the frontend.** Rejected: the CLI and remote clients would
need a second parser, and the two would drift on aliases, quoting, and arity.

**Post command output as a channel message.** Rejected: it pollutes the
timeline and the context branch, and the sidebar would preview a command even
though the user performed an action.

**Implement handlers inside `App.tsx`.** Rejected: the migration, rename, and
context handlers are large enough that inline dispatch would bury the command
logic between render branches, and the command-to-action mapping would not be
testable without rendering.

**Validate choice values in the parser.** Rejected for v1: the parser has no
roster and no provider list, and the aliases a UI accepts (`auto` for
`safe_auto`) are presentation, not wire format.

## Consequences

- The composer gains a keyboard surface for 15 actions with no new store or
  service API.
- The desktop carries two representations of the catalogue — the Rust constant
  and the fetched JSON — but only the Rust one is authored; the JSON is derived.
- A command that needs a new capability (a side question, a CLI executor) must
  add its handler where the capability lives, not in the parser.
- Unknown-token handling makes `/`-prefixed text safe to type, at the cost of
  one Escape press before sending a line that starts with a path-like slash.
- The catalogue's `target` field is advisory: the desktop enforces conversation
  commands, and another client is expected to enforce it too.
