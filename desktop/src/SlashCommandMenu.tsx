import type { CommandInfo } from "./types";
import { useT } from "./i18n";
import { t as translate } from "./i18n/catalog";

function commandSummary(name: string, fallback: string): string {
  switch (name) {
    case "approval":
      return translate("commands.catalogue.approval");
    case "btw":
      return translate("commands.catalogue.btw");
    case "clear":
      return translate("commands.catalogue.clear");
    case "compact":
      return translate("commands.catalogue.compact");
    case "delete":
      return translate("commands.catalogue.delete");
    case "help":
      return translate("commands.catalogue.help");
    case "model":
      return translate("commands.catalogue.model");
    case "new":
      return translate("commands.catalogue.new");
    case "note":
      return translate("commands.catalogue.note");
    case "pin":
      return translate("commands.catalogue.pin");
    case "remember":
      return translate("commands.catalogue.remember");
    case "rename":
      return translate("commands.catalogue.rename");
    case "settings":
      return translate("commands.catalogue.settings");
    case "stop":
      return translate("commands.catalogue.stop");
    case "task":
      return translate("commands.catalogue.task");
    case "unpin":
      return translate("commands.catalogue.unpin");
    default:
      return fallback;
  }
}

interface SlashCommandMenuProps {
  commands: CommandInfo[];
  activeIndex: number;
  onAccept: (command: CommandInfo) => void;
  onHover: (index: number) => void;
  /** Shown under the rows; the unknown-command hint uses it. */
  hint?: string | null;
}

/// The composer's command listbox.
///
/// Mirrors the mention popover's markup and mouse handling: rows are buttons
/// that act on `mousedown` with the default prevented, so choosing a row never
/// moves focus out of the textarea.
export default function SlashCommandMenu({
  commands,
  activeIndex,
  onAccept,
  onHover,
  hint = null,
}: SlashCommandMenuProps) {
  const t = useT();
  return (
    <div
      className="command-suggestions"
      id="command-suggestions"
      role="listbox"
      aria-label={t("commands.suggestionsAria")}
    >
      {commands.map((command, index) => (
        <button
          key={command.name}
          type="button"
          role="option"
          id={`command-option-${index}`}
          aria-selected={index === activeIndex}
          className={`command-option${index === activeIndex ? " is-active" : ""}`}
          onMouseDown={(event) => {
            event.preventDefault();
            onAccept(command);
          }}
          onMouseEnter={() => onHover(index)}
          title={commandSummary(command.name, command.summary)}
        >
          <span className="command-usage">{command.usage}</span>
          <span className="command-summary">
            {commandSummary(command.name, command.summary)}
          </span>
        </button>
      ))}
      {commands.length === 0 && hint ? (
        <p className="command-hint" role="status">
          {hint}
        </p>
      ) : null}
    </div>
  );
}
