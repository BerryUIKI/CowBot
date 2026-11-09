import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { approvalModes } from "./types";
import type { ApprovalLevel } from "./types";

interface ApprovalModeMenuProps {
  value: ApprovalLevel;
  onChange: (level: ApprovalLevel) => void;
  disabled?: boolean;
}

export default function ApprovalModeMenu({
  value,
  onChange,
  disabled = false,
}: ApprovalModeMenuProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const current = approvalModes.find((mode) => mode.value === value) ?? approvalModes[1];

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent | globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey as EventListener);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey as EventListener);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    menuRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitemradio"]')
      ?.focus();
  }, [open]);

  function handleMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitemradio"]',
      ) ?? [],
    );
    if (items.length === 0) return;
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);

    if (event.key === "ArrowDown") {
      event.preventDefault();
      items[(currentIndex + 1) % items.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      items[(currentIndex - 1 + items.length) % items.length]?.focus();
    } else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
    } else if (/^[1-5]$/.test(event.key)) {
      event.preventDefault();
      const mode = approvalModes[Number(event.key) - 1];
      if (mode) {
        onChange(mode.value);
        setOpen(false);
      }
    }
  }

  return (
    <div className="approval-mode" ref={rootRef}>
      <button
        type="button"
        className="approval-mode-button"
        aria-haspopup="menu"
        aria-expanded={open}
        title={`Approval mode: ${current.label}`}
        onClick={() => setOpen((current) => !current)}
        disabled={disabled}
      >
        <span className="approval-mode-dot" aria-hidden="true" />
        {current.label}
      </button>

      {open ? (
        <div
          ref={menuRef}
          className="approval-mode-popover"
          role="menu"
          aria-label="Approval mode"
          onKeyDown={handleMenuKeyDown}
        >
          <div className="approval-mode-header">Mode</div>
          {approvalModes.map((mode, index) => (
            <button
              key={mode.value}
              type="button"
              role="menuitemradio"
              aria-checked={mode.value === value}
              className={`approval-mode-option${
                mode.value === value ? " is-active" : ""
              }`}
              onClick={() => {
                onChange(mode.value);
                setOpen(false);
              }}
            >
              <span className="approval-mode-check" aria-hidden="true">
                {mode.value === value ? "✓" : ""}
              </span>
              <span className="approval-mode-copy">
                <span className="approval-mode-label">
                  {mode.label}
                  <span className="approval-mode-shortcut">{index + 1}</span>
                </span>
                <span className="approval-mode-description">
                  {mode.description}
                </span>
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
