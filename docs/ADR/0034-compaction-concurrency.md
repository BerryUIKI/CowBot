# ADR 0034: Compaction concurrency and cutoff insertion

- Status: Accepted
- Date: 2026-11-12
- Discussion: [#179](https://github.com/BerryUIKI/CowBot/issues/179)
- Supersedes: None. Amends the compaction semantics in [ADR-0030](0030-conversation-sessions-and-context.md).

## Context

A compaction is two operations with a model call between them: the service
reads a branch and builds a summary request, the provider returns a summary,
and the store excludes the summarized entries and writes the summary. During
the model call a conversation turn, another compaction, or a clear can change
the same branch.

The original write appended the summary with `MAX(seq) + 1`. That was safe
against the unique index but wrong for ordering: any entry appended during the
model call ended up *before* the summary that was built from earlier history.
Two compactions could also both read the same prefix and write two summaries.

Three constraints shape the fix:

- `context_entries` has `UNIQUE (channel_id, agent_id, seq)`. SQLite unique
  indexes are not deferrable, and a single `UPDATE ... SET seq = seq + 1`
  conflicts with the next row mid-statement.
- A compaction can be started from the desktop's context meter, the pre-send
  capacity check, a group turn's automatic path, or the service API. Those
  paths use different `ConversationService` instances in one process.
- The store is also opened by tests with a single in-memory connection, so the
  transaction must stay short and must never be held across the model call.

## Decision

### 1. One process-wide per-branch lock registry

`CompactionLocks` holds a `HashSet<(ChannelId, AgentId)>` behind a
`std::sync::Mutex`. `try_acquire` returns an RAII `CompactionGuard` that
removes the key on drop, including on an error, early return, or panic. The
service claims the branch **before** reading entries; a request that finds the
branch held returns an `in_progress` outcome without another model call.

`LocalClient::build` creates one `Arc<CompactionLocks>` and injects it into
both its `ConversationService` and the `ChannelAgentService`'s conversation
service. Clones share the registry. The lock is an in-process duplicate
prevention; the store guard in the next section is the correctness boundary
for a second process.

### 2. A guarded, cutoff-aware store write

`Store::compact_context` receives the first kept entry's sequence as
`cutoff_seq` and applies the following inside one transaction started with
`BEGIN IMMEDIATE`, so the write lock is held before the reads and a concurrent
appender waits on the busy timeout instead of surfacing
`SQLITE_BUSY_SNAPSHOT`:

1. Reject a branch with non-positive sequences. Imported packages can carry
   arbitrary sequence numbers, and the renumber below uses negatives as a
   staging range. The workspace importer normalizes such a branch to start at
   1 before writing; the service also checks before the model call so a
   hand-edited database does not pay for a summary that cannot be written.
2. Check that the active entries before the cutoff are exactly the summarized
   ids. A mismatch means another writer changed the prefix; roll back and
   return `CompactionWrite::Stale`.
3. Compare-and-set each summarized entry from active to excluded. Any row that
   was already excluded also produces `Stale` and rolls back.
4. Verify the cutoff row still exists and is active. If the branch is empty,
   return `CompactionWrite::Empty`; otherwise `Stale`.
5. Shift the tail one position later through a disjoint negative range:
   `SET seq = -seq WHERE seq >= cutoff`, then
   `SET seq = -seq + 1 WHERE seq < 0`. Both updates are injective and cannot
   collide with the unique index.
6. Insert the summary at `cutoff_seq`.
7. Recompute `tokens_before` and `tokens_after` from the branch as written,
   including entries appended during the model call, and record the context
   event with those totals.

The summary therefore sits immediately before the kept tail, and new entries
continue after it with `MAX(seq) + 1`.

### 3. Additive outcome fields and a bounded model call

`CompactionOutcome` gains serde-default `in_progress: bool` and
`reason: Option<String>`. Old payloads still deserialize, and the desktop can
show "already running", "the context changed", "the context was cleared", or
"nothing to summarize" as notices instead of an error or the generic
"Nothing to compact yet."

The summarizer call is bounded by a 120-second timeout. A timeout returns a
non-compacted outcome with a reason; the branch lock is released when the
outcome is returned, so a hung provider cannot hold the branch until restart.

### 4. A summary is not re-summarized on its own

The cutoff insertion moves the summary to the oldest position in the active
branch. `compaction_split` now returns zero when every entry it would
summarize is already a `Summary`, so an immediate second compaction is a
no-op instead of replacing one summary with another. Once new history
accumulates, the summary is folded into the next summary with that history.

## Alternatives considered

- **An explicit position or anchor column** ordered as `COALESCE(anchor_seq,
  seq)`. Avoids rewriting the tail, but needs a migration, a package-format
  field, and a tie-break against the entry still at the cutoff. Rejected for
  this change; it remains the fallback if rewriting the tail becomes too
  expensive.
- **Append the summary at `MAX(seq) + 1` with a stale check.** Preserves the
  old layout but keeps the ordering bug: entries appended during the model
  call still precede the summary.
- **A database transaction around the model call.** Not viable: it would hold
  a write transaction for the duration of a provider request.
- **A channel-wide lock.** Too coarse; independent participant branches should
  compact concurrently.
- **A global process lock.** Same problem, and it would serialize unrelated
  conversations.

## Consequences

- A compaction summary is now ordered at the cutoff, so the model sees
  `[older excluded history] [summary] [kept tail] [new entries]`.
- Renumbering changes the sequence numbers of the kept tail. No caller
  persists the result of `append_context_entry` across a later compaction and
  the package format stores the rows, not a separate sequence map, but code
  must not treat `seq` as stable identity.
- An imported package with zero or negative sequence numbers is normalized per
  branch at import, shifting the branch minimum to 1. A hand-edited database
  that still has non-positive rows returns a non-compacted outcome before the
  model call rather than a turn-aborting error; the store guard remains the
  last line.
- Per-participant clear excludes every active row of the branch in one
  `BEGIN IMMEDIATE` statement. A summary committed concurrently is caught by
  that update instead of remaining active after the clear reports success.
- The per-branch lock is shared by the desktop, the channel agent service, the
  pre-send path, and group auto-compaction. A second process still races only
  at the store guard; the guarded write aborts instead of writing out of
  order.
- Automatic compaction failures are logged and never abort a group turn. When
  a branch is already compacting, the capacity skip names that state instead
  of telling the user to compact a branch that is already compacting.
- The transaction remains short (no model call inside it), so the
  single-connection test pool and WAL writers are not blocked for long.
- Tests cover cutoff insertion, appended entries, stale prefixes, non-positive
  sequences, concurrent compactions on one branch, concurrent compaction of
  independent branches, group auto-compaction while a manual compaction holds
  the branch, the in-progress skip notice, the summary timeout, and a clear
  during compaction.
