import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import {
  activateModelProvider,
  deleteModelProvider,
  fetchProviderModels,
  getModelSettings,
  saveModelProvider,
  setActiveModel,
  testModelProvider,
} from "./api";
import type { FetchedModel } from "./api";
import { errorMessage } from "./errors";
import { t, type TranslationKey } from "./i18n/catalog";
import SelectMenu from "./SelectMenu";
import type {
  ModelMetadata,
  ModelProvider,
  ModelSettings,
  ProviderProtocol,
} from "./types";

interface ProtocolOption {
  id: ProviderProtocol;
  labelKey: TranslationKey;
  descriptionKey: TranslationKey;
}

const protocolOptions: ProtocolOption[] = [
  {
    id: "openai",
    labelKey: "models.protocol.openai",
    descriptionKey: "models.protocol.openaiDescription",
  },
  {
    id: "anthropic",
    labelKey: "models.protocol.anthropic",
    descriptionKey: "models.protocol.anthropicDescription",
  },
  {
    id: "openai-responses",
    labelKey: "models.protocol.responses",
    descriptionKey: "models.protocol.responsesDescription",
  },
];

function protocolLabel(protocol: ProviderProtocol): string {
  const option = protocolOptions.find((candidate) => candidate.id === protocol);
  return option ? t(option.labelKey) : protocol;
}

function protocolDescription(protocol: ProviderProtocol): string {
  const option = protocolOptions.find((candidate) => candidate.id === protocol);
  return option ? t(option.descriptionKey) : "";
}

type CatalogueCategory = "vendors" | "platforms" | "cloud" | "local";

interface CatalogueGroup {
  id: CatalogueCategory;
  labelKey: TranslationKey;
}

const catalogueGroups: CatalogueGroup[] = [
  { id: "vendors", labelKey: "models.catalogueGroup.vendors" },
  { id: "platforms", labelKey: "models.catalogueGroup.platforms" },
  { id: "cloud", labelKey: "models.catalogueGroup.cloud" },
  { id: "local", labelKey: "models.catalogueGroup.local" },
];

interface CatalogueEntry {
  id: string;
  name: string;
  category: CatalogueCategory;
  protocol: ProviderProtocol;
  baseUrl: string;
  model: string;
  descriptionKey: TranslationKey;
}

const catalogue: CatalogueEntry[] = [
  // Model vendors
  {
    id: "openai",
    name: "OpenAI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4o",
    descriptionKey: "models.catalogueDescription.openai",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    category: "vendors",
    protocol: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet-5-5",
    descriptionKey: "models.catalogueDescription.anthropic",
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.deepseek.com",
    model: "deepseek-chat",
    descriptionKey: "models.catalogueDescription.deepseek",
  },
  {
    id: "qwen",
    name: "Qwen",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "qwen-plus",
    descriptionKey: "models.catalogueDescription.qwen",
  },
  {
    id: "kimi",
    name: "Kimi",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.moonshot.cn/v1",
    model: "moonshot-v1-8k",
    descriptionKey: "models.catalogueDescription.kimi",
  },
  {
    id: "zhipu",
    name: "Zhipu AI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4.5",
    descriptionKey: "models.catalogueDescription.zhipu",
  },
  {
    id: "gemini",
    name: "Google Gemini",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    model: "gemini-2.5-pro",
    descriptionKey: "models.catalogueDescription.gemini",
  },
  {
    id: "xai",
    name: "xAI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.x.ai/v1",
    model: "grok-4",
    descriptionKey: "models.catalogueDescription.xai",
  },
  {
    id: "mistral",
    name: "Mistral AI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.mistral.ai/v1",
    model: "mistral-large-latest",
    descriptionKey: "models.catalogueDescription.mistral",
  },
  // Third-party platforms and relays
  {
    id: "openrouter",
    name: "OpenRouter",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o",
    descriptionKey: "models.catalogueDescription.openrouter",
  },
  {
    id: "siliconflow",
    name: "SiliconFlow",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.siliconflow.cn/v1",
    model: "deepseek-ai/DeepSeek-V3",
    descriptionKey: "models.catalogueDescription.siliconflow",
  },
  {
    id: "groq",
    name: "Groq",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.groq.com/openai/v1",
    model: "llama-3.3-70b-versatile",
    descriptionKey: "models.catalogueDescription.groq",
  },
  {
    id: "together",
    name: "Together AI",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.together.xyz/v1",
    model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
    descriptionKey: "models.catalogueDescription.together",
  },
  {
    id: "fireworks",
    name: "Fireworks AI",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.fireworks.ai/inference/v1",
    model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
    descriptionKey: "models.catalogueDescription.fireworks",
  },
  // Cloud providers
  {
    id: "volcengine",
    name: "Volcengine Ark",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
    model: "doubao-seed-1-6",
    descriptionKey: "models.catalogueDescription.volcengine",
  },
  {
    id: "hunyuan",
    name: "Tencent Cloud Hunyuan",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://api.hunyuan.cloud.tencent.com/v1",
    model: "hunyuan-turbos-latest",
    descriptionKey: "models.catalogueDescription.hunyuan",
  },
  {
    id: "qianfan",
    name: "Baidu Qianfan",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://qianfan.baidubce.com/v2",
    model: "ernie-4.5-turbo-128k",
    descriptionKey: "models.catalogueDescription.qianfan",
  },
  // Local runtimes
  {
    id: "ollama",
    name: "Ollama",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.2",
    descriptionKey: "models.catalogueDescription.ollama",
  },
  {
    id: "lmstudio",
    name: "LM Studio",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:1234/v1",
    model: "local-model",
    descriptionKey: "models.catalogueDescription.lmstudio",
  },
  {
    id: "vllm",
    name: "vLLM",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:8000/v1",
    model: "",
    descriptionKey: "models.catalogueDescription.vllm",
  },
];

/// Everything the connection editor needs, whether it edits a saved provider
/// or a catalogue entry that has not been saved yet.
interface ConnectionDraft {
  providerId: string | null;
  name: string;
  baseUrl: string;
  protocol: ProviderProtocol;
  apiKey: string;
  clearApiKey: boolean;
  /// The model a new provider starts with; a saved provider keeps its own.
  model: string;
  models: string[];
  modelMetadata: Record<string, ModelMetadata>;
}

function draftFromCatalogue(entry: CatalogueEntry | null): ConnectionDraft {
  return {
    providerId: null,
    name: entry?.name ?? "",
    baseUrl: entry?.baseUrl ?? "",
    protocol: entry?.protocol ?? "openai",
    apiKey: "",
    clearApiKey: false,
    model: entry?.model ?? "",
    models: entry?.model ? [entry.model] : [],
    modelMetadata: {},
  };
}

function draftFromProvider(provider: ModelProvider): ConnectionDraft {
  return {
    providerId: provider.id,
    name: provider.name,
    baseUrl: provider.base_url,
    protocol: provider.protocol,
    apiKey: "",
    clearApiKey: false,
    model: provider.model,
    models: provider.models,
    modelMetadata: provider.model_metadata,
  };
}

/// Trim, drop blanks, and collapse duplicates while keeping the first
/// occurrence of every model.
function mergeModels(...lists: string[][]): string[] {
  const merged: string[] = [];
  for (const list of lists) {
    for (const raw of list) {
      const value = raw.trim();
      if (value && !merged.includes(value)) merged.push(value);
    }
  }
  return merged;
}

/// A token count in the compact form a connection row has room for.
function compactTokens(tokens: number): string {
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}M`;
  }
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k`;
  return String(tokens);
}

function windowLabel(metadata: ModelMetadata | undefined): string {
  return metadata?.context_window
    ? t("models.window", { tokens: compactTokens(metadata.context_window) })
    : t("models.windowUnknown");
}

function modelCountLabel(count: number): string {
  return t(count === 1 ? "models.countOne" : "models.countOther", { count });
}

function optionalTokenCount(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed <= 0) return undefined;
  return Math.floor(parsed);
}

/// Identifies one model on one provider inside the default-model selector.
///
/// JSON keeps the two parts unambiguous whatever a provider id or a model name
/// contains, which a separator character cannot promise.
function modelKey(providerId: string, model: string): string {
  return JSON.stringify([providerId, model]);
}

function parseModelKey(value: string): { providerId: string; model: string } | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (
      Array.isArray(parsed) &&
      parsed.length === 2 &&
      typeof parsed[0] === "string" &&
      typeof parsed[1] === "string"
    ) {
      return { providerId: parsed[0], model: parsed[1] };
    }
  } catch {
    // Not a model key; the caller ignores it.
  }
  return null;
}

interface ModelProviderSettingsProps {
  /// Hide the configuration file path when embedded in a guided flow.
  showConfigPath?: boolean;
}

/// The default model every run starts from, and the connections that supply
/// one (ADR-0048).
///
/// Only this selector changes which model agents run. Connections hold the
/// endpoint, credentials, model list, and per-model limits; saving one never
/// changes the default.
export default function ModelProviderSettings({
  showConfigPath = true,
}: ModelProviderSettingsProps) {
  const [modelSettings, setModelSettings] = useState<ModelSettings | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editor, setEditor] = useState<ConnectionDraft | null>(null);
  const [catalogueOpen, setCatalogueOpen] = useState(false);
  const [catalogueQuery, setCatalogueQuery] = useState("");
  const [managerId, setManagerId] = useState<string | null>(null);
  const [limitsFor, setLimitsFor] = useState<{
    providerId: string;
    model: string;
  } | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  useEffect(() => {
    let cancelled = false;

    getModelSettings()
      .then((settings) => {
        if (!cancelled) setModelSettings(settings);
      })
      .catch((error) => {
        if (!cancelled) setError(errorMessage(error));
      });

    return () => {
      cancelled = true;
    };
  }, []);

  if (!modelSettings) {
    return <p className="settings-empty">{t("models.loading")}</p>;
  }

  const providers = modelSettings.providers;
  const activeProvider = providers.find((provider) => provider.active) ?? null;
  const activeValue = activeProvider
    ? modelKey(activeProvider.id, activeProvider.model)
    : "offline";
  const readyProviders = providers.filter((provider) => provider.models.length > 0);
  const defaultOptions = [
    { value: "offline", label: t("models.default.offline") },
    ...readyProviders.flatMap((provider) =>
      provider.models.map((model) => ({
        value: modelKey(provider.id, model),
        label: model,
        group: provider.name,
      })),
    ),
  ];

  function applySettings(next: ModelSettings, message: string) {
    setModelSettings(next);
    setNotice(message);
    setError(null);
  }

  function resetTransient() {
    setError(null);
    setNotice(null);
  }

  async function handleDefaultChange(value: string) {
    resetTransient();
    setBusy("default");
    try {
      if (value === "offline") {
        applySettings(
          await activateModelProvider(null),
          t("models.notice.offlineSelected"),
        );
        return;
      }
      const selection = parseModelKey(value);
      if (!selection) return;
      applySettings(
        await setActiveModel(selection.providerId, selection.model),
        t("models.notice.defaultSet", { model: selection.model }),
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleTestDefault() {
    if (!activeProvider) {
      setNotice(t("models.notice.offlineNoEndpoint"));
      return;
    }
    resetTransient();
    setBusy("test-default");
    try {
      const message = await testModelProvider({
        providerId: activeProvider.id,
        protocol: activeProvider.protocol,
        baseUrl: activeProvider.base_url,
        model: activeProvider.model,
        apiKey: "",
      });
      setNotice(message);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleTestProvider(provider: ModelProvider) {
    resetTransient();
    setBusy(`test-${provider.id}`);
    try {
      const message = await testModelProvider({
        providerId: provider.id,
        protocol: provider.protocol,
        baseUrl: provider.base_url,
        model: provider.model,
        apiKey: "",
      });
      setNotice(t("models.notice.providerTest", {
        provider: provider.name,
        result: message,
      }));
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleDeleteProvider(provider: ModelProvider) {
    if (
      !window.confirm(
        t("models.confirmDelete", { name: provider.name }),
      )
    ) {
      return;
    }

    resetTransient();
    setBusy(`delete-${provider.id}`);
    try {
      const next = await deleteModelProvider(provider.id);
      setModelSettings(next);
      if (editor?.providerId === provider.id) setEditor(null);
      if (managerId === provider.id) setManagerId(null);
      if (limitsFor?.providerId === provider.id) setLimitsFor(null);
      setNotice(t("models.notice.deleteConnection"));
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  function openCatalogue() {
    setEditor(null);
    setCatalogueOpen(true);
    setCatalogueQuery("");
    resetTransient();
  }

  function chooseCatalogueEntry(entry: CatalogueEntry | null) {
    setEditor(draftFromCatalogue(entry));
    setCatalogueOpen(false);
    resetTransient();
  }

  function startEditProvider(provider: ModelProvider) {
    setEditor(draftFromProvider(provider));
    setCatalogueOpen(false);
    setManagerId(null);
    setLimitsFor(null);
    resetTransient();
  }

  const query = catalogueQuery.trim().toLowerCase();
  const filteredCatalogue = query
    ? catalogue.filter((entry) =>
        [
          entry.name,
          t(entry.descriptionKey),
          entry.baseUrl,
          protocolLabel(entry.protocol),
        ].some((value) => value.toLowerCase().includes(query)),
      )
    : catalogue;

  return (
    <>
      <section className="settings-default" aria-labelledby="settings-default-model">
        <div className="settings-default-head">
          <h4 id="settings-default-model" className="settings-card-title">
            {t("models.default.title")}
          </h4>
          <span className="settings-meta-inline">
            {t("models.default.hint")}
          </span>
        </div>
        <div className="settings-default-row">
          <SelectMenu
            className="settings-default-select"
            ariaLabel={t("models.default.aria")}
            value={activeValue}
            options={defaultOptions}
            disabled={busy !== null}
            onChange={(value) => void handleDefaultChange(value)}
          />
          <button
            type="button"
            className="ghost-button"
            onClick={() => void handleTestDefault()}
            disabled={busy !== null}
          >
            {busy === "test-default"
              ? t("models.default.testing")
              : t("models.default.test")}
          </button>
        </div>
        <p className="settings-card-body settings-default-meta">
          {activeProvider
            ? t("models.default.meta", {
                protocol: protocolLabel(activeProvider.protocol),
                url: activeProvider.base_url,
                window: windowLabel(
                  activeProvider.model_metadata[activeProvider.model],
                ),
              })
            : t("models.default.offlineMeta")}
        </p>
      </section>

      {error ? (
        <p className="settings-status is-error" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p className="settings-status is-ok" role="status">
          {notice}
        </p>
      ) : null}

      <div className="settings-subhead">
        <h4 className="settings-card-title">{t("models.connections.title")}</h4>
        {!catalogueOpen && !editor ? (
          <button type="button" className="settings-add-button" onClick={openCatalogue}>
            <span aria-hidden="true">+</span> {t("models.connections.add")}
          </button>
        ) : null}
      </div>

      {providers.length === 0 ? (
        <p className="settings-empty">
          {t("models.connections.empty")}
        </p>
      ) : null}

      <ul className="settings-list">
        {providers.map((provider) => {
          const isManager = managerId === provider.id;
          const isExpanded = expanded[provider.id] ?? false;
          const visible = isExpanded ? provider.models : provider.models.slice(0, 6);
          return (
            <li key={provider.id} className="settings-card">
              <div className="settings-card-head">
                <span className="settings-card-title">{provider.name}</span>
                <span className="settings-badge">{protocolLabel(provider.protocol)}</span>
                {provider.active ? (
                  <span className="settings-badge settings-badge-active">
                    {t("models.default.badge")}
                  </span>
                ) : null}
              </div>
              <p className="settings-card-body settings-path">{provider.base_url}</p>

              {provider.models.length === 0 ? (
                <p className="settings-hint">
                  {t("models.connections.noModels", {
                    manage: t("models.connections.manageModels"),
                  })}
                </p>
              ) : (
                <ul className="provider-models">
                  {visible.map((model) => {
                    const isDefault =
                      provider.active && model === provider.model;
                    const editingLimits =
                      limitsFor?.providerId === provider.id &&
                      limitsFor.model === model;
                    return (
                      <li key={model} className="provider-model">
                        <button
                          type="button"
                          className={`provider-model-use${isDefault ? " is-default" : ""}`}
                          onClick={() => void handleDefaultChange(modelKey(provider.id, model))}
                          disabled={busy !== null}
                          aria-current={isDefault ? "true" : undefined}
                          title={
                            isDefault
                              ? t("models.connections.defaultTitle")
                              : t("models.connections.useDefaultTitle")
                          }
                        >
                          {isDefault ? "✓ " : ""}
                          {model}
                        </button>
                        <span className="settings-meta-inline">
                          {windowLabel(provider.model_metadata[model])}
                        </span>
                        <button
                          type="button"
                          className="text-button"
                          onClick={() =>
                            setLimitsFor(
                              editingLimits
                                ? null
                                : { providerId: provider.id, model },
                            )
                          }
                        >
                          {editingLimits
                            ? t("models.connections.close")
                            : t("models.connections.limits")}
                        </button>
                        {editingLimits ? (
                          <ModelLimitsEditor
                            key={model}
                            provider={provider}
                            model={model}
                            onSaved={(next, message) => {
                              setModelSettings(next);
                              setLimitsFor(null);
                              setNotice(message);
                              setError(null);
                            }}
                            onClose={() => setLimitsFor(null)}
                          />
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              )}

              {provider.models.length > 6 ? (
                <button
                  type="button"
                  className="text-button provider-models-toggle"
                  onClick={() =>
                    setExpanded((current) => ({
                      ...current,
                      [provider.id]: !isExpanded,
                    }))
                  }
                >
                  {isExpanded
                    ? t("models.connections.showFewer")
                    : t("models.connections.showAll", {
                        count: provider.models.length,
                      })}
                </button>
              ) : null}

              <div className="settings-card-foot">
                <span className="settings-meta-inline">
                  {provider.has_api_key
                    ? t("models.connections.apiKeySaved")
                    : t("models.connections.noApiKey")}
                </span>
                <div className="settings-card-actions">
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => {
                      setManagerId(isManager ? null : provider.id);
                      setLimitsFor(null);
                    }}
                    disabled={busy !== null}
                  >
                    {isManager
                      ? t("models.connections.closeModels")
                      : t("models.connections.manage")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void handleTestProvider(provider)}
                    disabled={busy !== null}
                  >
                    {busy === `test-${provider.id}`
                      ? t("models.connections.testing")
                      : t("models.connections.test")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => startEditProvider(provider)}
                    disabled={busy !== null}
                  >
                    {t("models.connections.edit")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void handleDeleteProvider(provider)}
                    disabled={busy !== null}
                  >
                    {busy === `delete-${provider.id}`
                      ? t("models.connections.deleting")
                      : t("models.connections.delete")}
                  </button>
                </div>
              </div>

              {isManager ? (
                <ManageModelsPanel
                  key={provider.id}
                  provider={provider}
                  onSaved={(next, message) => {
                    setModelSettings(next);
                    setNotice(message);
                    setError(null);
                    setManagerId(null);
                  }}
                  onClose={() => setManagerId(null)}
                />
              ) : null}
            </li>
          );
        })}
      </ul>

      {editor ? (
        <ConnectionEditor
          key={editor.providerId ?? "new-connection"}
          initial={editor}
          hasStoredKey={
            editor.providerId
              ? (providers.find((provider) => provider.id === editor.providerId)
                  ?.has_api_key ?? false)
              : false
          }
          onSaved={(next, message) => {
            setModelSettings(next);
            setEditor(null);
            setNotice(message);
            setError(null);
          }}
          onCancel={() => setEditor(null)}
        />
      ) : catalogueOpen ? (
        <div className="provider-catalogue">
          <div className="catalogue-toolbar">
            <input
              className="catalogue-search"
              value={catalogueQuery}
              onChange={(event) => setCatalogueQuery(event.target.value)}
              placeholder={t("models.catalogue.searchPlaceholder")}
              aria-label={t("models.catalogue.searchAria")}
              autoComplete="off"
              autoFocus
            />
            <button
              type="button"
              className="ghost-button"
              onClick={() => setCatalogueOpen(false)}
            >
              {t("models.catalogue.cancel")}
            </button>
          </div>

          <button
            type="button"
            className="catalogue-custom"
            onClick={() => chooseCatalogueEntry(null)}
          >
            <span className="catalogue-entry-label">
              {t("models.catalogue.custom")}
            </span>
            <span className="catalogue-entry-hint">
              {t("models.catalogue.customHint")}
            </span>
          </button>

          {catalogueGroups.map((group) => {
            const entries = filteredCatalogue.filter(
              (entry) => entry.category === group.id,
            );
            if (entries.length === 0) return null;
            return (
              <section key={group.id} className="catalogue-group">
                <h4 className="catalogue-group-title">{t(group.labelKey)}</h4>
                <ul className="catalogue-list">
                  {entries.map((entry) => (
                    <li key={entry.id}>
                      <button
                        type="button"
                        className="catalogue-entry"
                        onClick={() => chooseCatalogueEntry(entry)}
                      >
                        <span className="catalogue-entry-label">
                          {entry.name}
                        </span>
                        <span className="catalogue-entry-hint">
                          {t(entry.descriptionKey)} · {entry.baseUrl}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            );
          })}

          {filteredCatalogue.length === 0 ? (
            <p className="settings-empty">
              {t("models.catalogue.empty", {
                query: catalogueQuery.trim(),
              })}
            </p>
          ) : null}
        </div>
      ) : null}

      {showConfigPath ? (
        <details className="settings-advanced">
          <summary>{t("models.advanced.summary")}</summary>
          <p className="settings-note settings-path">
            {t("models.advanced.config", {
              path: modelSettings.config_path,
            })}
          </p>
        </details>
      ) : null}
    </>
  );
}

interface ConnectionEditorProps {
  initial: ConnectionDraft;
  hasStoredKey: boolean;
  onSaved: (settings: ModelSettings, message: string) => void;
  onCancel: () => void;
}

/// Edit a saved connection, or create a new one from the catalogue.
///
/// An existing connection saves its current model list and metadata unchanged,
/// so editing an endpoint never alters which model runs (ADR-0048).
function ConnectionEditor({
  initial,
  hasStoredKey,
  onSaved,
  onCancel,
}: ConnectionEditorProps) {
  const [draft, setDraft] = useState<ConnectionDraft>(initial);
  const [fetched, setFetched] = useState<FetchedModel[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const creating = draft.providerId === null;

  function update(patch: Partial<ConnectionDraft>) {
    setDraft((current) => ({ ...current, ...patch }));
    setStatus(null);
  }

  async function handleFetch() {
    setBusy("fetch-models");
    setError(null);
    try {
      const models = await fetchProviderModels({
        providerId: draft.providerId,
        protocol: draft.protocol,
        baseUrl: draft.baseUrl,
        apiKey: draft.apiKey,
      });
      const first = models[0]?.id ?? "";
      const metadata: Record<string, ModelMetadata> = { ...draft.modelMetadata };
      for (const model of models) {
        if (!model.contextWindow && !model.maxOutputTokens) continue;
        const entry: ModelMetadata = { ...(metadata[model.id] ?? {}) };
        if (entry.context_window === undefined && model.contextWindow) {
          entry.context_window = model.contextWindow;
        }
        if (entry.max_output_tokens === undefined && model.maxOutputTokens) {
          entry.max_output_tokens = model.maxOutputTokens;
        }
        metadata[model.id] = entry;
      }
      setFetched(models);
      update({
        model: draft.model.trim() ? draft.model : first,
        models: mergeModels(draft.models, draft.model.trim() ? [] : [first]),
        modelMetadata: metadata,
      });
      const withWindow = models.filter((model) => model.contextWindow).length;
      setStatus(
        withWindow > 0
          ? t("models.notice.fetchedWithWindow", {
              count: modelCountLabel(models.length),
              withWindow,
            })
          : t("models.notice.fetchedSetByHand", {
              count: modelCountLabel(models.length),
            }),
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy("save");
    setError(null);
    try {
      const next = await saveModelProvider({
        providerId: draft.providerId,
        protocol: draft.protocol,
        name: draft.name.trim(),
        baseUrl: draft.baseUrl.trim(),
        models: mergeModels(draft.models, [draft.model]),
        model: draft.model.trim(),
        modelMetadata: draft.modelMetadata,
        apiKey: draft.apiKey,
        clearApiKey: draft.clearApiKey,
      });
      onSaved(
        next,
        creating
          ? t("models.notice.connectionAdded")
          : t("models.notice.connectionUpdated"),
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleTest() {
    setBusy("test");
    setError(null);
    try {
      const message = await testModelProvider({
        providerId: draft.providerId,
        protocol: draft.protocol,
        baseUrl: draft.baseUrl,
        model: draft.model,
        apiKey: draft.apiKey,
      });
      setStatus(message);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <form className="settings-form provider-editor" onSubmit={handleSubmit}>
      <p className="settings-card-title">
        {creating
          ? t("models.editor.newTitle")
          : t("models.editor.editTitle", { name: initial.name })}
      </p>

      <div className="settings-form-row">
        <label className="field">
          <span>{t("models.editor.name")}</span>
          <input
            value={draft.name}
            onChange={(event) => update({ name: event.target.value })}
            placeholder="OpenAI"
            autoComplete="off"
          />
        </label>
        <label className="field">
          <span>{t("models.editor.baseUrl")}</span>
          <input
            value={draft.baseUrl}
            onChange={(event) => update({ baseUrl: event.target.value })}
            placeholder={
              draft.protocol === "anthropic"
                ? "https://api.anthropic.com"
                : "https://api.openai.com/v1"
            }
            autoComplete="off"
          />
        </label>
      </div>

      {creating ? (
        <div className="field">
          <span>{t("models.editor.firstModel")}</span>
          <div className="model-toolbar">
            <input
              aria-label={t("models.editor.modelNameAria")}
              value={draft.model}
              onChange={(event) => update({ model: event.target.value })}
              placeholder="gpt-4o"
              autoComplete="off"
            />
            <button
              type="button"
              className="ghost-button"
              onClick={() => void handleFetch()}
              disabled={busy !== null || !draft.baseUrl.trim()}
            >
              {busy === "fetch-models"
                ? t("models.editor.fetching")
                : t("models.editor.fetchModels")}
            </button>
          </div>
          {fetched.length > 0 ? (
            <SelectMenu
              ariaLabel={t("models.editor.fetchedAria")}
              value={
                fetched.some((model) => model.id === draft.model)
                  ? draft.model
                  : ""
              }
              options={[
                { value: "", label: t("models.editor.selectFetched") },
                ...fetched.map((model) => ({
                  value: model.id,
                  label: model.contextWindow
                    ? `${model.id} · ${compactTokens(model.contextWindow)}`
                    : model.id,
                })),
              ]}
              onChange={(value) =>
                update({
                  model: value,
                  models: mergeModels(draft.models, [value]),
                })
              }
            />
          ) : null}
          <span className="settings-hint">
            {t("models.editor.modelHint")}
          </span>
        </div>
      ) : null}

      <label className="field">
        <span>{t("models.editor.apiKey")}</span>
        <input
          type="password"
          value={draft.apiKey}
          onChange={(event) =>
            update({ apiKey: event.target.value, clearApiKey: false })
          }
          placeholder={
            hasStoredKey
              ? t("models.editor.savedKeyPlaceholder")
              : t("models.editor.optionalKeyPlaceholder")
          }
          autoComplete="off"
        />
      </label>

      {hasStoredKey ? (
        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={draft.clearApiKey}
            onChange={(event) =>
              update({
                clearApiKey: event.target.checked,
                apiKey: event.target.checked ? "" : draft.apiKey,
              })
            }
          />
          {t("models.editor.removeSavedKey")}
        </label>
      ) : null}

      <details className="settings-advanced">
        <summary>{t("models.editor.advanced")}</summary>
        <div className="field">
          <span>{t("models.editor.protocol")}</span>
          <SelectMenu
            ariaLabel={t("models.editor.protocol")}
            value={draft.protocol}
            options={protocolOptions.map((option) => ({
              value: option.id,
              label: t(option.labelKey),
            }))}
            disabled={busy !== null}
            onChange={(value) =>
              update({ protocol: value as ProviderProtocol })
            }
          />
        </div>
        <p className="settings-hint">{protocolDescription(draft.protocol)}</p>
      </details>

      {error ? (
        <p className="settings-status is-error" role="alert">
          {error}
        </p>
      ) : null}
      {status ? (
        <p className="settings-status is-ok" role="status">
          {status}
        </p>
      ) : null}

      <div className="form-actions">
        <button
          type="submit"
          className="primary-button"
          disabled={
            busy !== null ||
            !draft.name.trim() ||
            !draft.baseUrl.trim() ||
            !draft.model.trim()
          }
        >
          {busy === "save"
            ? t("models.editor.saving")
            : creating
              ? t("models.editor.add")
              : t("models.editor.save")}
        </button>
        <button
          type="button"
          className="ghost-button"
          onClick={() => void handleTest()}
          disabled={
            busy !== null || !draft.baseUrl.trim() || !draft.model.trim()
          }
        >
          {busy === "test"
            ? t("models.editor.testing")
            : t("models.editor.test")}
        </button>
        <button
          type="button"
          className="ghost-button"
          onClick={onCancel}
          disabled={busy !== null}
        >
          {t("models.editor.cancel")}
        </button>
      </div>

      <p className="settings-hint">
        {t("models.editor.note")}
      </p>
    </form>
  );
}

interface ModelLimitsEditorProps {
  provider: ModelProvider;
  model: string;
  onSaved: (settings: ModelSettings, message: string) => void;
  onClose: () => void;
}

/// Per-model context window and output limit, stored with the provider.
///
/// Values the endpoint reported arrive prefilled; leaving a field blank keeps
/// the window unknown, which the meter labels instead of guessing (ADR-0029).
function ModelLimitsEditor({
  provider,
  model,
  onSaved,
  onClose,
}: ModelLimitsEditorProps) {
  const initial = provider.model_metadata[model] ?? {};
  const [window, setWindow] = useState(
    initial.context_window ? String(initial.context_window) : "",
  );
  const [output, setOutput] = useState(
    initial.max_output_tokens ? String(initial.max_output_tokens) : "",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const metadata: Record<string, ModelMetadata> = {
        ...provider.model_metadata,
      };
      const entry: ModelMetadata = {};
      const windowTokens = optionalTokenCount(window);
      const outputTokens = optionalTokenCount(output);
      if (windowTokens !== undefined) entry.context_window = windowTokens;
      if (outputTokens !== undefined) entry.max_output_tokens = outputTokens;
      if (Object.keys(entry).length === 0) delete metadata[model];
      else metadata[model] = entry;

      const next = await saveModelProvider({
        providerId: provider.id,
        protocol: provider.protocol,
        name: provider.name,
        baseUrl: provider.base_url,
        models: provider.models,
        model: provider.model,
        modelMetadata: metadata,
        apiKey: "",
        clearApiKey: false,
      });
      onSaved(next, t("models.notice.limitsSaved", { model }));
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="model-limits" onSubmit={handleSave}>
      <div className="settings-form-row">
        <label className="field">
          <span>{t("models.limits.contextWindow")}</span>
          <input
            type="number"
            min={1}
            inputMode="numeric"
            value={window}
            onChange={(event) => setWindow(event.target.value)}
            placeholder="128000"
          />
        </label>
        <label className="field">
          <span>{t("models.limits.maxOutput")}</span>
          <input
            type="number"
            min={1}
            inputMode="numeric"
            value={output}
            onChange={(event) => setOutput(event.target.value)}
            placeholder="8192"
          />
        </label>
      </div>
      {error ? (
        <p className="settings-status is-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="form-actions">
        <button type="submit" className="primary-button" disabled={busy}>
          {busy ? t("models.limits.saving") : t("models.limits.save")}
        </button>
        <button type="button" className="ghost-button" onClick={onClose}>
          {t("models.limits.cancel")}
        </button>
      </div>
    </form>
  );
}

interface ManageModelsPanelProps {
  provider: ModelProvider;
  onSaved: (settings: ModelSettings, message: string) => void;
  onClose: () => void;
}

/// Add, fetch, and remove the models a connection knows.
///
/// Saving here rewrites the model list and its metadata only; the connection's
/// endpoint, key, and default model stay as they are.
function ManageModelsPanel({
  provider,
  onSaved,
  onClose,
}: ManageModelsPanelProps) {
  const [models, setModels] = useState<string[]>(provider.models);
  const [metadata, setMetadata] = useState<Record<string, ModelMetadata>>(
    provider.model_metadata,
  );
  const [fetched, setFetched] = useState<FetchedModel[] | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [manual, setManual] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const filtered = (fetched ?? []).filter((model) =>
    model.id.toLowerCase().includes(filter.trim().toLowerCase()),
  );

  async function handleFetch() {
    setBusy("fetch");
    setError(null);
    try {
      const next = await fetchProviderModels({
        providerId: provider.id,
        protocol: provider.protocol,
        baseUrl: provider.base_url,
        apiKey: "",
      });
      setFetched(next);
      setSelected(next.map((model) => model.id));
      setFilter("");
      const withWindow = next.filter((model) => model.contextWindow).length;
      setStatus(
        withWindow > 0
          ? t("models.notice.fetchedWithWindow", {
              count: modelCountLabel(next.length),
              withWindow,
            })
          : t("models.notice.fetchedWithoutWindow", {
              count: modelCountLabel(next.length),
            }),
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  function toggle(model: string) {
    setSelected((current) =>
      current.includes(model)
        ? current.filter((candidate) => candidate !== model)
        : [...current, model],
    );
  }

  /// Seed the window the endpoint reported; a value already set stays.
  function seedMetadata(source: FetchedModel[]): Record<string, ModelMetadata> {
    const next: Record<string, ModelMetadata> = { ...metadata };
    for (const model of source) {
      if (!model.contextWindow && !model.maxOutputTokens) continue;
      const entry: ModelMetadata = { ...(next[model.id] ?? {}) };
      if (entry.context_window === undefined && model.contextWindow) {
        entry.context_window = model.contextWindow;
      }
      if (entry.max_output_tokens === undefined && model.maxOutputTokens) {
        entry.max_output_tokens = model.maxOutputTokens;
      }
      next[model.id] = entry;
    }
    return next;
  }

  function addSelected() {
    if (selected.length === 0) return;
    const source = (fetched ?? []).filter((model) =>
      selected.includes(model.id),
    );
    setMetadata(seedMetadata(source));
    setModels(mergeModels(models, selected));
    setStatus(
      t("models.notice.addedToProvider", {
        count: modelCountLabel(selected.length),
        provider: provider.name,
      }),
    );
  }

  function addManual() {
    const value = manual.trim();
    if (!value) return;
    if (models.includes(value)) {
      setStatus(t("models.notice.modelKnown", { model: value }));
      return;
    }
    setModels([...models, value]);
    setManual("");
    setStatus(t("models.notice.modelAdded", { model: value }));
  }

  function remove(model: string) {
    if (model === provider.model) return;
    setModels(models.filter((candidate) => candidate !== model));
  }

  async function handleSave() {
    setBusy("save");
    setError(null);
    try {
      const next = await saveModelProvider({
        providerId: provider.id,
        protocol: provider.protocol,
        name: provider.name,
        baseUrl: provider.base_url,
        models: mergeModels(models, [provider.model]),
        model: provider.model,
        modelMetadata: metadata,
        apiKey: "",
        clearApiKey: false,
      });
      onSaved(next, t("models.notice.modelsSaved", { provider: provider.name }));
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="model-manager">
      <div className="model-manager-head">
        <span className="settings-card-title">
          {t("models.manage.title", { provider: provider.name })}
        </span>
        <button type="button" className="text-button" onClick={onClose}>
          {t("models.manage.done")}
        </button>
      </div>

      <div className="model-toolbar">
        <button
          type="button"
          className="ghost-button"
          onClick={() => void handleFetch()}
          disabled={busy !== null}
        >
          {busy === "fetch"
            ? t("models.manage.fetching")
            : t("models.manage.fetch")}
        </button>
        <input
          aria-label={t("models.manage.addAria")}
          value={manual}
          onChange={(event) => setManual(event.target.value)}
          placeholder={t("models.manage.addPlaceholder")}
          autoComplete="off"
        />
        <button
          type="button"
          className="ghost-button"
          onClick={addManual}
          disabled={busy !== null || !manual.trim()}
        >
          {t("models.manage.add")}
        </button>
      </div>

      {fetched ? (
        <div className="model-catalogue-panel">
          <input
            aria-label={t("models.manage.filterAria")}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder={t("models.manage.filterPlaceholder")}
            autoComplete="off"
          />
          <div className="model-toolbar">
            <button
              type="button"
              className="ghost-button"
              onClick={() => setSelected((fetched ?? []).map((model) => model.id))}
            >
              {t("models.manage.selectAll")}
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={() => setSelected([])}
            >
              {t("models.manage.clear")}
            </button>
            <button
              type="button"
              className="primary-button"
              onClick={addSelected}
              disabled={selected.length === 0}
            >
              {t("models.manage.addSelected", { count: selected.length })}
            </button>
          </div>
          <ul className="model-catalogue-list">
            {filtered.map((model) => (
              <li key={model.id}>
                <label>
                  <input
                    type="checkbox"
                    checked={selected.includes(model.id)}
                    onChange={() => toggle(model.id)}
                  />
                  <span>{model.id}</span>
                  <span className="settings-meta-inline">
                    {model.contextWindow
                      ? compactTokens(model.contextWindow)
                      : t("models.manage.noWindow")}
                  </span>
                  {models.includes(model.id) ? (
                    <span className="settings-meta-inline">
                      {t("models.manage.known")}
                    </span>
                  ) : null}
                </label>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="field">
        <span>
          {t("models.manage.knownModels", { count: models.length })}
        </span>
        <ul className="known-model-list">
          {models.map((model) => (
            <li
              key={model}
              className={model === provider.model ? "is-active" : undefined}
            >
              <span>{model}</span>
              <span className="settings-meta-inline">
                {windowLabel(metadata[model])}
              </span>
              {model === provider.model ? (
                <span className="settings-meta-inline">
                  {t("models.manage.default")}
                </span>
              ) : (
                <button
                  type="button"
                  className="text-button"
                  onClick={() => remove(model)}
                  disabled={busy !== null}
                >
                  {t("models.manage.remove")}
                </button>
              )}
            </li>
          ))}
        </ul>
      </div>

      {error ? (
        <p className="settings-status is-error" role="alert">
          {error}
        </p>
      ) : null}
      {status ? (
        <p className="settings-status is-ok" role="status">
          {status}
        </p>
      ) : null}

      <div className="form-actions">
        <button
          type="button"
          className="primary-button"
          onClick={() => void handleSave()}
          disabled={busy !== null}
        >
          {busy === "save"
            ? t("models.manage.saving")
            : t("models.manage.save")}
        </button>
      </div>

      <p className="settings-hint">
        {t("models.manage.note")}
      </p>
    </div>
  );
}
