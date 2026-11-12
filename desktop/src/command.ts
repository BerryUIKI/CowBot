import { listCommands, parseCommand } from "./api";
import type { CommandInfo, CommandParseResult } from "./types";

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
export function requiredArgument(
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

/// Read a draft through the shared Rust parser.
///
/// This is the only IPC round trip in the command path, and it happens once
/// per Enter press.
export function parseCommandDraft(input: string): Promise<CommandParseResult> {
  return parseCommand(input);
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

/// Forget the cached catalogue. Only tests and workspace switches need this.
export function resetCommandCatalogue(): void {
  cataloguePromise = null;
}
