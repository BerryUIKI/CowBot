# ADR 0042: Group routing modes, participant mute, and reply cleanup

- Status: Accepted
- Date: 2026-11-13
- Discussion: [#183](https://github.com/BerryUIKI/CowBot/issues/183)
- Supersedes: None. Extends the group routing of [ADR-0032](0032-group-conversations-and-routing.md).

## Context

Group conversations had one way to answer an unaddressed message: the stored
default responder, or the model router when it was enabled. Three gaps followed.

- **A group could not take turns for free.** Without a router call, the same
  participant answered every message. Letting a group rotate speakers is a
  scheduling problem, not a routing problem, and paying a model call to solve
  it makes a cheap turn expensive.
- **There was no temporary mute.** Removing a participant changes the roster
  and the history the branch was built from. A user who wants an agent quiet
  for one conversation had no way to say so.
- **Model output drifted.** Told to follow the `[Name]` convention, a model
  sometimes answers `Planner: ...` anyway, or keeps writing after the point
  where another participant would have spoken. The system prompt asked it not
  to; nothing checked.

## Decision

### 1. A routing mode decides the unaddressed case

`ConversationSettings` gains `routing_mode: RoutingMode`, one of `mention`
(the default, preserving current behaviour), `rotation`, or `default`.

An explicit `@mention` always wins, whatever the mode. The mode only decides
what happens when nobody is named:

- **`mention`** — the model router decides when `router_enabled` is set,
  falling back to the default responder. This is the pre-existing behaviour and
  stays the default for every existing conversation.
- **`rotation`** — the participant after the last speaker in roster order takes
  the turn. No router call is made, and `router_enabled` is ignored.
- **`default`** — only the stored default responder answers; the router never
  runs, so a conversation can turn it off without losing the setting.

`RoutingMode::uses_router` is the single place that decides whether a mode may
call the router, so a future mode cannot accidentally buy a routing call.

### 2. Rotation is deterministic and roster-ordered

`next_rotation` walks forward from the last speaker's roster slot, wraps, and
skips muted participants. It takes an id extractor rather than a concrete
participant type, so the pure function is tested in `cowbot-core` and reused by
the service over `AgentProfile`.

The last speaker is the author of the newest assistant entry across the
participants' branches, read once per turn. Direct branches diverge after a
while, so there is no single "last entry"; the newest assistant entry over the
roster is the best answer to "who spoke last". An unknown or absent last
speaker — a fresh conversation, or a participant since removed — starts the
rotation at the first unmuted participant, so a roster change never fails a
turn.

A selection is recorded as `RouteSource::Rotation`, and the Answered-by line
says "by rotation", so the same evidence follows every other route source.

### 3. Mute keeps the roster

`ConversationSettings` gains `muted_participants: Vec<AgentId>`. A muted
participant stays in the roster, keeps receiving every message in its context
branch, and is simply never selected. `update_settings` refuses an id outside
the roster and de-duplicates the list, so a stale or duplicated id is a
reported error rather than silent state.

Muting everyone leaves the turn `Silent` with a recorded reason instead of
failing the send. That matches the existing all-unavailable behaviour, and a
user who muted the whole group did ask for silence.

### 4. Reply cleanup is a pure function, on by default

`clean_group_reply(content, speaker, others)` strips a leading `Name:` or
`[Name]:` label for the speaker and truncates the reply at the first line
another participant appears to have written. `reply_cleanup` (default `true`)
turns it off per conversation.

Two rules keep it safe:

- **It never deletes an answer.** If cleanup would leave the reply empty, the
  original text is stored. A `Reviewer: ...` line from a different speaker as
  the whole reply therefore survives, because the alternative is silence.
- **It only runs on a successful group reply.** A failure posts its notice, and
  a stopped reply keeps its `_[Stopped]_` marker, neither of which is
  reformatted.

`TurnSink::Conversation` carries an optional `ReplyCleanup` so the runner
cleans the body it stores while the terminal `TurnDone` event reports the raw
text; the live bubble and the stored message never disagree about what the
model produced.

### 5. The settings persist through one migration

Migration `0018_group_routing_policy.sql` adds `routing_mode`,
`muted_participants`, and `reply_cleanup` to `conversation_settings` with
defaults that preserve behaviour: `'mention'`, `'[]'`, and `1`. Existing
conversations keep answering exactly as they did.

## Alternatives considered

**Pooled or random selection, as CatCup does.** Rejected for the same reason the
router is deterministic: "why did this agent answer" has to be explainable
after the fact. Randomness makes an audit trail a guess.

**Call the model router for every unaddressed message.** Works, and it is what
`mention` mode already does when enabled, but it adds cost and latency to a
turn-taking problem that has a deterministic answer.

**Remove a participant instead of muting.** Destructive: it changes the roster
and blocks the participant from the context it was part of. Muting is the
reversible version.

**Do not clean replies.** Keeps the model's raw output, and produces duplicate
speaker labels and cross-talk in the transcript. The opt-out keeps raw output
available for a user who wants it.

**Store the routing mode as a boolean pair** (`router_enabled` plus
`rotation_enabled`). Two flags can express states nobody wants, such as both
on. One enum has one meaning.

## Consequences

- A rotation turn costs one participant's reply and no router call, which makes
  a group usable on a small or local model without a routing budget.
- The last-speaker lookup reads every participant's branch each rotation turn.
  That is a cheap local query at current roster sizes (two to eight); a
  dedicated column would be the next step if a roster ever needed to be large.
- Reply cleanup is a heuristic over labels. It can cut a reply that legitimately
  starts a line with another participant's name followed by a colon, which is
  why the option exists and why it never returns an empty reply.
- `RouterSource::Rotation` joins the persisted `RouteSource` values, so an older
  client reading a new turn falls through its label match rather than failing to
  parse.
