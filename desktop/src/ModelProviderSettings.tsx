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

interface ProviderDraft {
  providerId: string | null;
  protocol: ProviderProtocol;
  name: string;
  baseUrl: string;
  model: string;
  models: string[];
  modelMetadata: Record<string, ModelMetadata>;
  apiKey: string;
  clearApiKey: boolean;
  /// Whether the free-text model input is shown alongside the dropdown.
  manualModel: boolean;
}

interface ModelFetchStatus {
  ok: boolean;
  message: string;
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

function modelMetadataFor(draft: ProviderDraft): ModelMetadata {
  const model = draft.model.trim();
  return model ? (draft.modelMetadata[model] ?? {}) : {};
}

function optionalTokenCount(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed <= 0) return undefined;
  return Math.floor(parsed);
}

function draftForCatalogueEntry(entry: CatalogueEntry): ProviderDraft {
  return {
    providerId: null,
    protocol: entry.protocol,
    name: entry.name,
    baseUrl: entry.baseUrl,
    model: entry.model,
    models: entry.model ? [entry.model] : [],
    modelMetadata: {},
    apiKey: "",
    clearApiKey: false,
    manualModel: !entry.model,
  };
}

function blankDraft(): ProviderDraft {
  return {
    providerId: null,
    protocol: "openai",
    name: "",
    baseUrl: "",
    model: "",
    models: [],
    modelMetadata: {},
    apiKey: "",
    clearApiKey: false,
    manualModel: true,
  };
}

function draftForProvider(provider: ModelProvider): ProviderDraft {
  return {
    providerId: provider.id,
    protocol: provider.protocol,
    name: provider.name,
    baseUrl: provider.base_url,
    model: provider.model,
    models: mergeModels(provider.models, [provider.model]),
    modelMetadata: { ...(provider.model_metadata ?? {}) },
    apiKey: "",
    clearApiKey: false,
    manualModel: false,
  };
}

interface ModelProviderSettingsProps {
  /// Hide the configuration file path when embedded in a guided flow.
  showConfigPath?: boolean;
}

function modelCountLabel(count: number): string {
  return count === 1 ? "1 model" : `${count} models`;
}

export default function ModelProviderSettings({
  showConfigPath = true,
}: ModelProviderSettingsProps) {
  const [modelSettings, setModelSettings] = useState<ModelSettings | null>(null);
  const [draft, setDraft] = useState<ProviderDraft | null>(null);
  const [catalogueOpen, setCatalogueOpen] = useState(false);
  const [catalogueQuery, setCatalogueQuery] = useState("");
  const [modelBusy, setModelBusy] = useState<string | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  const [modelNotice, setModelNotice] = useState<string | null>(null);
  const [fetchStatus, setFetchStatus] = useState<ModelFetchStatus | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [modelsOpen, setModelsOpen] = useState(false);
  const [modelFilter, setModelFilter] = useState("");

  useEffect(() => {
    let cancelled = false;

    getModelSettings()
      .then((settings) => {
        if (!cancelled) setModelSettings(settings);
      })
      .catch((error) => {
        if (!cancelled) setModelError(errorMessage(error));
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const activeProvider =
    modelSettings?.providers.find((provider) => provider.active) ?? null;
  const hasStoredKey = draft?.providerId
    ? (modelSettings?.providers.find((provider) => provider.id === draft.providerId)
        ?.has_api_key ?? false)
    : false;

  function updateDraft(patch: Partial<ProviderDraft>) {
    setDraft((current) => (current ? { ...current, ...patch } : current));
    if (
      patch.protocol !== undefined ||
      patch.baseUrl !== undefined ||
      patch.apiKey !== undefined
    ) {
      setFetchStatus(null);
      setFetchedModels([]);
      setSelectedModels([]);
      setModelsOpen(false);
    }
  }

  function updateModelMetadata(patch: Partial<ModelMetadata>) {
    if (!draft) return;
    const model = draft.model.trim();
    if (!model) return;

    const next: ModelMetadata = { ...(draft.modelMetadata[model] ?? {}) };
    if ("context_window" in patch) {
      if (patch.context_window === undefined) delete next.context_window;
      else next.context_window = patch.context_window;
    }
    if ("max_output_tokens" in patch) {
      if (patch.max_output_tokens === undefined) delete next.max_output_tokens;
      else next.max_output_tokens = patch.max_output_tokens;
    }

    const metadata = { ...draft.modelMetadata };
    if (Object.keys(next).length === 0) delete metadata[model];
    else metadata[model] = next;
    updateDraft({ modelMetadata: metadata });
  }

  function resetTransientState() {
    setModelError(null);
    setModelNotice(null);
    setFetchStatus(null);
    setFetchedModels([]);
    setSelectedModels([]);
    setModelsOpen(false);
    setModelFilter("");
  }

  function openCatalogue() {
    setDraft(null);
    setCatalogueOpen(true);
    setCatalogueQuery("");
    resetTransientState();
  }

  function closeCatalogue() {
    setCatalogueOpen(false);
    setCatalogueQuery("");
  }

  function chooseCatalogueEntry(entry: CatalogueEntry | null) {
    setDraft(entry ? draftForCatalogueEntry(entry) : blankDraft());
    setCatalogueOpen(false);
    resetTransientState();
  }

  function startEditProvider(provider: ModelProvider) {
    setDraft(draftForProvider(provider));
    setCatalogueOpen(false);
    resetTransientState();
  }

  function changeProtocol(protocol: ProviderProtocol) {
    // Keep the name, base URL, model list, and active model the user has
    // entered; the protocol only changes how the endpoint is called.
    updateDraft({ protocol });
  }

  async function handleSaveProvider(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft) return;

    setModelBusy("save-provider");
    setModelError(null);
    setModelNotice(null);
    try {
      const next = await saveModelProvider({
        providerId: draft.providerId,
        protocol: draft.protocol,
        name: draft.name,
        baseUrl: draft.baseUrl,
        models: mergeModels(draft.models, [draft.model]),
        model: draft.model,
        modelMetadata: draft.modelMetadata,
        apiKey: draft.apiKey,
        clearApiKey: draft.clearApiKey,
      });
      setModelSettings(next);
      setDraft(null);
      setModelNotice(draft.providerId ? "Provider updated." : "Provider added.");
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  async function handleFetchModels() {
    if (!draft) return;

    setModelBusy("fetch-models");
    setFetchStatus(null);
    try {
      const models = await fetchProviderModels({
        providerId: draft.providerId,
        protocol: draft.protocol,
        baseUrl: draft.baseUrl,
        apiKey: draft.apiKey,
      });
      setDraft((current) => {
        if (!current) return current;
        const active = current.model.trim() ? current.model : (models[0] ?? "");
        return {
          ...current,
          model: active,
          models: mergeModels(current.models, active ? [active] : []),
        };
      });
      setFetchedModels(models);
      setSelectedModels(models);
      setModelsOpen(true);
      setFetchStatus({
        ok: true,
        message: `Fetched ${modelCountLabel(models.length)} from the endpoint. Select models to add.`,
      });
    } catch (error) {
      setFetchStatus({ ok: false, message: errorMessage(error) });
    } finally {
      setModelBusy(null);
    }
  }

  function toggleFetchedModel(model: string) {
    setSelectedModels((current) =>
      current.includes(model)
        ? current.filter((candidate) => candidate !== model)
        : [...current, model],
    );
  }

  function addSelectedModels() {
    if (!draft) return;
    updateDraft({
      models: mergeModels(draft.models, selectedModels),
      model: draft.model.trim() ? draft.model : (selectedModels[0] ?? ""),
    });
    setFetchStatus({
      ok: true,
      message: `Added ${modelCountLabel(selectedModels.length)} to this provider.`,
    });
  }

  function removeKnownModel(model: string) {
    if (!draft || model === draft.model) return;
    updateDraft({
      models: draft.models.filter((candidate) => candidate !== model),
    });
  }

  async function handleSelectModel(provider: ModelProvider, model: string) {
    setModelBusy(`model-${provider.id}`);
    setModelError(null);
    setModelNotice(null);
    try {
      const next = await setActiveModel(provider.id, model);
      setModelSettings(next);
      setModelNotice(`Active model set to ${model}.`);
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  async function handleTestProvider() {
    if (!draft) return;

    setModelBusy("test-provider");
    setModelError(null);
    setModelNotice(null);
    try {
      const message = await testModelProvider({
        providerId: draft.providerId,
        protocol: draft.protocol,
        baseUrl: draft.baseUrl,
        model: draft.model,
        apiKey: draft.apiKey,
      });
      setModelNotice(message);
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  async function handleTestSavedProvider(provider: ModelProvider) {
    setModelBusy(`test-${provider.id}`);
    setModelError(null);
    setModelNotice(null);
    try {
      const message = await testModelProvider({
        providerId: provider.id,
        protocol: provider.protocol,
        baseUrl: provider.base_url,
        model: provider.model,
        apiKey: "",
      });
      setModelNotice(message);
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  async function handleActivateProvider(providerId: string | null) {
    setModelBusy(providerId ? `activate-${providerId}` : "activate-mock");
    setModelError(null);
    setModelNotice(null);
    try {
      const next = await activateModelProvider(providerId);
      setModelSettings(next);
      setModelNotice(
        providerId ? "Provider activated." : "Switched to the offline mock model.",
      );
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  async function handleDeleteProvider(provider: ModelProvider) {
    if (!window.confirm(`Delete provider "${provider.name}"?`)) return;

    setModelBusy(`delete-${provider.id}`);
    setModelError(null);
    setModelNotice(null);
    try {
      const next = await deleteModelProvider(provider.id);
      setModelSettings(next);
      if (draft?.providerId === provider.id) setDraft(null);
      setModelNotice("Provider deleted.");
    } catch (error) {
      setModelError(errorMessage(error));
    } finally {
      setModelBusy(null);
    }
  }

  const status =
    modelError || modelNotice ? (
      <>
        {modelError ? (
          <p className="settings-status is-error" role="alert">
            {modelError}
          </p>
        ) : null}
        {modelNotice ? (
          <p className="settings-status is-ok" role="status">
            {modelNotice}
          </p>
        ) : null}
      </>
    ) : null;

  if (!modelSettings) {
    return <p className="settings-empty">Loading model settings…</p>;
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
  const modelOptions = draft
    ? mergeModels(draft.models, [draft.model])
    : [];
  const filteredFetched = fetchedModels.filter((model) =>
    model.toLowerCase().includes(modelFilter.trim().toLowerCase()),
  );

  return (
    <>
      <div className="settings-active">
        <span className="settings-badge settings-badge-active">Active</span>
        <div className="settings-active-body">
          <p className="settings-card-title">{modelSettings.active_label}</p>
          <p className="settings-card-subtitle">
            {activeProvider
              ? `${protocolLabel(activeProvider.protocol)} · ${activeProvider.model} · ${activeProvider.base_url}`
              : "No provider selected. Runs use the offline mock model."}
          </p>
        </div>
        {activeProvider ? (
          <button
            type="button"
            className="ghost-button"
            onClick={() => handleActivateProvider(null)}
            disabled={modelBusy !== null}
          >
            Use offline mock
          </button>
        ) : null}
      </div>

      {modelSettings.providers.length === 0 ? (
        <p className="settings-empty">
          No providers yet. Add one to connect a real model.
        </p>
      ) : (
        <ul className="settings-list">
          {modelSettings.providers.map((provider) => (
            <li key={provider.id} className="settings-card">
              <div className="settings-card-head">
                <span className="settings-card-title">{provider.name}</span>
                <span className="settings-badge">
                  {protocolLabel(provider.protocol)}
                </span>
                {provider.active ? (
                  <span className="settings-badge settings-badge-active">Active</span>
                ) : null}
              </div>
              <p className="settings-card-body">
                {provider.model} · {modelCountLabel(provider.models.length)} ·{" "}
                {provider.base_url}
              </p>
              {provider.models.length > 1 ? (
                <details className="provider-model-list">
                  <summary>Show models</summary>
                  <ul>
                    {provider.models.map((model) => (
                      <li key={model}>
                        <button
                          type="button"
                          className={
                            model === provider.model ? "is-active" : undefined
                          }
                          aria-current={
                            model === provider.model ? "true" : undefined
                          }
                          onClick={() => void handleSelectModel(provider, model)}
                          disabled={
                            modelBusy !== null || model === provider.model
                          }
                        >
                          {model === provider.model ? "✓ " : ""}
                          {model}
                        </button>
                      </li>
                    ))}
                  </ul>
                </details>
              ) : null}
              <div className="settings-card-foot">
                <span className="settings-meta-inline">
                  {provider.has_api_key ? "API key saved" : "No API key"}
                </span>
                <div className="settings-card-actions">
                  {!provider.active ? (
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => handleActivateProvider(provider.id)}
                      disabled={modelBusy !== null}
                    >
                      Use
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => handleTestSavedProvider(provider)}
                    disabled={modelBusy !== null}
                  >
                    {modelBusy === `test-${provider.id}` ? "Testing…" : "Test"}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => startEditProvider(provider)}
                    disabled={modelBusy !== null}
                  >
                    Edit
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => handleDeleteProvider(provider)}
                    disabled={modelBusy !== null}
                  >
                    Delete
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      {draft ? (
        <form className="settings-form provider-editor" onSubmit={handleSaveProvider}>
          <p className="settings-card-title">
            {draft.providerId ? "Edit provider" : "New provider"}
          </p>

          <div className="settings-form-row">
            <label className="field">
              <span>Name</span>
              <input
                value={draft.name}
                onChange={(event) => updateDraft({ name: event.target.value })}
                placeholder="OpenAI"
                autoComplete="off"
              />
            </label>
            <label className="field">
              <span>Protocol</span>
              <select
                value={draft.protocol}
                onChange={(event) =>
                  changeProtocol(event.target.value as ProviderProtocol)
                }
                disabled={modelBusy !== null}
              >
                {protocolOptions.map((option) => (
                  <option key={option.id} value={option.id}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <p className="settings-hint">{protocolDescription(draft.protocol)}</p>

          <label className="field">
            <span>Base URL</span>
            <input
              value={draft.baseUrl}
              onChange={(event) => updateDraft({ baseUrl: event.target.value })}
              placeholder={
                draft.protocol === "anthropic"
                  ? "https://api.anthropic.com"
                  : "https://api.openai.com/v1"
              }
              autoComplete="off"
            />
          </label>

          <div className="field">
            <span>Models</span>
            <div className="model-toolbar">
              <select
                aria-label="Active model"
                value={draft.model}
                onChange={(event) => updateDraft({ model: event.target.value })}
                disabled={modelBusy !== null}
              >
                {draft.model === "" ? (
                  <option value="">Select a model</option>
                ) : null}
                {modelOptions.map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="ghost-button"
                onClick={handleFetchModels}
                disabled={modelBusy !== null || !draft.baseUrl.trim()}
              >
                {modelBusy === "fetch-models" ? "Fetching…" : "Fetch models"}
              </button>
              <button
                type="button"
                className="ghost-button"
                onClick={() =>
                  updateDraft({ manualModel: !draft.manualModel })
                }
                disabled={modelBusy !== null}
              >
                {draft.manualModel ? "Hide manual entry" : "Add manually"}
              </button>
            </div>
            {draft.manualModel ? (
              <input
                aria-label="Model name"
                value={draft.model}
                onChange={(event) => updateDraft({ model: event.target.value })}
                placeholder="gpt-4o"
                autoComplete="off"
              />
            ) : null}
            {fetchStatus ? (
              <p
                className={`model-status ${fetchStatus.ok ? "is-ok" : "is-error"}`}
                role={fetchStatus.ok ? "status" : "alert"}
              >
                {fetchStatus.message}
              </p>
            ) : null}
            <span className="settings-hint">
              {draft.models.length > 0
                ? `${modelCountLabel(draft.models.length)} known for this provider. Fetch again to refresh, or add a name by hand.`
                : "Fetch the endpoint's model list, or add a model name by hand."}
            </span>
          </div>

          {draft.models.length > 0 ? (
            <div className="field">
              <span>Known models ({draft.models.length})</span>
              <ul className="known-model-list">
                {draft.models.map((model) => (
                  <li
                    key={model}
                    className={model === draft.model ? "is-active" : undefined}
                  >
                    <span>
                      {model === draft.model ? "✓ " : ""}
                      {model}
                    </span>
                    {model === draft.model ? (
                      <span className="settings-meta-inline">Active</span>
                    ) : (
                      <button
                        type="button"
                        className="text-button"
                        onClick={() => removeKnownModel(model)}
                        disabled={modelBusy !== null}
                      >
                        Remove
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {fetchedModels.length > 0 ? (
            <div className="field">
              <button
                type="button"
                className="model-catalogue-toggle"
                aria-expanded={modelsOpen}
                onClick={() => setModelsOpen((current) => !current)}
              >
                <span>Fetched models ({fetchedModels.length})</span>
                <span>{modelsOpen ? "Hide" : "Show"}</span>
              </button>
              {modelsOpen ? (
                <div className="model-catalogue-panel">
                  <input
                    aria-label="Filter fetched models"
                    value={modelFilter}
                    onChange={(event) => setModelFilter(event.target.value)}
                    placeholder="Filter models"
                    autoComplete="off"
                  />
                  <div className="model-toolbar">
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => setSelectedModels(fetchedModels)}
                    >
                      Select all
                    </button>
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={() => setSelectedModels([])}
                    >
                      Clear
                    </button>
                    <button
                      type="button"
                      className="primary-button"
                      onClick={addSelectedModels}
                      disabled={selectedModels.length === 0}
                    >
                      Add selected ({selectedModels.length})
                    </button>
                  </div>
                  <ul className="model-catalogue-list">
                    {filteredFetched.map((model) => (
                      <li key={model}>
                        <label>
                          <input
                            type="checkbox"
                            checked={selectedModels.includes(model)}
                            onChange={() => toggleFetchedModel(model)}
                          />
                          <span>{model}</span>
                          {draft.models.includes(model) ? (
                            <span className="settings-meta-inline">Known</span>
                          ) : null}
                        </label>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
            </div>
          ) : null}

          <div className="field">
            <span>Context window</span>
            <div className="settings-form-row">
              <label className="field">
                <span>Window (tokens)</span>
                <input
                  type="number"
                  min={1}
                  inputMode="numeric"
                  value={modelMetadataFor(draft).context_window ?? ""}
                  onChange={(event) =>
                    updateModelMetadata({
                      context_window: optionalTokenCount(event.target.value),
                    })
                  }
                  placeholder="128000"
                  disabled={modelBusy !== null || !draft.model.trim()}
                />
              </label>
              <label className="field">
                <span>Max output (tokens)</span>
                <input
                  type="number"
                  min={1}
                  inputMode="numeric"
                  value={modelMetadataFor(draft).max_output_tokens ?? ""}
                  onChange={(event) =>
                    updateModelMetadata({
                      max_output_tokens: optionalTokenCount(event.target.value),
                    })
                  }
                  placeholder="8192"
                  disabled={modelBusy !== null || !draft.model.trim()}
                />
              </label>
            </div>
            <span className="settings-hint">
              Optional. Leave blank when unknown; CowBot will not show a context
              percentage for a model without a window.
            </span>
          </div>

          <label className="field">
            <span>API key</span>
            <input
              type="password"
              value={draft.apiKey}
              onChange={(event) =>
                updateDraft({
                  apiKey: event.target.value,
                  clearApiKey: false,
                })
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
                  updateDraft({
                    clearApiKey: event.target.checked,
                    apiKey: event.target.checked ? "" : draft.apiKey,
                  })
                }
              />
              Remove the saved API key
            </label>
          ) : null}

          <div className="form-actions">
            <button
              type="submit"
              className="primary-button"
              disabled={
                modelBusy !== null ||
                !draft.name.trim() ||
                !draft.baseUrl.trim() ||
                !draft.model.trim()
              }
            >
              {modelBusy === "save-provider" ? "Saving…" : "Save provider"}
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={handleTestProvider}
              disabled={
                modelBusy !== null ||
                !draft.baseUrl.trim() ||
                !draft.model.trim()
              }
            >
              {modelBusy === "test-provider" ? "Testing…" : "Test connection"}
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={() => setDraft(null)}
              disabled={modelBusy !== null}
            >
              Cancel
            </button>
          </div>

          {status}

          <p className="settings-hint">
            Keys are stored in plain text in the local configuration file;
            protect it as sensitive local data.
          </p>
        </form>
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
              onClick={closeCatalogue}
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
      ) : (
        <>
          <button type="button" className="settings-add-button" onClick={openCatalogue}>
            <span aria-hidden="true">+</span> Add provider
          </button>
          {status}
        </>
      )}

      {showConfigPath ? (
        <p className="settings-note">Configuration: {modelSettings.config_path}</p>
      ) : null}
    </>
  );
}
