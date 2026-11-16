# ADR 0048: A default model and the connections that supply one

- Status: Accepted
- Date: 2026-11-16
- Discussion: The model settings redesign after 0.1.9.
- Supersedes: None. Refines [ADR-0021](0021-vendor-first-model-providers.md) and
  [ADR-0029](0029-token-usage-accounting.md); conversation scope is
  [ADR-0049](0049-conversation-model-scope.md).

## Context

The Models page mixed three jobs into one surface. A summary card repeated the
active provider that also appeared in the list, both carrying an **Active**
badge, and the summary card embedded a global action ("Use offline mock"). The
form edited the endpoint, the model library, and the active model at once, so
saving a connection could silently change which model ran.

Two actions contradicted what the page said. Selecting a model on a provider
that was not active reported "Active model set to X" while
`set_active_model` only rewrote that provider's model — the run model did not
change. Per-model context windows were reachable only for the model currently
selected in the draft, because the metadata editor wrote to `draft.model`.

## Decision

### 1. One selector answers "which model runs"

The page opens with a **Default model** selector listing every model on every
connection, grouped by connection, plus **Offline (no model)**. Selecting a
model writes the provider and its model in one command
(`set_active_model` now also sets `active_provider_id`), so the action and the
reported state cannot disagree. **Test** sends one request to that pair.

### 2. Connections own configuration; saving one never changes the default

A connection card holds the endpoint, the key flag, the model list, and each
model's limits. **Edit connection** changes the name, base URL, API key, and
(under **Advanced**) the protocol. It carries the existing model list and
metadata through unchanged, so saving a connection cannot alter what runs.
Because creating a connection needs at least one model, the create form keeps a
single **First model** field; after that, models are managed from the card.

### 3. Model rows are the unit of choice and configuration

Every model in a connection card is a row: choosing it makes it the default,
**Limits** expands its context window and output limit in place, and the
default row is marked rather than named "active". A connection that is not the
default shows no checkmark on its models, because its models do not run.

**Manage models** adds, fetches, and removes the connection's known models and
seeds the windows the endpoint reports. Saving it rewrites only the model list
and metadata; the endpoint, key, and default stay as they are.

### 4. Deleting a connection says what happens to conversations

Deleting the active connection falls back to the offline model. Conversations
that referenced the deleted connection move onto the current default, and
following conversations take the new snapshot (ADR-0049). The confirmation
names that consequence before the delete runs.

### 5. Wording and placement

**Offline (no model)** replaces "offline mock model"; the configuration file
path moves under **Advanced**; the section is titled **Models** and explains
the two layers before it shows either.

## Consequences

- `config.json` keeps its shape. The active pair is still
  `active_provider_id` plus that provider's `model`.
- Selecting any model from any connection is one action, so switching providers
  no longer requires finding the card and pressing a separate **Use**.
- A per-model window is editable without first making that model the draft's
  model, which the old form required.
- The desktop re-writes following conversations whenever the default changes;
  the cost is bounded by the number of conversations (ADR-0049).
- The composer keeps a short menu: the models of the provider that answers
  there, plus **Manage models…**. Cross-provider choice lives in Settings.

## Alternatives considered

- **Split Models and Connections into two settings pages.** The clearest
  separation, but it adds a navigation step to the common task of picking a
  model, and the connection a model comes from matters at the moment of choice.
- **Keep the layout and only remove the duplicate card and the misleading
  actions.** Half a day of work, but the form would still conflate connection,
  library, and default, and editing a connection would still be able to change
  the run model.
