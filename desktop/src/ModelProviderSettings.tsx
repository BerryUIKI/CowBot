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
import type {
  ModelMetadata,
  ModelProvider,
  ModelSettings,
  ProviderProtocol,
} from "./types";

interface ProtocolOption {
  id: ProviderProtocol;
  label: string;
  description: string;
}

const protocolOptions: ProtocolOption[] = [
  {
    id: "openai",
    label: "OpenAI Chat",
    description: "Chat Completions API. OpenAI, DeepSeek, Ollama, and most relays.",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    description: "Messages API. Claude and Claude Code-compatible relays.",
  },
  {
    id: "openai-responses",
    label: "Responses",
    description: "Responses API. OpenAI and Codex-style endpoints.",
  },
];

function protocolLabel(protocol: ProviderProtocol): string {
  return (
    protocolOptions.find((option) => option.id === protocol)?.label ?? protocol
  );
}

function protocolDescription(protocol: ProviderProtocol): string {
  return (
    protocolOptions.find((option) => option.id === protocol)?.description ?? ""
  );
}

type CatalogueCategory = "vendors" | "platforms" | "cloud" | "local";

interface CatalogueGroup {
  id: CatalogueCategory;
  label: string;
}

const catalogueGroups: CatalogueGroup[] = [
  { id: "vendors", label: "Model vendors" },
  { id: "platforms", label: "Third-party platforms and relays" },
  { id: "cloud", label: "Cloud providers" },
  { id: "local", label: "Local runtimes" },
];

interface CatalogueEntry {
  id: string;
  name: string;
  category: CatalogueCategory;
  protocol: ProviderProtocol;
  baseUrl: string;
  model: string;
  description: string;
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
    description: "Official OpenAI API",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    category: "vendors",
    protocol: "anthropic",
    baseUrl: "https://api.anthropic.com",
    model: "claude-sonnet-5-5",
    description: "Claude models",
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.deepseek.com",
    model: "deepseek-chat",
    description: "DeepSeek chat and reasoning models",
  },
  {
    id: "qwen",
    name: "Qwen",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    model: "qwen-plus",
    description: "Alibaba Qwen models",
  },
  {
    id: "kimi",
    name: "Kimi",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.moonshot.cn/v1",
    model: "moonshot-v1-8k",
    description: "Moonshot AI Kimi models",
  },
  {
    id: "zhipu",
    name: "Zhipu AI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    model: "glm-4.5",
    description: "GLM models",
  },
  {
    id: "gemini",
    name: "Google Gemini",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    model: "gemini-2.5-pro",
    description: "Gemini through its OpenAI-compatible endpoint",
  },
  {
    id: "xai",
    name: "xAI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.x.ai/v1",
    model: "grok-4",
    description: "Grok models",
  },
  {
    id: "mistral",
    name: "Mistral AI",
    category: "vendors",
    protocol: "openai",
    baseUrl: "https://api.mistral.ai/v1",
    model: "mistral-large-latest",
    description: "Mistral and Codestral models",
  },
  // Third-party platforms and relays
  {
    id: "openrouter",
    name: "OpenRouter",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o",
    description: "Multi-vendor router",
  },
  {
    id: "siliconflow",
    name: "SiliconFlow",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.siliconflow.cn/v1",
    model: "deepseek-ai/DeepSeek-V3",
    description: "Hosted open models",
  },
  {
    id: "groq",
    name: "Groq",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.groq.com/openai/v1",
    model: "llama-3.3-70b-versatile",
    description: "Low-latency hosted inference",
  },
  {
    id: "together",
    name: "Together AI",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.together.xyz/v1",
    model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
    description: "Hosted open models",
  },
  {
    id: "fireworks",
    name: "Fireworks AI",
    category: "platforms",
    protocol: "openai",
    baseUrl: "https://api.fireworks.ai/inference/v1",
    model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
    description: "Hosted open models",
  },
  // Cloud providers
  {
    id: "volcengine",
    name: "Volcengine Ark",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
    model: "doubao-seed-1-6",
    description: "ByteDance Doubao models",
  },
  {
    id: "hunyuan",
    name: "Tencent Cloud Hunyuan",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://api.hunyuan.cloud.tencent.com/v1",
    model: "hunyuan-turbos-latest",
    description: "Tencent Hunyuan models",
  },
  {
    id: "qianfan",
    name: "Baidu Qianfan",
    category: "cloud",
    protocol: "openai",
    baseUrl: "https://qianfan.baidubce.com/v2",
    model: "ernie-4.5-turbo-128k",
    description: "Baidu ERNIE models",
  },
  // Local runtimes
  {
    id: "ollama",
    name: "Ollama",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:11434/v1",
    model: "llama3.2",
    description: "Local Ollama server on port 11434",
  },
  {
    id: "lmstudio",
    name: "LM Studio",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:1234/v1",
    model: "local-model",
    description: "Local LM Studio server on port 1234",
  },
  {
    id: "vllm",
    name: "vLLM",
    category: "local",
    protocol: "openai",
    baseUrl: "http://localhost:8000/v1",
    model: "",
    description: "Self-hosted OpenAI-compatible server on port 8000",
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
    ? `${compactTokens(metadata.context_window)} window`
    : "window unknown";
}

function modelCountLabel(count: number): string {
  return count === 1 ? "1 model" : `${count} models`;
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
    return <p className="settings-empty">Loading model settings…</p>;
  }

  const providers = modelSettings.providers;
  const activeProvider = providers.find((provider) => provider.active) ?? null;
  const activeValue = activeProvider
    ? modelKey(activeProvider.id, activeProvider.model)
    : "offline";
  const readyProviders = providers.filter((provider) => provider.models.length > 0);

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
          "Offline mode selected. Runs use the local echo model.",
        );
        return;
      }
      const selection = parseModelKey(value);
      if (!selection) return;
      applySettings(
        await setActiveModel(selection.providerId, selection.model),
        `Default model set to ${selection.model}.`,
      );
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleTestDefault() {
    if (!activeProvider) {
      setNotice("Offline mode does not call an endpoint.");
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
      setNotice(`${provider.name}: ${message}`);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleDeleteProvider(provider: ModelProvider) {
    if (
      !window.confirm(
        `Delete the connection "${provider.name}"?\n\nConversations that use it switch to the default model. Recorded prompts and usage stay.`,
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
      setNotice("Connection deleted.");
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
          entry.description,
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
            Default model
          </h4>
          <span className="settings-meta-inline">
            Used by tasks, channels, and conversations that follow the default
          </span>
        </div>
        <div className="settings-default-row">
          <select
            aria-label="Default model"
            value={activeValue}
            onChange={(event) => void handleDefaultChange(event.target.value)}
            disabled={busy !== null}
          >
            <option value="offline">Offline (no model)</option>
            {readyProviders.map((provider) => (
              <optgroup key={provider.id} label={provider.name}>
                {provider.models.map((model) => (
                  <option key={model} value={modelKey(provider.id, model)}>
                    {model}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
          <button
            type="button"
            className="ghost-button"
            onClick={() => void handleTestDefault()}
            disabled={busy !== null}
          >
            {busy === "test-default" ? "Testing…" : "Test"}
          </button>
        </div>
        <p className="settings-card-body settings-default-meta">
          {activeProvider
            ? `${protocolLabel(activeProvider.protocol)} · ${activeProvider.base_url} · ${windowLabel(activeProvider.model_metadata[activeProvider.model])}`
            : "No endpoint is called. Runs use the local echo model."}
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
        <h4 className="settings-card-title">Connections</h4>
        {!catalogueOpen && !editor ? (
          <button type="button" className="settings-add-button" onClick={openCatalogue}>
            <span aria-hidden="true">+</span> Add connection
          </button>
        ) : null}
      </div>

      {providers.length === 0 ? (
        <p className="settings-empty">
          No connections yet. Add one to run agents on a real model.
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
                  <span className="settings-badge settings-badge-active">Default</span>
                ) : null}
              </div>
              <p className="settings-card-body settings-path">{provider.base_url}</p>

              {provider.models.length === 0 ? (
                <p className="settings-hint">
                  No models yet. Open <strong>Manage models</strong> to fetch or add one.
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
                          title={isDefault ? "This is the default model" : "Use as the default model"}
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
                          {editingLimits ? "Close" : "Limits"}
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
                    ? "Show fewer"
                    : `Show all ${provider.models.length} models`}
                </button>
              ) : null}

              <div className="settings-card-foot">
                <span className="settings-meta-inline">
                  {provider.has_api_key ? "API key saved" : "No API key"}
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
                    {isManager ? "Close models" : "Manage models"}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void handleTestProvider(provider)}
                    disabled={busy !== null}
                  >
                    {busy === `test-${provider.id}` ? "Testing…" : "Test"}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => startEditProvider(provider)}
                    disabled={busy !== null}
                  >
                    Edit connection
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void handleDeleteProvider(provider)}
                    disabled={busy !== null}
                  >
                    {busy === `delete-${provider.id}` ? "Deleting…" : "Delete"}
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
              placeholder="Search providers by name or URL"
              aria-label="Search providers"
              autoComplete="off"
              autoFocus
            />
            <button
              type="button"
              className="ghost-button"
              onClick={() => setCatalogueOpen(false)}
            >
              Cancel
            </button>
          </div>

          <button
            type="button"
            className="catalogue-custom"
            onClick={() => chooseCatalogueEntry(null)}
          >
            <span className="catalogue-entry-label">Custom</span>
            <span className="catalogue-entry-hint">
              Start from a blank form and enter any compatible endpoint.
            </span>
          </button>

          {catalogueGroups.map((group) => {
            const entries = filteredCatalogue.filter(
              (entry) => entry.category === group.id,
            );
            if (entries.length === 0) return null;
            return (
              <section key={group.id} className="catalogue-group">
                <h4 className="catalogue-group-title">{group.label}</h4>
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
                          {entry.description} · {entry.baseUrl}
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
              No providers match “{catalogueQuery.trim()}”. Choose Custom to
              enter an endpoint by hand.
            </p>
          ) : null}
        </div>
      ) : null}

      {showConfigPath ? (
        <details className="settings-advanced">
          <summary>Advanced</summary>
          <p className="settings-note settings-path">
            Configuration: {modelSettings.config_path}
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
          ? `Fetched ${modelCountLabel(models.length)}. ${withWindow} reported a context window.`
          : `Fetched ${modelCountLabel(models.length)}. None reported a context window, so set it by hand under Models.`,
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
      onSaved(next, creating ? "Connection added." : "Connection updated.");
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
        {creating ? "New connection" : `Edit ${initial.name}`}
      </p>

      <div className="settings-form-row">
        <label className="field">
          <span>Name</span>
          <input
            value={draft.name}
            onChange={(event) => update({ name: event.target.value })}
            placeholder="OpenAI"
            autoComplete="off"
          />
        </label>
        <label className="field">
          <span>Base URL</span>
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
          <span>First model</span>
          <div className="model-toolbar">
            <input
              aria-label="Model name"
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
              {busy === "fetch-models" ? "Fetching…" : "Fetch models"}
            </button>
          </div>
          {fetched.length > 0 ? (
            <select
              aria-label="Fetched models"
              value={fetched.some((model) => model.id === draft.model) ? draft.model : ""}
              onChange={(event) =>
                update({
                  model: event.target.value,
                  models: mergeModels(draft.models, [event.target.value]),
                })
              }
            >
              <option value="">Select a fetched model</option>
              {fetched.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.id}
                  {model.contextWindow
                    ? ` · ${compactTokens(model.contextWindow)}`
                    : ""}
                </option>
              ))}
            </select>
          ) : null}
          <span className="settings-hint">
            One model is enough to start. Add more, and set their context
            windows, from the connection card.
          </span>
        </div>
      ) : null}

      <label className="field">
        <span>API key</span>
        <input
          type="password"
          value={draft.apiKey}
          onChange={(event) =>
            update({ apiKey: event.target.value, clearApiKey: false })
          }
          placeholder={
            hasStoredKey
              ? "Saved — leave blank to keep"
              : "Optional for local endpoints"
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
          Remove the saved API key
        </label>
      ) : null}

      <details className="settings-advanced">
        <summary>Advanced</summary>
        <label className="field">
          <span>Protocol</span>
          <select
            value={draft.protocol}
            onChange={(event) =>
              update({ protocol: event.target.value as ProviderProtocol })
            }
            disabled={busy !== null}
          >
            {protocolOptions.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
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
            ? "Saving…"
            : creating
              ? "Add connection"
              : "Save connection"}
        </button>
        <button
          type="button"
          className="ghost-button"
          onClick={() => void handleTest()}
          disabled={
            busy !== null || !draft.baseUrl.trim() || !draft.model.trim()
          }
        >
          {busy === "test" ? "Testing…" : "Test connection"}
        </button>
        <button
          type="button"
          className="ghost-button"
          onClick={onCancel}
          disabled={busy !== null}
        >
          Cancel
        </button>
      </div>

      <p className="settings-hint">
        Saving a connection never changes which model runs. Keys are stored in
        plain text in the local configuration file; protect it as sensitive
        local data.
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
      onSaved(next, `${model} limits saved.`);
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
          <span>Context window (tokens)</span>
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
          <span>Max output (tokens)</span>
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
          {busy ? "Saving…" : "Save limits"}
        </button>
        <button type="button" className="ghost-button" onClick={onClose}>
          Cancel
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
          ? `Fetched ${modelCountLabel(next.length)}. ${withWindow} reported a context window.`
          : `Fetched ${modelCountLabel(next.length)}. None reported a context window.`,
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
    setStatus(`Added ${modelCountLabel(selected.length)} to ${provider.name}.`);
  }

  function addManual() {
    const value = manual.trim();
    if (!value) return;
    if (models.includes(value)) {
      setStatus(`${value} is already known.`);
      return;
    }
    setModels([...models, value]);
    setManual("");
    setStatus(`Added ${value}.`);
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
      onSaved(next, `Models for ${provider.name} saved.`);
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="model-manager">
      <div className="model-manager-head">
        <span className="settings-card-title">Models for {provider.name}</span>
        <button type="button" className="text-button" onClick={onClose}>
          Done
        </button>
      </div>

      <div className="model-toolbar">
        <button
          type="button"
          className="ghost-button"
          onClick={() => void handleFetch()}
          disabled={busy !== null}
        >
          {busy === "fetch" ? "Fetching…" : "Fetch models"}
        </button>
        <input
          aria-label="Add a model name"
          value={manual}
          onChange={(event) => setManual(event.target.value)}
          placeholder="Add a model name"
          autoComplete="off"
        />
        <button
          type="button"
          className="ghost-button"
          onClick={addManual}
          disabled={busy !== null || !manual.trim()}
        >
          Add
        </button>
      </div>

      {fetched ? (
        <div className="model-catalogue-panel">
          <input
            aria-label="Filter fetched models"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder="Filter models"
            autoComplete="off"
          />
          <div className="model-toolbar">
            <button
              type="button"
              className="ghost-button"
              onClick={() => setSelected((fetched ?? []).map((model) => model.id))}
            >
              Select all
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={() => setSelected([])}
            >
              Clear
            </button>
            <button
              type="button"
              className="primary-button"
              onClick={addSelected}
              disabled={selected.length === 0}
            >
              Add selected ({selected.length})
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
                      : "no window reported"}
                  </span>
                  {models.includes(model.id) ? (
                    <span className="settings-meta-inline">Known</span>
                  ) : null}
                </label>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="field">
        <span>Known models ({models.length})</span>
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
                <span className="settings-meta-inline">Default</span>
              ) : (
                <button
                  type="button"
                  className="text-button"
                  onClick={() => remove(model)}
                  disabled={busy !== null}
                >
                  Remove
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
          {busy === "save" ? "Saving…" : "Save models"}
        </button>
      </div>

      <p className="settings-hint">
        Set a model's context window beside it in the connection card. The
        default model cannot be removed here; choose another default first.
      </p>
    </div>
  );
}
