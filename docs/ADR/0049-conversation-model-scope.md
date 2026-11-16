# ADR 0049: Conversation model scope

- Status: Accepted
- Date: 2026-11-16
- Discussion: Part of the model settings redesign
  ([ADR-0048](0048-default-model-and-connections.md)).
- Supersedes: None. Changes the meaning of the model pair in
  [ADR-0030](0030-conversation-sessions-and-context.md).

## Context

A conversation recorded the desktop's active provider and model when it was
created and then ran on that pair for the rest of its life. Nothing in the app
could change it: `update_conversation_settings` had no model fields, the
context popover showed the pair only inside its detailed breakdown (and showed
the raw provider id), and the composer's model button displayed and changed the
global default, which the conversation ignored.

The result was a visible contradiction. A model picked in the composer appeared
to be selected, but the conversation kept answering with its recorded one;
switching to the offline model silenced tasks and channels while conversations
still called a real endpoint; deleting a provider made the conversation fall
back to the new active provider but kept sending the old model name.

The conversation pair exists for a reason: the context meter, compaction
threshold, router history budget, and read-only previews count tokens with the
conversation model's tokenizer, and changing the model mid-conversation should
be a deliberate act rather than a side effect of a global switch.

## Decision

### 1. A conversation either follows the default or is pinned

`ConversationSettings` gains `model_follows_default`. Migration `0020` adds the
column with `DEFAULT 0`, so every conversation written before this record stays
pinned — exactly how it already behaved. `ConversationSettings::new` and
`group` set the flag, so conversations created from now on follow the default.

The stored provider and model are always present and are the pair the next turn
will use when the flag is false. When the flag is true they are a mirror of the
default.

### 2. Following mirrors the default instead of resolving it at read time

Every command that changes the default rewrites the pair of every following
conversation in the same call: `set_active_model`, `activate_model_provider`,
`save_model_provider` when it saves the active connection, and
`delete_model_provider`.

Resolving at read time would have left the service's context projection,
compaction splits, and preview token counts on the old snapshot, because those
read `settings.model` inside `cowbot-service`. Mirroring keeps every reader
correct without changing a single method signature, and the write is bounded by
the number of conversations.

### 3. The composer changes the model that answers where you are

In a conversation the model button shows the conversation model with `· pinned`
or `· default`, and its menu lists that provider's models plus
**Follow the default model** when pinned. In a channel, agent direct chat, or
task view the same button shows and changes the default. The context popover's
meta line names the provider (not its id) and the scope.

### 4. A removed provider is never left as a stale model name

If a pinned conversation's provider is gone, the runtime resolves the
conversation through the active default rather than sending the old model name
to a different endpoint. Deleting a connection also rewrites the conversations
that referenced it: pinned ones start following the default, and following ones
take the new snapshot.

### 5. The update path carries the pair together

`ConversationSettingsUpdate` gains `provider_id`, `model`, and
`model_follows_default`. The service rejects a blank model, treats an empty
provider id as "none recorded", and applies the flag. The server's PATCH body
and `RemoteClient` forward all three, so a team client can change the same
scope. The desktop's update command fills the default's pair when a caller asks
to follow without naming one, so the flag never exists without the pair it
describes.

## Consequences

- New conversations track a changed default; existing ones behave as before
  until someone pins or follows explicitly.
- A default change is a small fan-out write. It runs after the configuration is
  persisted, so a failure leaves the default changed and a following
  conversation stale until the next change — the same failure mode the old
  global model had, and it is reported to the caller.
- Workspace packages round-trip the flag. A package written before this record
  loads its conversations as pinned.
- The context meter's percentages and tokenizer stay aligned with the model the
  next turn will call.
- Group routing, memory recall, and task runs are unaffected: groups live
  inside one conversation and tasks always use the default.

## Alternatives considered

- **One global model for everything, no per-conversation pair.** Simplest to
  explain, but it makes every global switch rewrite the token accounting of
  every open conversation, and it removes the deliberate choice the current
  per-conversation model provides.
- **Keep the pair immutable and only surface it.** It would fix the invisible
  state but leave users unable to change a conversation's model without
  starting a new conversation.
- **Resolve the effective model at read time in every reader.** Correct in
  principle, but it touches the service context projection, compaction, both
  previews, and the router budget, and each would need the configuration the
  service does not have.
