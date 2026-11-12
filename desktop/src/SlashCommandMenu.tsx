import type { CommandInfo } from "./types";

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
  return (
    <div
      className="command-suggestions"
      id="command-suggestions"
      role="listbox"
      aria-label="Command suggestions"
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
          title={command.summary}
        >
          <span className="command-usage">{command.usage}</span>
          <span className="command-summary">{command.summary}</span>
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
