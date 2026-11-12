import RichText from "./RichText";
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
  return (
    <aside className="side-answer" aria-label="Side answer">
      <header className="side-answer-header">
        <span className="side-answer-title">
          Side answer from {answer.agent_name}
        </span>
        <span className="side-answer-meta" title={answer.call_id}>
          {answer.model}
        </span>
        <button
          type="button"
          className="text-button"
          onClick={onAskAgain}
          title="Ask another side question"
        >
          Ask again
        </button>
        <button
          type="button"
          className="text-button"
          onClick={onDismiss}
          aria-label="Dismiss side answer"
        >
          Dismiss
        </button>
      </header>
      <div className="side-answer-body">
        <RichText content={answer.content} />
      </div>
      <p className="side-answer-footnote">
        This exchange is not in the conversation; the agent will not remember it.
      </p>
    </aside>
  );
}
