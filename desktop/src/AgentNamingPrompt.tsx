import { useState } from "react";
import type { FormEvent } from "react";

import { updateAgent } from "./api";
import { errorMessage } from "./errors";
import { useT } from "./i18n";
import type { AgentProfile } from "./types";

/**
 * First-conversation prompt that lets the user name the built-in assistant.
 *
 * The default agent is created as CowBot. While its direct conversation is
 * still empty and the name is unchanged, the chat offers a one-time rename;
 * saving calls the ordinary agent update, so the conversation title follows and
 * mentions use the new name. Keeping the default or renaming both mark the
 * prompt as seen for this agent on this device.
 */

const DISMISSED_PREFIX = "cowbot.namedAgent.";

function wasDismissed(workspaceId: string, agentId: string): boolean {
  try {
    return (
      window.localStorage.getItem(
        `${DISMISSED_PREFIX}${workspaceId}.${agentId}`,
      ) === "1"
    );
  } catch {
    return false;
  }
}

function markDismissed(workspaceId: string, agentId: string): void {
  try {
    window.localStorage.setItem(
      `${DISMISSED_PREFIX}${workspaceId}.${agentId}`,
      "1",
    );
  } catch {
    // A read-only WebView storage must not block the rename.
  }
}

interface AgentNamingPromptProps {
  agent: AgentProfile;
  workspaceId: string;
  onNamed: () => Promise<void>;
}

export default function AgentNamingPrompt({
  agent,
  workspaceId,
  onNamed,
}: AgentNamingPromptProps) {
  const t = useT();
  const [name, setName] = useState(agent.name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(() =>
    wasDismissed(workspaceId, agent.id),
  );

  if (dismissed) return null;

  function keepDefault() {
    markDismissed(workspaceId, agent.id);
    setDismissed(true);
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    setBusy(true);
    setError(null);
    try {
      await updateAgent(agent.id, trimmed, agent.role, agent.tools);
      await onNamed();
      markDismissed(workspaceId, agent.id);
      setDismissed(true);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="agent-naming" onSubmit={submit}>
      <h3 className="agent-naming-title">{t("mainAgent.nameTitle")}</h3>
      <p className="settings-hint">
        {t("mainAgent.nameBody", { name: agent.name })}
      </p>
      <label className="field">
        <span>{t("mainAgent.nameLabel")}</span>
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          maxLength={80}
          autoFocus
          autoComplete="off"
        />
      </label>
      {error ? (
        <p className="dialog-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="form-actions">
        <button
          type="submit"
          className="primary-button"
          disabled={busy || !name.trim()}
        >
          {busy ? t("app.dialog.saving") : t("mainAgent.saveName")}
        </button>
        <button
          type="button"
          className="ghost-button"
          onClick={keepDefault}
          disabled={busy}
        >
          {t("mainAgent.keepName", { name: agent.name })}
        </button>
      </div>
    </form>
  );
}
