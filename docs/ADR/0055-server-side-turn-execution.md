# ADR 0055: Server-side turn execution

- Status: Accepted
- Date: 2026-11-21
- Discussion: Phase 3 workstream 3.3 prerequisite, tracked as
  [#269](https://github.com/BerryUIKI/CowBot/issues/269) under the
  [IM integration umbrella #268](https://github.com/BerryUIKI/CowBot/issues/268).
  Plan: [P3-S0 server turns](../../docs/plans/P3-S0-server-turns.md).
- Supersedes: None. Amends the "only one machine runs agent turns" gap noted in
  [ADR-0045](0045-collaboration-api-and-remote-client.md) and
  [ADR-0053](0053-desktop-server-mode.md).

## Context

Since ADR-0045 the server has served the collaboration surface — channels,
messages, tasks, approvals — and `RemoteClient` has implemented the `Client`
trait over it. What it never did was answer a message. In server mode the
desktop's send path returned before running a turn:

```rust
// The reply runs in-process, so server mode posts without a local turn.
let Some(client) = state.local_client.clone() else {
    return Ok(message);
};
```

So a user signed in to a server could mention an agent in a channel and nothing
answered. That was tolerable while the server was a read-and-collaborate surface
for humans, with execution belonging to whichever desktop had the model
provider. It stopped being tolerable when the IM integration work reached
planning: a bot must answer at 3 a.m. with nobody's desktop open.

The server had no `cowbot-llm` dependency and no `LlmProvider` reference
anywhere in the crate, and model providers live in the desktop's
`app_data_dir()/config.json` with keys that never cross to the frontend. The
blocker was not the turn logic — `ChannelAgentService::reply` has always taken
its provider, tool registry, and model as arguments, and the CLI constructs all
three itself — but the absence of a provider and a place to run.

## Decision

### 1. The deployment configures one provider, not the workspace

`COWBOT_SERVER_LLM_PROTOCOL`, `_BASE_URL`, `_API_KEY`, and `_MODEL` are read by
`ServerConfig::from_env` through `ServerLlmConfig`. The four are
all-or-nothing: setting none means the server runs no turn and behaves exactly
as before; setting some but not all is a startup error naming the missing one,
following the rule the other eight `COWBOT_SERVER_*` variables already apply to
a mistyped value.

A per-workspace provider table, mirroring the desktop's Settings → Models, is
the obvious next step and is explicitly not this one. It would need storage, a
Settings surface, a migration, and a key-at-rest story, none of which is needed
to prove the topology. A deployment that outgrows one provider gets the table
later; the executor below does not change shape when it does.

`COWBOT_SERVER_LLM_PROTOCOL=mock` builds `MockLlmProvider::echo()`. This is not a
debug convenience: it is what makes every acceptance test for this feature
runnable in CI without a key, including the multi-agent path.

### 2. The provider clients are the desktop's

`ServerLlmConfig::provider` returns the same `OpenAiCompatibleProvider`,
`AnthropicProvider`, and `ResponsesProvider` the desktop builds for its saved
providers (ADR-0021). A deployment and a desktop talking to one endpoint
therefore behave identically, and a protocol bug is not fixed twice.

### 3. One executor per process, one service per workspace

`TurnExecutor` lives in `AppState` and is built once in `main.rs`. It is
deliberately *not* constructed per request the way `AppState::channels()` and
friends are: `ChannelAgentService` owns the open-turn registry (`TurnRunner`,
which is what makes `cancel_turn` work) and, through `ConversationService`, the
compaction branch locks (ADR-0034). A per-request instance would give every
request its own registry and silently break both.

`ChannelAgentService` is workspace-scoped, so the executor keeps a
`HashMap<WorkspaceId, Arc<ChannelAgentService>>` behind a `Mutex` held only long
enough to look an entry up. A unit test pins the reuse with `Arc::ptr_eq`.

### 4. The turn runs after the response

`channels::post_message` persists and publishes the message, then calls
`TurnExecutor::spawn_reply`, and returns `201` with the message as it always
did. The answer reaches every client as the `MessagePosted` event the turn
already publishes, forwarded by the existing workspace socket. No route, no
payload, and no `Client` trait method changes, which is why `RemoteClient` needs
no update and the desktop sees server-run answers without new code.

A turn that cannot start posts a system notice naming the failure rather than
failing silently; a client waiting for an answer that will never come is the
worst outcome available.

### 5. The tool profile is narrow on purpose

The executor registers `execute_code` over the existing `SubprocessSandbox`, and
`read_file` / `write_file`, all rooted at `COWBOT_SERVER_WORKSPACE_DIR`
(default `cowbot-workspace`), which is created at startup.

The desktop's `workspace_tools` — create a channel, instantiate a team, queue a
task, reach a registered git project — are intentionally absent. They assume a
local project folder and a local client that a server does not have, and every
one of them is a write. A server turn is therefore less capable than a
desktop turn, and that is the intended trade: the server runs code beside other
people's data, so it gets the smallest tool set that still answers questions.

Approvals still apply: the executor shares the workspace's `ApprovalService`
(through the service's session decisions), so a risky call in a server turn
creates the same pending approval the desktop already shows and the API already
lists at `GET /workspaces/{w}/approvals`.

### 6. Streaming and cancellation stay deferred

`stream.rs` admits `TurnStarted` by its target and returns `false` for every
later event in a turn, because those carry only a `turn_id`. A remote client
therefore sees the bubble open and then nothing until the turn settles and its
message is posted. That is a visible limitation and it is accepted here:
streaming needs a turn→channel map in the socket scope, which is independent of
the executor and does not block any IM adapter.

Cancellation has no route either — `cancel_turn` is deliberately local-only
(ADR-0053) — so a server-run turn is stopped by cancelling its task. Both are
noted in `docs/LIMITATIONS.md`.

## Alternatives considered

- **A per-workspace provider table now.** More faithful to the desktop, much
  larger, and it does not change whether the topology works. Deferred.
- **A sidecar process that runs turns.** Duplicates scheduler wiring, the
  approval gate's process-local `SessionDecisions`, and the bus subscription.
  Rejected.
- **Keep turns desktop-only and have an IM bridge open `LocalClient`.** Works
  for one always-on machine with a local database, and cannot see messages a
  remote user posts because those never reach that database. Rejected as a
  milestone; named in the P3 plan as a demo at best.
- **Give the server the full desktop tool set.** Simplest to implement and the
  worst trade: it hands a shared deployment channel/team/task creation and file
  access outside a directory it owns. Rejected.

## Consequences

**Benefits.** A server deployment answers a mention with no desktop running,
which is the prerequisite for every IM adapter. Protocol behavior is identical
to the desktop because the clients are. A server-run turn records usage, prompt
snapshots, and approvals through the same service paths the desktop uses, so
audit does not split by transport.

**Costs and risks.**

- A deployment can now spend money on a model without a human present. The
  approval gate bounds tool execution, not token spend; a per-workspace budget
  remains unbuilt.
- The server becomes a turn executor, so the subprocess sandbox runs beside
  other people's data. It is not an isolation boundary (this was already true on
  a desktop) and the narrow tool set is the mitigation, not a fix.
- Group routing's percentage capacity gate needs `participant_capacity`, which
  the desktop computes from model metadata it owns. The executor passes none for
  now, which means the group skip path is inactive on the server. Recorded as a
  gap rather than solved here.
- The single-participant capacity check still lives in the desktop
  (`commands.rs`), so a server turn in a single-participant conversation can
  overrun its budget. Moving it into `ChannelAgentService` is the planned fix
  (S0.4) and is not part of this change.

**Follow-ups.** Streaming over the socket (S0.6); moving the single-participant
capacity check into the service; a per-workspace provider table; a token budget
per workspace.
