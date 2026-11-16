import { useEffect, useRef } from "react";
import type { ReactNode } from "react";

import { useT } from "./i18n";

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  className?: string;
}

export default function Modal({
  title,
  onClose,
  children,
  footer,
  className,
}: ModalProps) {
  const t = useT();
  const panelRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [onClose]);

  useEffect(() => {
    const field = panelRef.current?.querySelector<HTMLElement>(
      "input, textarea, select",
    );
    (field ?? panelRef.current?.querySelector<HTMLElement>("button"))?.focus();
  }, []);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className={`modal${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        ref={panelRef}
      >
        <header className="modal-head">
          <h2 className="modal-title">{title}</h2>
          <button
            type="button"
            className="icon-button modal-close"
            aria-label={t("modal.close")}
            onClick={onClose}
          >
            ×
          </button>
        </header>
        <div className="modal-body">{children}</div>
        {footer ? <footer className="modal-foot">{footer}</footer> : null}
      </div>
    </div>
  );
}
