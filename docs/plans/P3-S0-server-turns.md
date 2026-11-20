# S0 — Server-side turn execution (implementation plan)

- Status: Planned — not started
- Date: 2026-11-21
- Issue: [#269](https://github.com/BerryUIKI/CowBot/issues/269)
- Umbrella: [#268](https://github.com/BerryUIKI/CowBot/issues/268)
- Research: [P3 IM integrations](P3-im-integrations.md) §3
- Related: [ADR-0022](../ADR/0022-channel-mention-turns.md),
  [ADR-0041](../ADR/0041-turn-streaming-and-cancellation.md),
  [ADR-0045](../ADR/0045-collaboration-api-and-remote-client.md),
  [ADR-0034](../ADR/0034-compaction-concurrency.md)

## 1. Why this blocks everything

The server has no LLM client. `crates/cowbot-server/Cargo.toml` does not depend
on `cowbot-llm`, and nothing in `crates/cowbot-server/src` references
`LlmProvider`. In server mode the desktop's send path returns before running a
turn:

```rust
// The reply runs in-process, so server mode posts without a local turn.
let Some(client) = state.local_client.clone() else {
    return Ok(message);
};
```

So `@Agent` in a server channel produces no answer. Every IM adapter needs the
opposite topology, so S1–S7 wait on this.

## 2. What the code already gives us

The turn logic takes its dependencies as arguments and needs no change:

| Piece | Signature | File |
| --- | --- | --- |
| Channel turn | `reply(channel, message, llm, tools, model, &options)` | `cowbot-service/src/channel_agents.rs:228` |
| Conversation turn | `reply(channel, message, llm, tools, &options)` | `cowbot-service/src/conversations.rs:455` |
| Options | `ChannelTurnOptions` (approval level, concurrency, capacity) | `channel_agents.rs:48` |
| Report | `ChannelTurnReport` (replied/failed/skipped, route, turn id) | `channel_agents.rs:130` |
| Workspace from channel | `Store::channel_workspace(channel_id)` | used by `server/src/channels.rs:292` |
| Mock provider | `MockLlmProvider::echo()` | `cowbot-llm` |

`cowbot-service` already depends on `cowbot-llm` and `cowbot-tools`, so the
server only needs the same two dependency lines; the CLI proves the shape works
by constructing provider, tools, and model itself
(`crates/cowbot-cli/src/main.rs:561-590`).

## 3. Constraints found in the code

These six decide the design. Each is a fact about the current tree, not a
preference.

1. **Services are constructed per request.** `AppState::channels()` and friends
   build a fresh service on every call (`server/src/router.rs:51`). That is fine
   for stateless services, but `ChannelAgentService` owns two pieces of
   process-wide state: its `TurnRunner` (the open-turn map that makes
   `cancel_turn` work) and, through `ConversationService`, its
   `CompactionLocks` (`conversations.rs:113-122`). A per-request instance would
   give every request its own registry, so cancellation and branch locking would
   silently stop working. **The executor must hold one instance per workspace.**
2. **`ChannelAgentService` is workspace-scoped** — `new(store, bus,
   workspace_id)` — so a single shared instance cannot serve every workspace.
   It needs a `HashMap<WorkspaceId, Arc<ChannelAgentService>>`.
3. **Turn deltas are not routable over the socket.** `stream.rs:183-194` admits
   `TurnStarted` by its target and returns `false` for every later event in the
   turn, because those carry only a `turn_id`. Today that is invisible: the
   desktop in local mode drives its live bubble through Tauri's in-process
   emitter. Once the server runs turns, a remote client will see `TurnStarted`
   and then nothing — a bubble that never fills. Milestone one can accept
   "reply appears when the turn finishes"; streaming needs a turn→channel map in
   the socket scope (S0.6).
4. **Single-participant auto-compaction lives in the desktop.** The pre-send
   threshold check and the context-full block are in
   `desktop/src-tauri/src/commands.rs:236-280`, not in the service. Group
   conversations auto-compact inside `ConversationService`
   (`conversations.rs:842`). So without care, a server turn in a
   single-participant conversation would run over budget or fail opaquely. The
   executor must either replicate the check or the check must move into the
   service (preferred, and it also removes a desktop/server divergence).
5. **There is no compaction route on the server**, and `RemoteClient` has no
   compaction method. If the executor compacts, the operation must live in the
   service so both transports reach it.
6. **Cancellation has no route.** `cancel_turn` is deliberately local-only
   (`remote.rs:1180`). A server run turn can therefore only be stopped by
   cancelling the task behind it. Acceptable for S0; note it in the ADR.

## 4. Work breakdown

Each step is independently reviewable and leaves the tree green.

### S0.1 — Configuration

`ServerConfig` gains four fields and `from_env` reads them, following the
existing eight-variable pattern (a value that does not parse exits, an unset
value keeps the default):

| Variable | Default |
| --- | --- |
| `COWBOT_SERVER_LLM_PROTOCOL` | unset — no turn execution |
| `COWBOT_SERVER_LLM_BASE_URL` | unset |
| `COWBOT_SERVER_LLM_API_KEY` | unset |
| `COWBOT_SERVER_LLM_MODEL` | unset |

Unset everywhere means behaviour identical to today. Set means the server
answers. Half-set is a startup error naming what is missing, not a silent
fallback.

*Verify*: unit tests for the parse matrix; a run with nothing set starts
unchanged.

### S0.2 — The provider

A small builder that maps the protocol string onto the three existing clients
(`OpenAiCompatibleProvider`, `AnthropicProvider`, `ResponsesProvider`) and falls
back to `MockLlmProvider::echo()` when `COWBOT_SERVER_LLM_PROTOCOL=mock`. The
mock is not a convenience: it is what makes S0 testable in CI without a key.

*Verify*: each protocol string produces the expected client; an unknown string
is a startup error.

### S0.3 — The executor

New module `crates/cowbot-server/src/turn_executor.rs`:

```rust
pub struct TurnExecutor {
    store: Store,
    bus: EventBus,
    llm: Arc<dyn LlmProvider>,
    model: String,
    tools: ToolRegistry,
    session_decisions: Arc<SessionDecisions>,
    /// One service per workspace: it owns the open-turn map and the branch
    /// locks, so a per-request instance would break both (§3, constraint 1).
    agents: Mutex<HashMap<WorkspaceId, Arc<ChannelAgentService>>>,
}

impl TurnExecutor {
    fn service(&self, workspace_id: &WorkspaceId) -> Arc<ChannelAgentService>;
    async fn spawn_reply(&self, channel_id: ChannelId, message: ChannelMessage);
}
```

`spawn_reply` resolves the workspace from the channel, gets the service,
`tokio::spawn`s `reply(...)`, and logs the report. Failures are logged and
posted as a system notice — never a panic and never a silently dropped message.

`AppState` gains `turn_executor: Option<Arc<TurnExecutor>>`, built in `main.rs`
only when the four variables are set.

*Verify*: unit test that two calls for one workspace return the same `Arc`
(tokio's `Arc::ptr_eq`), and that two workspaces get different ones.

### S0.4 — Wire it into post

`channels::post_message` calls `spawn_reply` after the message is persisted and
published. The response stays `201` with the message, matching the desktop's
"reply in the background" contract, so `RemoteClient` needs no change.

Two additions from §3, constraint 4:

- Move the single-participant capacity check (threshold → auto-compact;
  at-or-over → refuse with the existing message) from `commands.rs` into
  `ChannelAgentService::reply` behind a service-level helper, so both transports
  get it. The desktop keeps its pre-send call site only for the immediate
  composer error; the service becomes the authority.
- Pass `participant_capacity` from the executor by computing it the way
  `conversation_context_view` does, or accept the CLI's behaviour (empty → the
  percentage gate is skipped) for milestone one and record it as a known gap.
  Prefer computing it: the group skip path is user-visible.

*Verify*: an integration test with the mock provider — post a message that
mentions an agent, assert an agent message is stored and a `MessagePosted`
event was published for it.

### S0.5 — Tools profile

A deliberately narrow registry, chosen for a server that runs beside other
people's data:

- `CodeExecutionTool` with a `SubprocessSandbox` (timeouts and output limits
  already exist and are tested).
- `ReadFileTool` / `WriteFileTool` rooted at a per-workspace scratch directory,
  if at all — a server has no concept of "the user's project folder", so the
  desktop's `workspace_tools` bundle does not apply.

Document the choice in the ADR and in `docs/LIMITATIONS.md`, including that a
server turn cannot reach a local git project (worktrees stay desktop-only,
ADR-0031).

*Verify*: a server turn that requests `create_channel` (a desktop-only tool)
gets a clear "unknown tool" result rather than a crash.

### S0.6 — Live streaming (follow-up, may slip)

Track `turn_id → channel_id` in the socket scope so `TextDelta`, `ToolCall*`,
and `TurnDone` are admitted for a turn whose `TurnStarted` was admitted. This is
the only step that changes the socket contract, and it is independent of S1 —
IM adapters do not need streaming.

*Verify*: a socket subscribed to the workspace receives a delta sequence for a
server-run turn, in order, and no deltas for another workspace's turn.

## 5. Verification strategy

- **No API key needed.** `MockLlmProvider::echo()` makes every acceptance test
  in §4 runnable in CI, including the multi-tool path if the mock is extended
  with a scripted tool call.
- **Regression guarantee**: with the four variables unset, `main.rs` builds no
  executor, and `post_message` behaves exactly as today. Assert with the
  existing server tests unchanged.
- **Gates**: `cargo test --workspace`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo fmt --check`. No desktop gate is affected, since the
  only desktop change is moving the capacity check (S0.4) — run
  `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets --
  -D warnings` anyway if that lands in the same PR.
- **Manual smoke**: one Docker Compose run with a real key, one message, one
  approval.

## 6. Risks

| Risk | Mitigation |
| --- | --- |
| A shared executor leaks one workspace's turn into another | Services are keyed by workspace and every channel is resolved to its workspace before use; add a test with two workspaces. |
| Per-request service construction quietly breaks cancellation | S0.3's cache is the fix; the unit test in S0.3 pins it. |
| Long turns hold a connection | The turn runs in a spawned task; the request returns `201` immediately. |
| A server turn executes code in the shared deployment | Narrow tool registry (S0.5), existing approval gate, and the existing sandbox limits. |
| Auto-compaction divergence between transports | Move the check into the service (S0.4) rather than duplicating it. |

## 7. Sequence and parallel work

S0.1 → S0.2 → S0.3 → S0.4 → S0.5 can be one branch or five small PRs; S0.4
depends on S0.3, the rest are near-independent. S0.6 is separate.

**S1 (the adapter crate) can start in parallel with S0.3**, because it only
needs the `EventBus`, `ChannelMessage`, and `Store` — not a working executor.
Its trait takes the executor as an injected dependency, so the two branches do
not collide. S2 cannot start until S0.4 merges, because it needs a turn to
actually answer.

Suggested first PR: S0.1 + S0.2 + S0.3 together (config, provider, executor),
with the mock provider proving the cache and the spawn. Then S0.4 (wire-in,
the observable behavior change), then S0.5.
