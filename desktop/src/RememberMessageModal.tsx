import { useState } from "react";
import type { FormEvent } from "react";

import Modal from "./Modal";
import { addMemoryEntry } from "./api";
import { errorMessage } from "./errors";
import type { MemoryKind } from "./types";

interface RememberMessageModalProps {
  channelId: string;
  initialContent: string;
  onClose: () => void;
  onSaved: () => void;
}

const kinds: MemoryKind[] = [
  "fact",
  "preference",
  "decision",
  "summary",
  "note",
];

/**
 * Prefilled editor for remembering a user message.
 *
 * The entry is conversation-shared: it has no agent scope, so every
 * participant recalls it in later turns of this conversation.
 */
export default function RememberMessageModal({
  channelId,
  initialContent,
  onClose,
  onSaved,
}: RememberMessageModalProps) {
  const [content, setContent] = useState(initialContent);
  const [kind, setKind] = useState<MemoryKind>("fact");
  const [pinned, setPinned] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await addMemoryEntry(content, kind, { channelId, pinned });
      onSaved();
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title="Remember this message"
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="ghost-button"
            onClick={onClose}
            disabled={busy}
          >
            Cancel
          </button>
          <button
            type="submit"
            form="remember-message-form"
            className="primary-button"
            disabled={busy || !content.trim()}
          >
            {busy ? "Saving…" : "Save to memory"}
          </button>
        </>
      }
    >
      <form
        id="remember-message-form"
        className="dialog-form"
        onSubmit={(event) => void handleSubmit(event)}
      >
        <p className="dialog-copy">
          One durable fact, preference, or decision. The entry is shared with
          every participant of this conversation and is recalled in later
          turns.
        </p>
        <label className="field">
          <span>Memory</span>
          <textarea
            value={content}
            rows={4}
            disabled={busy}
            onChange={(event) => setContent(event.target.value)}
          />
        </label>
        <label className="field">
          <span>Kind</span>
          <select
            value={kind}
            disabled={busy}
            onChange={(event) => setKind(event.target.value as MemoryKind)}
          >
            {kinds.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        </label>
        <label className="context-toggle">
          <input
            type="checkbox"
            checked={pinned}
            disabled={busy}
            onChange={(event) => setPinned(event.target.checked)}
          />
          <span>Pin so it is recalled before scored entries</span>
        </label>
        {error ? (
          <p className="dialog-error" role="alert">
            {error}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}
