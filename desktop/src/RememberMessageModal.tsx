import { useState } from "react";
import type { FormEvent } from "react";

import Modal from "./Modal";
import { addMemoryEntry } from "./api";
import { errorMessage } from "./errors";
import { useT } from "./i18n";
import type { TranslationKey } from "./i18n/catalog";
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

const kindKeys: Record<MemoryKind, TranslationKey> = {
  fact: "memory.kind.fact",
  preference: "memory.kind.preference",
  decision: "memory.kind.decision",
  summary: "memory.kind.summary",
  note: "memory.kind.note",
};

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
  const t = useT();
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
      title={t("remember.title")}
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="ghost-button"
            onClick={onClose}
            disabled={busy}
          >
            {t("remember.cancel")}
          </button>
          <button
            type="submit"
            form="remember-message-form"
            className="primary-button"
            disabled={busy || !content.trim()}
          >
            {busy ? t("remember.saving") : t("remember.save")}
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
          {t("remember.copy")}
        </p>
        <label className="field">
          <span>{t("remember.memoryLabel")}</span>
          <textarea
            value={content}
            rows={4}
            disabled={busy}
            onChange={(event) => setContent(event.target.value)}
          />
        </label>
        <label className="field">
          <span>{t("remember.kind")}</span>
          <select
            value={kind}
            disabled={busy}
            onChange={(event) => setKind(event.target.value as MemoryKind)}
          >
            {kinds.map((option) => (
              <option key={option} value={option}>
                {t(kindKeys[option])}
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
          <span>{t("remember.pin")}</span>
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
