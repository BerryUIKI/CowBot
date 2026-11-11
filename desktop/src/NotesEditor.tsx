import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { deleteMemoryNote, getMemoryNotes, saveMemoryNote } from "./api";
import { errorMessage } from "./errors";

interface NotesEditorProps {
  channelId?: string;
  label: string;
  hint?: string;
  onChanged?: () => void;
}

/**
 * Editor for the workspace note (`channelId` omitted) or a conversation note.
 *
 * Writes are redacted by the service. The editor reports a redaction so the
 * user knows the saved text may differ from what they typed.
 */
export default function NotesEditor({
  channelId,
  label,
  hint,
  onChanged,
}: NotesEditorProps) {
  const [content, setContent] = useState("");
  const [hasNote, setHasNote] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoaded(false);
    setError(null);
    setNotice(null);
    getMemoryNotes(channelId)
      .then((notes) => {
        if (cancelled) return;
        const note = channelId ? notes.conversation : notes.workspace;
        setContent(note?.content ?? "");
        setHasNote(note !== null);
        setLoaded(true);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(errorMessage(err));
        setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [channelId]);

  async function handleSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const saved = await saveMemoryNote(content, channelId);
      setContent(saved.note.content);
      setHasNote(true);
      setNotice(
        saved.redacted
          ? "Saved. Credential-shaped text was redacted."
          : "Saved.",
      );
      onChanged?.();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!channelId) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await deleteMemoryNote(channelId);
      setContent("");
      setHasNote(false);
      setNotice("Note deleted.");
      onChanged?.();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="notes-editor" onSubmit={(event) => void handleSave(event)}>
      <div className="notes-editor-head">
        <h4>{label}</h4>
        {hint ? <p className="settings-hint">{hint}</p> : null}
      </div>
      {loaded ? (
        <textarea
          value={content}
          rows={4}
          disabled={busy}
          placeholder={
            channelId
              ? "Notes shared with every participant of this conversation."
              : "Notes shared with every agent and conversation in this workspace."
          }
          onChange={(event) => setContent(event.target.value)}
        />
      ) : (
        <p className="settings-empty">Loading note…</p>
      )}
      <div className="notes-editor-actions">
        <button
          type="submit"
          className="primary-button"
          disabled={busy || !loaded || !content.trim()}
        >
          {busy ? "Saving…" : "Save note"}
        </button>
        {channelId && hasNote ? (
          <button
            type="button"
            className="ghost-button"
            disabled={busy}
            onClick={() => void handleDelete()}
          >
            Delete note
          </button>
        ) : null}
        {notice ? (
          <span className="settings-meta-inline" role="status">
            {notice}
          </span>
        ) : null}
      </div>
      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}
    </form>
  );
}
