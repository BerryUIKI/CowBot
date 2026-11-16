import { useT } from "./i18n";

interface UpdateProgressProps {
  downloaded: number;
  total: number | null;
  className?: string;
}

export default function UpdateProgress({
  downloaded,
  total,
  className,
}: UpdateProgressProps) {
  const t = useT();
  const percent =
    total && total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;

  return (
    <div
      className={`update-progress${className ? ` ${className}` : ""}`}
      aria-label={t("update.progressAria")}
    >
      {percent !== null ? (
        <div className="update-progress-track">
          <div className="update-progress-fill" style={{ width: `${percent}%` }} />
        </div>
      ) : null}
      <span className="update-progress-label">
        {percent === null
          ? t("update.downloading")
          : t("update.downloadingPercent", { percent })}
      </span>
    </div>
  );
}
