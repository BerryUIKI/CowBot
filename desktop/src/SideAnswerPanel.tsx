import RichText from "./RichText";
import { useT } from "./i18n";
import type { SideAnswer } from "./types";

interface SideAnswerPanelProps {
  answer: SideAnswer;
  onDismiss: () => void;
  onAskAgain: () => void;
}

/**
 * The ephemeral answer to a `/btw` side question.
 *
 * It sits above the composer, is not part of the timeline, and disappears on
 * dismiss or chat switch. Nothing about it was persisted, so there is nothing
 * to reload: the panel exists only while the user is looking at it.
 */
export default function SideAnswerPanel({
  answer,
  onDismiss,
  onAskAgain,
}: SideAnswerPanelProps) {
  const t = useT();
  return (
    <aside className="side-answer" aria-label={t("sideAnswer.aria")}>
      <header className="side-answer-header">
        <span className="side-answer-title">
          {t("sideAnswer.title", { name: answer.agent_name })}
        </span>
        <span className="side-answer-meta" title={answer.call_id}>
          {answer.model}
        </span>
        <button
          type="button"
          className="text-button"
          onClick={onAskAgain}
          title={t("sideAnswer.askAgainTitle")}
        >
          {t("sideAnswer.askAgain")}
        </button>
        <button
          type="button"
          className="text-button"
          onClick={onDismiss}
          aria-label={t("sideAnswer.dismissAria")}
        >
          {t("sideAnswer.dismiss")}
        </button>
      </header>
      <div className="side-answer-body">
        <RichText content={answer.content} />
      </div>
      <p className="side-answer-footnote">
        {t("sideAnswer.footnote")}
      </p>
    </aside>
  );
}
