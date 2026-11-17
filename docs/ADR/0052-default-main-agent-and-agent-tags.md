# ADR 0052: Default Main Agent and Agent Tags

- Status: Accepted
- Date: 2026-11-17
- Discussion: Requested with the naming and team-readiness brief for the main agent.
- Supersedes: None. Extends ADR-0028 (Main Assistant) and ADR-0050
  (interface localisation).

## Context

A new desktop workspace started empty: the user had to open the onboarding
team step or Settings → Team and add the Main Assistant template before any
conversation existed. That is an unnecessary first-run decision, and it made a
skipped onboarding leave a workspace with nothing to talk to.

The built-in assistant was also displayed as `main`, an identifier-shaped
default rather than a product identity. Renaming it was already supported, but
the default read like an internal name.

Team mode will put human teammates and agents in the same conversations. A
display name alone does not say which author is an agent, and a tag is a
low-cost way to make authorship explicit without changing stored names or
mentions.

## Decision

### 1. An empty workspace gets a main agent named CowBot

The built-in template's agent is named **CowBot**. Its id stays random: an agent
id is globally unique across every workspace in a database, so a fixed id such
as `main` would collide as soon as a second workspace shares the file, which
team mode does. The template keeps no id field. The desktop calls
`LocalClient::ensure_main_agent` after opening a workspace. The method
instantiates the built-in template only when the workspace has no agents at all
and then provisions the agent's direct conversation. A workspace with any agent
is left untouched: the default makes a new workspace usable, it does not
resurrect an assistant the user replaced with their own team.

Default-responder fallback and the desktop's responder preselection accept an
agent named **CowBot** and keep the legacy name `main` for workspaces created
before this change. Renaming still goes through
`AgentService::update_agent`, so the direct conversation title follows and
mentions use the new name. A renamed assistant is no longer matched by name;
group routing then falls back to the first participant until the user sets a
default responder explicitly.

### 2. The first conversation asks for a name

While the main agent's direct conversation is empty and its name is still the
default **CowBot**, the chat shows an inline naming card. It is pre-filled with
the current name and offers **Save name** and **Keep CowBot**. Saving calls the
ordinary agent update, so validation, uniqueness, events, and the conversation
rename all behave exactly as they do in Settings.

The prompt is deliberately non-modal and repeats nothing for an agent whose name
changed. Keeping the default records a device-local dismissal keyed by
workspace and agent id, so a second workspace still offers the prompt once.
The card is only rendered for an empty thread, so sending a first message ends
the invitation naturally.

### 3. Agent names can carry a `bot` tag

`AppConfig` gains `show_agent_bot_tags`, default `false`, edited under
**Settings → General**. The frontend caches it in localStorage beside the
locale and reconciles it with the configuration file.

When enabled, a small tag follows agent names in the sidebar's agent contacts,
the direct-chat title, group participant chips, message senders, inline
approvals, typing and streaming rows, and the agent cards in Settings → Team.
The tag text is a catalogue string: `bot` where the word is used as-is, and the
local short form elsewhere (for example 机器人, ボット, 봇, बॉट). The tag is
presentation only: it never enters the agent's stored name, a mention, a task
assignee, a conversation title, or a prompt.

## Alternatives considered

- **Create the main agent in every `LocalClient::open`.** Removes a desktop-only
  rule, but changes the CLI, server tests, and embedded clients, and would
  recreate an agent every time a workspace is opened empty on any surface.
- **A fixed agent id for the main assistant.** Would survive renames, but agent
  ids are globally unique per database and team mode shares one database across
  workspaces, so a literal `main` id collides on the second workspace. A
  workspace-scoped role column is the durable fix if identity is needed later.
- **Store a `name_confirmed` flag on the agent profile.** Workspace-scoped and
  survives devices, but needs a schema migration and a new update path for one
  conversational nudge. Device-local dismissal is enough for a prompt, and the
  rename itself remains workspace data.
- **Rename existing `main` agents automatically.** Silently rewrites user data;
  existing workspaces keep their current name and can be renamed in Settings.
- **Append “bot” to stored agent names.** Breaks mentions, assignment, package
  round-trips, and every place that resolves an agent by name.
- **Show the tag by default.** The brief asked for it to be off by default, and
  a local single-user workspace does not need the distinction until teammates
  share the conversation.

## Consequences

- A fresh desktop workspace has a usable main agent without visiting a template
  list; onboarding's team step shows the template as already added.
- Existing workspaces are not renamed or migrated. A workspace whose agents were
  all deleted receives CowBot on its next desktop launch.
- The server and CLI do not auto-provision the agent; their bootstrap keeps its
  current behaviour until the desktop's startup rule is shared.
- The default-responder fallback recognises the default name `CowBot` and the
  legacy name `main`; after a rename it uses the first participant until the
  user picks a responder.
- The bot tag is device-local and translated; switching it off restores the
  previous presentation without a restart.
