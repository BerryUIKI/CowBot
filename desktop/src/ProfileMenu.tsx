import { useEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import { useT } from "./i18n";

type MenuIconName =
  | "settings"
  | "language"
  | "models"
  | "workspace"
  | "changelog"
  | "learn";

interface ProfileMenuProps {
  userName: string;
  workspaceName: string;
  onClose: () => void;
  onSettings: () => void;
  onLanguage: () => void;
  onModels: () => void;
  onWorkspace: () => void;
  onChangelog: () => void;
  onLearnMore: () => void;
}

function MenuIcon({ name }: { name: MenuIconName }) {
  switch (name) {
    case "settings":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      );
    case "language":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <circle cx="12" cy="12" r="9" />
          <path d="M3 12h18M12 3c2.5 2.5 3.8 5.6 3.8 9S14.5 18.5 12 21c-2.5-2.5-3.8-5.6-3.8-9S9.5 5.5 12 3z" />
        </svg>
      );
    case "models":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <path d="M4 6h16M4 12h16M4 18h16" />
          <circle cx="9" cy="6" r="2" fill="currentColor" stroke="none" />
          <circle cx="15" cy="12" r="2" fill="currentColor" stroke="none" />
          <circle cx="8" cy="18" r="2" fill="currentColor" stroke="none" />
        </svg>
      );
    case "workspace":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z" />
        </svg>
      );
    case "changelog":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <path d="M6 3h9l4 4v14H6z" />
          <path d="M9 11h7M9 15h7M9 7h3" />
        </svg>
      );
    case "learn":
      return (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 11v5M12 8h.01" />
        </svg>
      );
  }
}

/**
 * Bottom-left account menu, modeled on Claude's: the user and workspace at the
 * top, then navigation to settings, language, models, workspace, and the
 * project's changelog and documentation.
 */
export default function ProfileMenu({
  userName,
  workspaceName,
  onClose,
  onSettings,
  onLanguage,
  onModels,
  onWorkspace,
  onChangelog,
  onLearnMore,
}: ProfileMenuProps) {
  const t = useT();
  const menuRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const onPointer = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  useEffect(() => {
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  }, []);

  const items: { icon: MenuIconName; label: string; onSelect: () => void }[] = [
    { icon: "settings", label: t("profile.settings"), onSelect: onSettings },
    { icon: "language", label: t("profile.language"), onSelect: onLanguage },
    { icon: "models", label: t("profile.models"), onSelect: onModels },
    { icon: "workspace", label: t("profile.workspace"), onSelect: onWorkspace },
    { icon: "changelog", label: t("profile.changelog"), onSelect: onChangelog },
    { icon: "learn", label: t("profile.learnMore"), onSelect: onLearnMore },
  ];

  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const buttons = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ??
        [],
    );
    if (buttons.length === 0) return;
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      buttons[(current + 1) % buttons.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      buttons[(current - 1 + buttons.length) % buttons.length]?.focus();
    }
  }

  return (
    <div
      className="profile-menu"
      role="menu"
      aria-label={t("profile.menuAria")}
      ref={menuRef}
      onKeyDown={handleKeyDown}
    >
      <div className="profile-menu-head">
        <span className="profile-menu-name">{userName}</span>
        <span className="profile-menu-workspace">{workspaceName}</span>
      </div>
      <div className="profile-menu-separator" role="separator" />
      {items.slice(0, 4).map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className="profile-menu-item"
          onClick={() => {
            item.onSelect();
            onClose();
          }}
        >
          <span className="profile-menu-icon">
            <MenuIcon name={item.icon} />
          </span>
          <span className="profile-menu-label">{item.label}</span>
        </button>
      ))}
      <div className="profile-menu-separator" role="separator" />
      {items.slice(4).map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className="profile-menu-item"
          onClick={() => {
            item.onSelect();
            onClose();
          }}
        >
          <span className="profile-menu-icon">
            <MenuIcon name={item.icon} />
          </span>
          <span className="profile-menu-label">{item.label}</span>
        </button>
      ))}
    </div>
  );
}
