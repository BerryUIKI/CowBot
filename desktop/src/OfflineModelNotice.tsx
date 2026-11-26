import { useT } from "./i18n";

interface OfflineModelNoticeProps {
  onOpenModelSettings: () => void;
  onDismiss: () => void;
}

/**
 * Tells the user that no model connection supplies the open conversation.
 *
 * With no active provider the runtime falls back to the offline echo model
 * (ADR-0012), which replies by repeating the prompt it was given. That reads
 * as a strange answer rather than as a missing configuration, so the composer's
 * "Offline model" label is not enough on its own. This notice sits above the
 * composer where the reply appears, says what is happening in one sentence,
 * and links to the section that fixes it.
 *
 * It is a reminder, not a gate: the echo model keeps working, because the
 * offline fallback is what makes a credential-free trial run possible. The
 * caller owns whether it is showing, so a dismissal survives chat switches
 * until the user connects a model and then loses it again.
 */
export default function OfflineModelNotice({
  onOpenModelSettings,
  onDismiss,
}: OfflineModelNoticeProps) {
  const t = useT();
  return (
    <aside className="offline-model-notice" role="status">
      <div className="offline-model-notice-body">
        <span className="offline-model-notice-title">
          {t("offlineModelNotice.title")}
        </span>
        <span className="offline-model-notice-text">
          {t("offlineModelNotice.body")}
        </span>
      </div>
      <div className="offline-model-notice-actions">
        <button type="button" className="text-button" onClick={onOpenModelSettings}>
          {t("offlineModelNotice.openSettings")}
        </button>
        <button
          type="button"
          className="text-button"
          onClick={onDismiss}
          title={t("offlineModelNotice.dismissTitle")}
        >
          {t("offlineModelNotice.dismiss")}
        </button>
      </div>
    </aside>
  );
}
