import { approvalLevelLabel, approvalModes } from "./types";
import type { ApprovalLevel } from "./types";
import type { SettingsSection } from "./SettingsView";

/// The state a command handler reads: the chat it applies to, the roster it
/// resolves `@participant` against, and the model catalogue it completes.
export interface CommandTargets {
  /// The selected chat, or `null` when nothing is selected.
  channel: {
    id: string;
    kind: "channel" | "direct" | "conversation";
    name: string;
    topic: string;
    agent_id: string | null;
  } | null;
  /// Participant display names paired with their agent ids, in roster order.
  participants: { agentId: string; name: string }[];
  /// Every provider with its known models, for `model` resolution.
  providers: { id: string; name: string; active: boolean; model: string; models: string[] }[];
  /// The composer's current approval mode, for `/approval` with no argument.
  approval: ApprovalLevel;
}

/// What the desktop should carry out for one command.
///
/// Handlers do not touch React state, so the mapping from a command to a
/// behaviour is testable without a renderer, and the desktop keeps one place
/// where state changes happen.
export type CommandAction =
  | { kind: "notice"; message: string }
  | { kind: "error"; message: string }
  | { kind: "help" }
  | { kind: "open-new-conversation"; participants: string[] }
  | { kind: "open-delete"; channelId: string }
  | { kind: "open-note" }
  | { kind: "open-settings"; section: SettingsSection }
  | { kind: "open-model-menu" }
  | { kind: "open-task"; description: string }
  | { kind: "set-approval"; mode: ApprovalLevel }
  | { kind: "compact"; channelId: string; agentId: string | null }
  | { kind: "clear"; channelId: string; agentId: string | null }
  | { kind: "rename"; channelId: string; name: string; topic: string }
  | { kind: "pin"; channelId: string; pinned: boolean }
  | { kind: "stop"; taskIds: string[] }
  | { kind: "remember"; content: string; agentId: string | null; channelId: string | null }
  | { kind: "save-note"; content: string; channelId: string };

/// Approval values and the aliases the issue documents for each.
const APPROVAL_ALIASES: Record<string, ApprovalLevel> = {
  safe_auto: "safe_auto",
  auto: "safe_auto",
  ask: "ask",
  manual: "ask",
  accept_edits: "accept_edits",
  "accept-edits": "accept_edits",
  plan_only: "plan_only",
  "plan-only": "plan_only",
  plan: "plan_only",
  trusted: "trusted",
  bypass: "trusted",
};

const SETTINGS_IDS: readonly SettingsSection[] = [
  "model",
  "team",
  "memory",
  "approvals",
  "audit",
  "projects",
  "workspace",
  "about",
];

/// Resolve an `@participant` argument against the roster, case-insensitively.
///
/// Returns `undefined` for a name that matches nothing and `null` when no
/// argument was given, so callers can tell "none asked for" from "bad name".
export function resolveParticipant(
  participants: CommandTargets["participants"],
  argument: string | undefined,
): { agentId: string; name: string } | null | undefined {
  const trimmed = (argument ?? "").trim();
  if (!trimmed) return null;
  const needle = trimmed.replace(/^@/, "").toLowerCase();
  return participants.find((participant) => participant.name.toLowerCase() === needle);
}

function rosterList(participants: CommandTargets["participants"]): string {
  return participants.map((participant) => `@${participant.name}`).join(", ");
}

/// Exact, case-insensitive model lookup across every provider.
export type ModelResolution =
  | { kind: "active"; model: string }
  | { kind: "other"; model: string; provider: string }
  | { kind: "unknown"; matches: string[] };

export function resolveModel(
  providers: CommandTargets["providers"],
  argument: string,
): ModelResolution {
  const needle = argument.trim().toLowerCase();
  const matches = providers.flatMap((provider) =>
    provider.models
      .filter((model) => model.toLowerCase() === needle)
      .map((model) => ({ model, provider: provider.name, active: provider.active })),
  );
  if (matches.length === 0) {
    const near = providers.flatMap((provider) =>
      provider.models.filter((model) => model.toLowerCase().includes(needle)),
    );
    return { kind: "unknown", matches: [...new Set(near)] };
  }
  const active = matches.find((match) => match.active);
  if (active) return { kind: "active", model: active.model };
  return { kind: "other", model: matches[0].model, provider: matches[0].provider };
}

/// `compact` and `clear` read a branch, so they only apply where the desktop
/// loads conversation context.
function needsConversation(targets: CommandTargets, name: string): CommandAction | null {
  if (targets.channel?.kind === "conversation") return null;
  return {
    kind: "notice",
    message: `/${name} only works in a conversation; this chat has no context branch.`,
  };
}

function needsChannel(targets: CommandTargets): CommandAction | null {
  if (targets.channel) return null;
  return { kind: "notice", message: "Select a chat first." };
}

/// Map one parsed invocation to the action the desktop should take.
///
/// `runningTaskIds` are the non-terminal tasks whose `channel_id` is the
/// selected chat; `/stop` acts on exactly those.
export function planCommand(
  invocation: { name: string; args: string[]; raw_args: string },
  targets: CommandTargets,
  runningTaskIds: string[],
): CommandAction {
  const channel = targets.channel;
  const first = invocation.args[0];

  switch (invocation.name) {
    case "help":
      return { kind: "help" };

    case "new": {
      if (invocation.args.length === 0) {
        return { kind: "open-new-conversation", participants: [] };
      }
      const resolved: string[] = [];
      const unknown: string[] = [];
      for (const argument of invocation.args) {
        const match = resolveParticipant(targets.participants, argument);
        if (match) {
          if (!resolved.includes(match.agentId)) resolved.push(match.agentId);
        } else {
          unknown.push(argument);
        }
      }
      if (unknown.length > 0) {
        return {
          kind: "notice",
          message: `Unknown ${unknown.join(", ")}. Participants: ${rosterList(targets.participants) || "none"}.`,
        };
      }
      return { kind: "open-new-conversation", participants: resolved };
    }

    case "rename": {
      const missing = needsChannel(targets);
      if (missing) return missing;
      return {
        kind: "rename",
        channelId: channel!.id,
        name: invocation.raw_args.trim(),
        topic: channel!.topic,
      };
    }

    case "pin":
    case "unpin": {
      const missing = needsChannel(targets);
      if (missing) return missing;
      return {
        kind: "pin",
        channelId: channel!.id,
        pinned: invocation.name === "pin",
      };
    }

    case "delete": {
      const missing = needsChannel(targets);
      if (missing) return missing;
      return { kind: "open-delete", channelId: channel!.id };
    }

    case "compact":
    case "clear": {
      const missing = needsChannel(targets);
      if (missing) return missing;
      const blocked = needsConversation(targets, invocation.name);
      if (blocked) return blocked;
      const participant = resolveParticipant(targets.participants, first);
      if (participant === undefined) {
        return {
          kind: "notice",
          message: `Unknown ${first}. Participants: ${rosterList(targets.participants) || "none"}.`,
        };
      }
      const agentId = participant?.agentId ?? null;
      return { kind: invocation.name, channelId: channel!.id, agentId };
    }

    case "note": {
      const missing = needsChannel(targets);
      if (missing) return missing;
      const blocked = needsConversation(targets, invocation.name);
      if (blocked) return blocked;
      const text = invocation.raw_args.trim();
      if (!text) return { kind: "open-note" };
      return { kind: "save-note", content: text, channelId: channel!.id };
    }

    case "remember": {
      const content = invocation.raw_args.trim();
      if (!content) return { kind: "error", message: "Usage: /remember <text>" };
      const missing = needsChannel(targets);
      if (missing) return missing;
      // A conversation scopes to its branch, an agent chat to the agent, and a
      // group channel has no narrower home than the workspace.
      if (channel!.kind === "conversation") {
        return { kind: "remember", content, channelId: channel!.id, agentId: null };
      }
      if (channel!.kind === "direct" && channel!.agent_id) {
        return { kind: "remember", content, agentId: channel!.agent_id, channelId: null };
      }
      return { kind: "remember", content, agentId: null, channelId: null };
    }

    case "task":
      return { kind: "open-task", description: invocation.raw_args.trim() };

    case "model": {
      const name = (first ?? "").trim();
      if (!name) return { kind: "open-model-menu" };
      const resolved = resolveModel(targets.providers, name);
      if (resolved.kind === "active") {
        return { kind: "notice", message: `The active model is already ${resolved.model}.` };
      }
      if (resolved.kind === "other") {
        return {
          kind: "notice",
          message: `${resolved.model} belongs to ${resolved.provider}; activate that provider in Settings → Model first.`,
        };
      }
      const hint =
        resolved.matches.length > 0
          ? `Did you mean ${resolved.matches.slice(0, 5).join(", ")}?`
          : "No known model contains that name.";
      return { kind: "notice", message: `Unknown model ${name}. ${hint}` };
    }

    case "approval": {
      const raw = (first ?? "").trim();
      if (!raw) {
        const valid = approvalModes.map((mode) => mode.value).join(", ");
        return {
          kind: "notice",
          message: `Approval mode is ${approvalLevelLabel(targets.approval)}. Use one of: ${valid}.`,
        };
      }
      const mode = APPROVAL_ALIASES[raw.toLowerCase()];
      if (!mode) {
        const valid = [...new Set(Object.keys(APPROVAL_ALIASES))].join(", ");
        return {
          kind: "notice",
          message: `Unknown approval mode ${raw}. Use one of: ${valid}.`,
        };
      }
      return { kind: "set-approval", mode };
    }

    case "stop":
      if (runningTaskIds.length === 0) {
        return { kind: "notice", message: "No running tasks in this chat." };
      }
      return { kind: "stop", taskIds: runningTaskIds };

    case "settings": {
      const raw = (first ?? "").trim().toLowerCase();
      if (!raw) return { kind: "open-settings", section: "model" };
      if (!SETTINGS_IDS.includes(raw as SettingsSection)) {
        return {
          kind: "notice",
          message: `Unknown section ${raw}. Use one of: ${SETTINGS_IDS.join(", ")}.`,
        };
      }
      return { kind: "open-settings", section: raw as SettingsSection };
    }

    default:
      return { kind: "error", message: `/${invocation.name} is not implemented.` };
  }
}
