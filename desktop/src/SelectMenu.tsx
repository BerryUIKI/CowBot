import { Fragment, useEffect, useRef, useState, type KeyboardEvent } from "react";

export interface SelectOption {
  value: string;
  label: string;
  /** Optional heading inserted above the first option of a group. */
  group?: string;
}

interface SelectMenuProps {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
}

/**
 * A themed dropdown that replaces the native select.
 *
 * The native popup is drawn by the OS and ignores the app's dark palette, so a
 * listbox built from buttons keeps the menu inside the interface. The trigger
 * keeps focus and handles the arrow keys, Enter, Space, and Escape.
 */
export default function SelectMenu({
  value,
  options,
  onChange,
  ariaLabel,
  disabled = false,
  className,
}: SelectMenuProps) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);

  const selected = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    return () => document.removeEventListener("mousedown", onPointer);
  }, [open]);

  function openMenu() {
    const index = options.findIndex((option) => option.value === value);
    setActive(index >= 0 ? index : 0);
    setOpen(true);
  }

  function choose(option: SelectOption) {
    onChange(option.value);
    setOpen(false);
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (!open) {
      if (
        event.key === "Enter" ||
        event.key === " " ||
        event.key === "ArrowDown"
      ) {
        event.preventDefault();
        openMenu();
      }
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((index) => Math.min(index + 1, options.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((index) => Math.max(index - 1, 0));
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const option = options[active];
      if (option) choose(option);
    }
  }

  let lastGroup: string | undefined;

  return (
    <div
      className={`select-menu${className ? ` ${className}` : ""}`}
      ref={rootRef}
    >
      <button
        type="button"
        className="select-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => (open ? setOpen(false) : openMenu())}
        onKeyDown={onKeyDown}
      >
        <span className="select-value">{selected?.label ?? ""}</span>
        <svg
          className="select-chevron"
          viewBox="0 0 12 12"
          aria-hidden="true"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
        >
          <path d="M2.5 4.5L6 8l3.5-3.5" />
        </svg>
      </button>

      {open ? (
        <ul className="select-popup" role="listbox" aria-label={ariaLabel}>
          {options.map((option, index) => {
            const heading = option.group !== lastGroup ? option.group : undefined;
            lastGroup = option.group;
            return (
              <Fragment key={option.value}>
                {heading ? (
                  <li className="select-group" role="presentation">
                    {heading}
                  </li>
                ) : null}
                <li
                  role="option"
                  aria-selected={option.value === value}
                  className={`select-option${
                    index === active ? " is-active" : ""
                  }${option.value === value ? " is-selected" : ""}`}
                  onMouseEnter={() => setActive(index)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => choose(option)}
                >
                  <span className="select-option-label">{option.label}</span>
                  {option.value === value ? (
                    <svg
                      className="select-check"
                      viewBox="0 0 12 12"
                      aria-hidden="true"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.5"
                    >
                      <path d="M2 6.5L4.8 9 10 3.5" />
                    </svg>
                  ) : null}
                </li>
              </Fragment>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
