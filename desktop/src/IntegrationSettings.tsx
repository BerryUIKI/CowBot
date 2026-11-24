import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";

import {
  bindIntegrationChannel,
  createIntegration,
  listChannels,
  listIntegrationBindings,
  listIntegrations,
  revokeIntegration,
  unbindIntegrationChannel,
} from "./api";
import { errorMessage } from "./errors";
import { useI18n } from "./i18n";
import SelectMenu from "./SelectMenu";
import type { BindingView, Channel, IntegrationView } from "./types";
import type { IntegrationKind } from "./generated/index";

/**
 * Settings → Integrations (ADR-0056).
 *
 * Lists the workspace's IM connections and the conversations bound to each. A
 * bot is created here with its adapter configuration, which never comes back:
 * the list shows a label and a platform, not a token.
 *
 * In a local workspace nothing can run and every kind reports unsupported, so
 * the panel says so instead of offering a create form that would produce a row
 * nothing starts.
 */

const KINDS: IntegrationKind[] = ["mattermost", "telegram", "slack", "feishu"];

const KIND_LABEL: Record<IntegrationKind, string> = {
  mattermost: "Mattermost",
  telegram: "Telegram",
  slack: "Slack",
  feishu: "Feishu",
};

/** A starting configuration, so the field is not blank for each platform. */
const CONFIG_PLACEHOLDER: Record<IntegrationKind, string> = {
  mattermost: '{\n  "base_url": "https://chat.example.com",\n  "bot_token": "…"\n}',
  telegram: '{\n  "bot_token": "123456:ABC…"\n}',
  slack: '{\n  "bot_token": "xoxb-…",\n  "app_token": "xapp-…"\n}',
  feishu: '{\n  "app_id": "cli_…",\n  "app_secret": "…",\n  "domain": "feishu"\n}',
};

export default function IntegrationSettings() {
  const { t } = useI18n();
  const [integrations, setIntegrations] = useState<IntegrationView[] | null>(null);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [bindings, setBindings] = useState<Record<string, BindingView[]>>({});
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // The create form.
  const [kind, setKind] = useState<IntegrationKind>("mattermost");
  const [label, setLabel] = useState("");
  const [config, setConfig] = useState(CONFIG_PLACEHOLDER.mattermost);

  // The bind form, one set of values shared because only one panel is open.
  const [externalId, setExternalId] = useState("");
  const [channelId, setChannelId] = useState("");
  const [agentId, setAgentId] = useState("");

  const refresh = useCallback(async () => {
    const [listed, channelList] = await Promise.all([listIntegrations(), listChannels()]);
    setIntegrations(listed);
    setChannels(channelList);
  }, []);

  useEffect(() => {
    let cancelled = false;
    refresh().catch((err) => {
      if (!cancelled) setError(errorMessage(err));
    });
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  async function loadBindings(integrationId: string) {
    try {
      const listed = await listIntegrationBindings(integrationId);
      setBindings((current) => ({ ...current, [integrationId]: listed }));
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function toggleBindings(integrationId: string) {
    if (expanded === integrationId) {
      setExpanded(null);
      return;
    }
    setExpanded(integrationId);
    await loadBindings(integrationId);
  }

  async function submitCreate(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setNotice(null);
    let parsed: unknown;
    try {
      parsed = JSON.parse(config);
    } catch {
      // Parsed here rather than letting the command reject it, so the error
      // names the field the user is looking at.
      setError(t("settings.integrations.configHint"));
      return;
    }
    setBusy("create");
    try {
      await createIntegration(kind, label, parsed);
      setLabel("");
      setNotice(t("settings.integrations.created"));
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function submitBind(event: FormEvent, integrationId: string) {
    event.preventDefault();
    setError(null);
    setNotice(null);
    setBusy(`bind:${integrationId}`);
    try {
      const bound = await bindIntegrationChannel(
        integrationId,
        externalId,
        channelId,
        agentId || null,
      );
      setExternalId("");
      setChannelId("");
      setAgentId("");
      // A binding the adapter could not confirm is worth saying now: the
      // operator is still on the screen and can go add the bot to the channel.
      const detail = bound.health?.detail;
      setNotice(
        bound.health && bound.health.status !== "reachable" && detail
          ? t("settings.integrations.boundUnreachable", { reason: detail })
          : t("settings.integrations.bound"),
      );
      await loadBindings(integrationId);
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function revoke(integrationId: string) {
    if (!window.confirm(t("settings.integrations.revokeConfirm"))) return;
    setError(null);
    setNotice(null);
    setBusy(`revoke:${integrationId}`);
    try {
      await revokeIntegration(integrationId);
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function unbind(integrationId: string, external: string) {
    setError(null);
    setNotice(null);
    setBusy(`unbind:${external}`);
    try {
      await unbindIntegrationChannel(integrationId, external);
      await loadBindings(integrationId);
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="settings-section" aria-labelledby="settings-integrations">
      <div className="settings-section-head">
        <h3 id="settings-integrations">{t("settings.integrations.title")}</h3>
        <p className="settings-hint">{t("settings.integrations.hint")}</p>
      </div>

      {error ? <p className="settings-error">{t("settings.integrations.error", { error })}</p> : null}
      {notice ? <p className="settings-hint">{notice}</p> : null}

      {integrations === null ? null : integrations.length === 0 ? (
        <p className="settings-hint">{t("settings.integrations.empty")}</p>
      ) : (
        <ul className="integration-list">
          {integrations.map((entry) => {
            const bound = bindings[entry.id];
            const retired = entry.revoked_at !== null;
            return (
              <li key={entry.id} className="integration-card">
                <div className="integration-head">
                  <div>
                    <span className="settings-card-title">{entry.label}</span>
                    <p className="settings-hint">
                      {KIND_LABEL[entry.kind]}
                      {" · "}
                      {entry.binding_count === 1
                        ? t("settings.integrations.bindingCountOne", {
                            count: entry.binding_count,
                          })
                        : t("settings.integrations.bindingCountOther", {
                            count: entry.binding_count,
                          })}
                      {retired ? ` · ${t("settings.integrations.revoked")}` : ""}
                      {entry.supported ? "" : ` · ${t("settings.integrations.unsupported")}`}
                    </p>
                  </div>
                  <div className="integration-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => void toggleBindings(entry.id)}
                    >
                      {expanded === entry.id
                        ? t("settings.integrations.hideBindings")
                        : t("settings.integrations.showBindings")}
                    </button>
                    {retired ? null : (
                      <button
                        type="button"
                        className="ghost-button"
                        disabled={busy !== null}
                        onClick={() => void revoke(entry.id)}
                      >
                        {busy === `revoke:${entry.id}`
                          ? t("settings.integrations.revoking")
                          : t("settings.integrations.revoke")}
                      </button>
                    )}
                  </div>
                </div>

                {expanded === entry.id ? (
                  <div className="integration-bindings">
                    {bound === undefined ? null : bound.length === 0 ? (
                      <p className="settings-hint">
                        {t("settings.integrations.bindingsEmpty")}
                      </p>
                    ) : (
                      <ul className="integration-binding-list">
                        {bound.map((binding) => (
                          <li key={binding.external_id} className="integration-binding">
                            <code>{binding.external_id}</code>
                            <span aria-hidden="true">→</span>
                            <span>
                              {channels.find((channel) => channel.id === binding.channel_id)
                                ?.name ?? binding.channel_id}
                            </span>
                            <button
                              type="button"
                              className="ghost-button"
                              disabled={busy !== null}
                              onClick={() => void unbind(entry.id, binding.external_id)}
                            >
                              {busy === `unbind:${binding.external_id}`
                                ? t("settings.integrations.unbinding")
                                : t("settings.integrations.unbind")}
                            </button>
                          </li>
                        ))}
                      </ul>
                    )}

                    {retired ? null : (
                      <form
                        className="integration-bind-form"
                        onSubmit={(event) => void submitBind(event, entry.id)}
                      >
                        <label>
                          {t("settings.integrations.externalId")}
                          <input
                            value={externalId}
                            onChange={(event) => setExternalId(event.target.value)}
                            required
                          />
                        </label>
                        <p className="settings-hint">
                          {t("settings.integrations.externalIdHint")}
                        </p>
                        <label>
                          {t("settings.integrations.channel")}
                          <SelectMenu
                            value={channelId}
                            ariaLabel={t("settings.integrations.channelPlaceholder")}
                            options={channels.map((channel) => ({
                              value: channel.id,
                              label: channel.name,
                            }))}
                            onChange={setChannelId}
                          />
                        </label>
                        <label>
                          {t("settings.integrations.agent")}
                          <input value={agentId} onChange={(event) => setAgentId(event.target.value)} />
                        </label>
                        <p className="settings-hint">{t("settings.integrations.agentHint")}</p>
                        <button type="submit" className="primary-button" disabled={busy !== null}>
                          {busy === `bind:${entry.id}`
                            ? t("settings.integrations.bindingBusy")
                            : t("settings.integrations.binding")}
                        </button>
                      </form>
                    )}
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}

      <form className="integration-create" onSubmit={(event) => void submitCreate(event)}>
        <h4>{t("settings.integrations.addTitle")}</h4>
        <label>
          {t("settings.integrations.kind")}
          <SelectMenu
            value={kind}
            ariaLabel={t("settings.integrations.kind")}
            options={KINDS.map((value) => ({ value, label: KIND_LABEL[value] }))}
            onChange={(value) => {
              const next = value as IntegrationKind;
              setKind(next);
              setConfig(CONFIG_PLACEHOLDER[next]);
            }}
          />
        </label>
        <p className="settings-hint">{t("settings.integrations.kindHint")}</p>
        <label>
          {t("settings.integrations.label")}
          <input
            value={label}
            placeholder={t("settings.integrations.labelPlaceholder")}
            onChange={(event) => setLabel(event.target.value)}
            required
          />
        </label>
        <p className="settings-hint">{t("settings.integrations.labelHint")}</p>
        <label>
          {t("settings.integrations.config")}
          <textarea
            className="integration-config"
            value={config}
            spellCheck={false}
            onChange={(event) => setConfig(event.target.value)}
            required
          />
        </label>
        <p className="settings-hint">{t("settings.integrations.configHint")}</p>
        <button type="submit" className="primary-button" disabled={busy !== null}>
          {busy === "create"
            ? t("settings.integrations.creating")
            : t("settings.integrations.create")}
        </button>
      </form>
    </section>
  );
}
