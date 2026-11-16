import { listCommands } from "./api";
import type { CommandInfo } from "./types";

/// The menu is open only while the whole draft is a single slash token:
/// `/` followed by zero or more name characters and nothing else. Once the
/// draft contains whitespace the remainder is arguments, and the menu closes.
const SLASH_DRAFT = /^\/([\p{L}\p{N}_-]*)$/u;

/// Rows the menu shows at once. The catalogue has more, so filtering matters.
export const MAX_COMMAND_ROWS = 8;

/// The query of a slash-only draft, or `null` when the draft is not one.
export function slashQuery(draft: string): string | null {
  const match = draft.match(SLASH_DRAFT);
  return match ? match[1] : null;
}

/// Whether the draft is a command draft that should be sent through the
/// command path rather than posted as a message.
export function isCommandDraft(draft: string): boolean {
  return draft.trimStart().startsWith("/");
}

/// The catalogue entry the draft's first token names, or `null` when the draft
/// is not a slash command or names nothing in the catalogue.
///
/// This is the synchronous counterpart of `parse_command`: Escape needs to know
/// whether the draft is a mistyped command (which may be sent as a message) or
/// a real one (which Enter should still run) before the next Enter spends an
/// IPC round trip.
export function findDraftCommand(
  draft: string,
  catalogue: CommandInfo[],
): CommandInfo | null {
  const match = draft.trimStart().match(/^\/([\p{L}\p{N}_-]*)/u);
  if (!match) return null;
  const name = match[1].toLowerCase();
  return (
    catalogue.find(
      (command) =>
        command.name.toLowerCase() === name ||
        command.aliases.some((alias) => alias.toLowerCase() === name),
    ) ?? null
  );
}

/// Whether the slash token is followed by whitespace and more text.
///
/// A line such as `/usr/bin/env python` is a path, not a mistyped command, so
/// the caller sends it as an ordinary message instead of showing the unknown
/// command hint (ADR-0038, "Unknown input is never silently sent").
export function hasArgumentsAfterCommandToken(line: string): boolean {
  const trimmed = line.trimStart();
  if (!trimmed.startsWith("/")) return false;
  const rest = trimmed.slice(1);
  const separator = rest.search(/\s/u);
  return separator !== -1 && rest.slice(separator).trim().length > 0;
}

/// The command and partial first argument of an argument draft.
export interface ArgumentDraft {
  command: CommandInfo;
  query: string;
  /// Index in the draft where the argument text begins.
  start: number;
}

/// A command followed by one argument token, with no second argument yet.
const ARGUMENT_DRAFT = /^\s*\/([\p{L}\p{N}_-]+)[ \t]+([^\s]*)$/u;

/// The command and first-argument query of a draft such as `/approval as`.
///
/// Returns `null` once a second argument starts or while a quoted argument is
/// being typed, because those are free text and not completion candidates.
export function argumentQuery(
  draft: string,
  catalogue: CommandInfo[],
): ArgumentDraft | null {
  const match = draft.match(ARGUMENT_DRAFT);
  if (!match) return null;
  const name = match[1].toLowerCase();
  const command = catalogue.find(
    (candidate) =>
      candidate.name.toLowerCase() === name ||
      candidate.aliases.some((alias) => alias.toLowerCase() === name),
  );
  if (!command || command.args.length === 0) return null;
  const query = match[2];
  if (query.startsWith('"')) return null;
  return {
    command,
    query,
    start: draft.length - query.length,
  };
}

/// Filter the catalogue by a case-insensitive prefix on the canonical name or
/// an alias, ordered as the catalogue is.
export function filterCommands(
  catalogue: CommandInfo[],
  query: string,
  limit = MAX_COMMAND_ROWS,
): CommandInfo[] {
  const needle = query.toLowerCase();
  if (!needle) return catalogue.slice(0, limit);
  return catalogue
    .filter(
      (command) =>
        command.name.toLowerCase().startsWith(needle) ||
        command.aliases.some((alias) => alias.toLowerCase().startsWith(needle)),
    )
    .slice(0, limit);
}

/// The first argument the command still needs, if any.
function requiredArgument(
  command: CommandInfo,
): CommandInfo["args"][number] | null {
  return command.args.find((arg) => arg.required) ?? null;
}

/// The draft to insert when a command is accepted.
///
/// A command with no required argument runs immediately, so this returns
/// `null`. A command with one inserts `/name ` and leaves the caret in the
/// composer for the argument.
export function acceptedDraft(command: CommandInfo): string | null {
  return requiredArgument(command) ? `/${command.name} ` : null;
}

let cataloguePromise: Promise<CommandInfo[]> | null = null;

/// Fetch the catalogue once per frontend session.
///
/// A failed fetch is not cached, so the next attempt can recover; callers
/// keep slash mode off until it succeeds and report the failure once.
export function loadCommands(): Promise<CommandInfo[]> {
  if (!cataloguePromise) {
    cataloguePromise = listCommands().catch((error: unknown) => {
      cataloguePromise = null;
      throw error;
    });
  }
  return cataloguePromise;
}
