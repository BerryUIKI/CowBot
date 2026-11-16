import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useT } from "./i18n";

type TitleBarProps = {
  leading: ReactNode;
  title?: string;
  actions?: ReactNode;
};

export function TitleBar({ leading, title, actions }: TitleBarProps) {
  const t = useT();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let cancelled = false;
    const sync = () => {
      win.isMaximized().then((value) => {
        if (!cancelled) setMaximized(value);
      });
    };
    sync();
    const promise = win.onResized(sync);
    return () => {
      cancelled = true;
      promise.then((unlisten) => unlisten());
    };
  }, []);

  const win = getCurrentWindow();

  return (
    <div className="titlebar">
      <div className="titlebar-leading">{leading}</div>
      <div className="titlebar-center" data-tauri-drag-region>
        {title ? <span className="titlebar-title">{title}</span> : null}
      </div>
      <div className="titlebar-actions">{actions}</div>
      <div className="titlebar-controls">
        <button
          type="button"
          className="titlebar-button"
          title={t("titleBar.minimize")}
          aria-label={t("titleBar.minimize")}
          onClick={() => win.minimize()}
        >
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M1 6h10" stroke="currentColor" strokeWidth="1" fill="none" />
          </svg>
        </button>
        <button
          type="button"
          className="titlebar-button"
          title={maximized ? t("titleBar.restore") : t("titleBar.maximize")}
          aria-label={maximized ? t("titleBar.restore") : t("titleBar.maximize")}
          onClick={() => win.toggleMaximize()}
        >
          {maximized ? (
            <svg viewBox="0 0 12 12" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1">
              <path d="M3.5 1.5h7v7" />
              <rect x="1.5" y="3.5" width="7" height="7" />
            </svg>
          ) : (
            <svg viewBox="0 0 12 12" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1">
              <rect x="1.5" y="1.5" width="9" height="9" />
            </svg>
          )}
        </button>
        <button
          type="button"
          className="titlebar-button titlebar-close"
          title={t("titleBar.close")}
          aria-label={t("titleBar.close")}
          onClick={() => win.close()}
        >
          <svg viewBox="0 0 12 12" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1">
            <path d="M1.5 1.5l9 9M10.5 1.5l-9 9" />
          </svg>
        </button>
      </div>
    </div>
  );
}
