import UpdateProgress from "./UpdateProgress";
import { useT } from "./i18n";
import type { Updater } from "./updater";

interface UpdateBannerProps {
  updater: Updater;
  tasksRunning: boolean;
}

/**
 * Sidebar update prompt. It sits above the account row and only appears after
 * a launch-time check finds a new version or an installation attempt reports
 * its state; manual check results live in Settings.
 */
export default function UpdateBanner({ updater, tasksRunning }: UpdateBannerProps) {
  const t = useT();
  const { state } = updater;
  const visible =
    !state.dismissed &&
    (state.phase === "available" ||
      state.phase === "downloading" ||
      state.phase === "ready" ||
      (state.phase === "error" && state.version !== null));

  if (!visible) return null;

  const title =
    state.phase === "ready"
      ? t("update.installed", { version: state.version ?? "" })
      : t("update.available", { version: state.version ?? "" });

  return (
    <div className="update-card" role="status">
      <div className="update-card-head">
        <span className="update-card-icon" aria-hidden="true">
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
          >
            <path d="M12 4v10" />
            <path d="m8 10 4 4 4-4" />
            <path d="M5 19h14" />
          </svg>
        </span>
        <div className="update-card-text">
          <span className="update-card-title">{title}</span>

          {state.phase === "available" ? (
            <span className="update-card-hint">
              {tasksRunning ? t("update.finishTasks") : t("update.downloadNow")}
            </span>
          ) : null}

          {state.phase === "downloading" ? (
            <UpdateProgress
              downloaded={state.downloaded}
              total={state.total}
              className="update-card-progress"
            />
          ) : null}

          {state.phase === "ready" ? (
            <span className="update-card-hint">{t("update.restartHint")}</span>
          ) : null}

          {state.phase === "error" && state.error ? (
            <span className="update-card-hint is-error">{state.error}</span>
          ) : null}
        </div>
      </div>

      <div className="update-card-actions">
        {state.phase === "available" ? (
          <button
            type="button"
            className="primary-button"
            onClick={() => void updater.install()}
            disabled={tasksRunning}
          >
            {t("update.downloadInstall")}
          </button>
        ) : null}

        {state.phase === "ready" ? (
          <button
            type="button"
            className="primary-button"
            onClick={() => void updater.restart()}
          >
            {t("update.restartNow")}
          </button>
        ) : null}

        {state.phase === "error" ? (
          <button
            type="button"
            className="primary-button"
            onClick={() => void updater.install()}
          >
            {t("update.tryAgain")}
          </button>
        ) : null}

        {state.phase !== "downloading" ? (
          <button type="button" className="ghost-button" onClick={updater.dismiss}>
            {t("update.later")}
          </button>
        ) : null}
      </div>
    </div>
  );
}
