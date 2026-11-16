import UpdateProgress from "./UpdateProgress";
import { useT } from "./i18n";
import type { Updater } from "./updater";

interface UpdateBannerProps {
  updater: Updater;
  tasksRunning: boolean;
}

/**
 * Launch-time update prompt. It only appears after a successful check or an
 * installation attempt; manual check results live in Settings.
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

  return (
    <div className="update-banner" role="status">
      <div className="update-banner-body">
        <span className="update-banner-title">
          {state.phase === "ready"
            ? t("update.installed", { version: state.version ?? "" })
            : t("update.available", { version: state.version ?? "" })}
        </span>

        {state.phase === "available" ? (
          <span className="update-banner-hint">
            {tasksRunning
              ? t("update.finishTasks")
              : t("update.downloadNow")}
          </span>
        ) : null}

        {state.phase === "downloading" ? (
          <UpdateProgress downloaded={state.downloaded} total={state.total} />
        ) : null}

        {state.phase === "ready" ? (
          <span className="update-banner-hint">{t("update.restartHint")}</span>
        ) : null}

        {state.phase === "error" && state.error ? (
          <span className="update-banner-hint is-error">{state.error}</span>
        ) : null}
      </div>

      <div className="update-banner-actions">
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
