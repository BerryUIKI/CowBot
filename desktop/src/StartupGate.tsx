import { useEffect, useState, type ReactNode } from "react";

import { getStartupStatus, quitApp, resolveStartup } from "./api";
import wordmark from "./assets/cowbot-wordmark-dark.svg";
import { errorMessage } from "./errors";
import { useT } from "./i18n";
import { TitleBar } from "./TitleBar";
import type { StartupAction, StartupStatus } from "./types";

/**
 * Holds the app back until the workspace database can be opened.
 *
 * Startup asks for confirmation before a one-way schema upgrade, and offers a
 * compatible backup or an empty workspace when the database belongs to a newer
 * CowBot. The backend only opens the database once this screen is satisfied.
 */
export default function StartupGate({ children }: { children: ReactNode }) {
  const t = useT();
  const [status, setStatus] = useState<StartupStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [selectedBackup, setSelectedBackup] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    getStartupStatus()
      .then((next) => {
        if (cancelled) return;
        setStatus(next);
        setSelectedBackup(
          next.state === "newer" ? (next.backups[0] ?? null) : null,
        );
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      });

    return () => {
      cancelled = true;
    };
  }, []);

  async function apply(action: StartupAction) {
    setBusy(true);
    setError(null);
    try {
      const next = await resolveStartup(action);
      setStatus(next);
      setSelectedBackup(
        next.state === "newer" ? (next.backups[0] ?? null) : null,
      );
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  if (status?.state === "ready") return <>{children}</>;

  const backups =
    status && (status.state === "newer" || status.state === "failed")
      ? status.backups
      : [];
  const replaced =
    status && (status.state === "newer" || status.state === "failed")
      ? status.replaced
      : null;

  return (
    <div className="startup">
      <TitleBar
        leading={<img className="wordmark" src={wordmark} alt="CowBot" />}
      />
      <main className="startup-main">
        <div className="startup-panel">
          {status === null ? (
            <p className="startup-loading" role="status">
              {error ?? t("startup.loading")}
            </p>
          ) : (
            <>
              <p className="onboarding-kicker">{t("startup.kicker")}</p>
              <h1 className="onboarding-title">
                {status.state === "upgrade"
                  ? t("startup.upgradeTitle")
                  : status.state === "newer"
                    ? t("startup.newerTitle")
                    : t("startup.failedTitle")}
              </h1>
              <p className="onboarding-copy">
                {status.state === "upgrade"
                  ? `${t("startup.upgradeCopy", {
                      current: status.current,
                      target: status.target,
                    })} ${t("startup.upgradeOneWay")}`
                  : status.state === "newer"
                    ? t("startup.newerCopy", {
                        current: status.current,
                        target: status.target,
                      })
                    : status.message}
              </p>

              {status.state === "upgrade" ? (
                <>
                  <p className="startup-path-label">
                    {t("startup.backupHint")}
                  </p>
                  <code className="startup-path">{status.backup}</code>
                </>
              ) : null}

              {backups.length > 0 ? (
                <div className="startup-backups">
                  <p className="startup-path-label">
                    {t("startup.backupsTitle")}
                  </p>
                  <ul
                    className="startup-backup-list"
                    role="radiogroup"
                    aria-label={t("startup.backupsTitle")}
                  >
                    {backups.map((backup) => (
                      <li key={backup}>
                        <button
                          type="button"
                          role="radio"
                          aria-checked={selectedBackup === backup}
                          className={`startup-backup${
                            selectedBackup === backup ? " is-selected" : ""
                          }`}
                          onClick={() => setSelectedBackup(backup)}
                          disabled={busy}
                        >
                          <span
                            className="startup-backup-mark"
                            aria-hidden="true"
                          />
                          <span className="startup-backup-path">{backup}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {replaced ? (
                <>
                  <p className="startup-path-label">
                    {t("startup.freshHint")}
                  </p>
                  <code className="startup-path">{replaced}</code>
                </>
              ) : null}

              <div className="onboarding-actions">
                {status.state === "upgrade" ? (
                  <>
                    <button
                      type="button"
                      className="primary-button"
                      disabled={busy}
                      onClick={() => void apply({ kind: "upgrade" })}
                    >
                      {t("startup.upgradeAction")}
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      disabled={busy}
                      onClick={() => void apply({ kind: "upgradeWithBackup" })}
                    >
                      {t("startup.upgradeBackupAction")}
                    </button>
                  </>
                ) : null}
                {backups.length > 0 ? (
                  <button
                    type="button"
                    className="primary-button"
                    disabled={busy || selectedBackup === null}
                    onClick={() => {
                      if (selectedBackup) {
                        void apply({ kind: "restore", backup: selectedBackup });
                      }
                    }}
                  >
                    {t("startup.restoreAction")}
                  </button>
                ) : null}
                {status.state !== "upgrade" ? (
                  <button
                    type="button"
                    className={
                      backups.length > 0 ? "ghost-button" : "primary-button"
                    }
                    disabled={busy}
                    onClick={() => void apply({ kind: "fresh" })}
                  >
                    {t("startup.freshAction")}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="ghost-button"
                  disabled={busy}
                  onClick={() => void quitApp()}
                >
                  {t("startup.quit")}
                </button>
              </div>
            </>
          )}

          {error ? (
            <p className="settings-status is-error" role="alert">
              {error}
            </p>
          ) : null}
        </div>
      </main>
    </div>
  );
}
