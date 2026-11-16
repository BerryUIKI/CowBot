import type { ArgumentSuggestion } from "./commandActions";

interface CommandArgumentMenuProps {
  suggestions: ArgumentSuggestion[];
  activeIndex: number;
  onAccept: (suggestion: ArgumentSuggestion) => void;
  onHover: (index: number) => void;
}

/// The composer's command-argument listbox.
///
/// Shares the command menu's markup and styles. Rows act on `mousedown` with
/// the default prevented, so choosing one never moves focus out of the
/// textarea.
export default function CommandArgumentMenu({
  suggestions,
  activeIndex,
  onAccept,
  onHover,
}: CommandArgumentMenuProps) {
  return (
    <div
      className="command-suggestions"
      id="command-argument-suggestions"
      role="listbox"
      aria-label="Command argument suggestions"
    >
      {suggestions.map((suggestion, index) => (
        <button
          key={`${suggestion.value}:${suggestion.hint ?? ""}`}
          type="button"
          role="option"
          id={`command-argument-option-${index}`}
          aria-selected={index === activeIndex}
          className={`command-option${index === activeIndex ? " is-active" : ""}`}
          onMouseDown={(event) => {
            event.preventDefault();
            onAccept(suggestion);
          }}
          onMouseEnter={() => onHover(index)}
          title={suggestion.hint}
        >
          <span className="command-usage">{suggestion.label}</span>
          {suggestion.hint ? (
            <span className="command-summary">{suggestion.hint}</span>
          ) : null}
        </button>
      ))}
    </div>
  );
}
