# ADR 0040: Generated desktop types

- Status: Accepted
- Date: 2026-11-13
- Discussion: [#184](https://github.com/BerryUIKI/CowBot/issues/184)
- Supersedes: None. Constrains how [ADR-0009](0009-desktop-ipc-and-state.md) types cross the bridge.

## Context

`desktop/src/types.ts` mirrored the Rust types returned by Tauri commands by
hand. A serde rename, a new optional field, or a widened enum compiled fine on
both sides and failed at runtime — usually as `undefined` rendered into a
component, occasionally as an IPC deserialization error.

The mirror also drifted in ways nobody noticed. Regenerating it for this change
immediately exposed three: `u64` fields were typed `number` while ts-rs
defaulted them to `bigint`, several snapshot fields carry
`#[serde(skip_serializing_if = "Vec::is_empty")]` and are absent from the wire
rather than empty, and the token-counter label is optional.

## Decision

### 1. The Rust definition is the source of truth

Serializable core types opt into generation with one attribute on their
definition:

```rust
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
pub struct Task { /* ... */ }
```

The `bindings` feature is off by default, so the runtime build carries no
codegen dependency and CI's existing workspace jobs are unaffected. Opting in
is a deliberate act: a type the desktop never sees stays out of the generated
surface, and the generated directory is not a dump of the whole crate.

### 2. The generated output is committed

`desktop/src/generated/` holds one file per type plus a barrel. It is checked
in, so a Rust field rename shows up as a desktop change in the same pull
request that causes it.

`desktop/scripts/generate-types.sh` regenerates it;
`desktop/scripts/check-types.mjs` fails when the working tree's generated
output differs from what is committed. CI runs both before `pnpm run build`,
which means a rename that the desktop has not adopted fails the desktop job
rather than shipping.

### 3. `types.ts` keeps only what the desktop owns

The file is now two halves: a re-export block from `./generated`, then the view
types and presentation helpers that exist only in the desktop —
`ConversationParticipantView`, `ContextPreview`, the composer's
`CommandInfo`, `approvalModes`, the status label functions, and so on.

A local declaration that shadows a generated name is a deliberate exception
with a comment: the meter's `ConversationContext` is the Tauri view, not the
core projection, and the composer's `CommandArgKind` is a flattened string
union, not the tagged Rust enum.

### 4. Large integers are numbers on the wire

`.cargo/config.toml` pins `TS_RS_LARGE_INT = "number"`. Tauri serializes `u64`
as a JSON number, so `bigint` would be wrong at the boundary; ts-rs's default
is only correct for code that keeps the JSON as text.

## Alternatives considered

**`tauri-specta`.** Generates typed command bindings and event types, which is
more than this issue needs and would restructure every `#[tauri::command]`.
The view types the desktop actually imports are core types passed through
commands, so generating those covers most of the surface with a much smaller
change. Worth revisiting if command *arguments* start drifting too.

**Runtime validation with Zod or valibot.** Catches the same drift, but only for
payloads that reach a validator, and still requires the shape to be written
twice.

**Generate in CI only and never commit.** The diff would disappear from review,
which is the one place a type change is easiest to judge.

**Do nothing.** Current behaviour: drift is found by a user, at runtime.

## Consequences

- Every opt-in type must keep its `ts(export)` attribute; deleting it silently
  removes the binding from the barrel, and the desktop import breaks loudly,
  which is the intended failure mode.
- The generated directory adds ~95 small files. They are mechanical and never
  edited by hand; a stale one is caught by `check:types`.
- A type the desktop wants but that lives in `cowbot-client` or
  `cowbot-service` rather than `cowbot-core` is not covered yet. Those crates
  do not carry the `bindings` feature; adding it is the same one-attribute
  move when a case appears.
- View types stay hand-written on purpose. They are shaped for rendering, not
  for the wire, and generating them would tie the UI's shape to a Rust struct
  that exists for IPC.
- The CI desktop job now needs a Rust toolchain before the frontend build. It
  already installed one for the Tauri gates, so the ordering is the only
  change.
