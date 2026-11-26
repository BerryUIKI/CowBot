import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

import BotTag from "./BotTag";
import ModelProviderSettings from "./ModelProviderSettings";
import MemorySettings from "./MemorySettings";
import { PromptSnapshotAuditPanel } from "./PromptSnapshotViewer";
import IntegrationSettings from "./IntegrationSettings";
import ServerSettings from "./ServerSettings";
import UpdateProgress from "./UpdateProgress";
import { errorMessage } from "./errors";
import { formatDateTime } from "./format";
import {
  currentIntlLocale,
  localeDefinitions,
  useI18n,
  type TranslationKey,
} from "./i18n";
import {
  clearSessionDecisions,
  createPermissionRule,
  createWorkspace,
  deletePermissionRule,
  exportWorkspace,
  getAuditEvents,
  getCloseToTray,
  getSessionDecisions,
  getWorkspacePolicy,
  importWorkspace,
  listPermissionRules,
  listWorkspaces,
  setBypassCritical as saveBypassCritical,
  setCloseToTray as saveCloseToTray,
  setPermissionRuleEnabled,
  switchWorkspace,
  updateUserProfile,
} from "./api";
import type {
  AgentProfile,
  AuditEvent,
  BypassCritical,
  ClientSession,
  PermissionRule,
  Project,
  RuleDecision,
  SessionDecisionEntry,
  TemplateDocument,
  WorkspaceSummary,
} from "./types";
import type { Updater } from "./updater";
import { useUiPreferences } from "./uiPreferences";

interface SettingsViewProps {
  session: ClientSession | null;
  agents: AgentProfile[];
  templates: TemplateDocument[];
  templateId: string;
  busy: string | null;
  updater: Updater;
  tasksRunning: boolean;
  section: SettingsSection;
  projects: Project[];
  /** False in server mode, where local worktrees and packages do not apply. */
  localRuntime: boolean;
  onSelectTemplate: (templateId: string) => void;
  onAddAgents: () => void;
  onEditAgent: (agent: AgentProfile) => void;
  onSessionChange: (session: ClientSession) => void;
  onAddProject: (path: string) => Promise<void>;
  onRemoveProject: (project: Project) => Promise<void>;
  onOpenLanguage: () => void;
}

export type SettingsSection =
  | "general"
  | "model"
  | "team"
  | "memory"
  | "approvals"
  | "audit"
  | "projects"
  | "integrations"
  | "workspace"
  | "server"
  | "about";

export const settingsSections: {
  id: SettingsSection;
  labelKey: TranslationKey;
}[] = [
  { id: "general", labelKey: "settings.nav.general" },
  { id: "model", labelKey: "settings.nav.model" },
  { id: "team", labelKey: "settings.nav.team" },
  { id: "memory", labelKey: "settings.nav.memory" },
  { id: "approvals", labelKey: "settings.nav.approvals" },
  { id: "audit", labelKey: "settings.nav.audit" },
  { id: "projects", labelKey: "settings.nav.projects" },
  { id: "integrations", labelKey: "settings.nav.integrations" },
  { id: "workspace", labelKey: "settings.nav.workspace" },
  { id: "server", labelKey: "settings.nav.server" },
  { id: "about", labelKey: "settings.nav.about" },
];

function agentInitials(name: string): string {
  return name
    .split(/[\s-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? "")
    .join("");
}

function packageFileName(name: string | undefined): string {
  const base =
    (name ?? "workspace").trim().replace(/[\\/:*?"<>|]+/g, "-") || "workspace";
  return base.toLowerCase().endsWith(".cowpkg") ? base : `${base}.cowpkg`;
}

function releaseUrl(version: string): string {
  return `https://github.com/BerryUIKI/CowBot/releases/tag/v${version.replace(/^v/, "")}`;
}

function releaseDate(value: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "—"
    : date.toLocaleDateString(currentIntlLocale(), {
        year: "numeric",
        month: "short",
        day: "numeric",
      });
}

export default function SettingsView({
  session,
  agents,
  templates,
  templateId,
  busy,
  updater,
  tasksRunning,
  section,
  projects,
  localRuntime,
  onSelectTemplate,
  onAddAgents,
  onEditAgent,
  onSessionChange,
  onAddProject,
  onRemoveProject,
  onOpenLanguage,
}: SettingsViewProps) {
  const { t, locale } = useI18n();
  const { showAgentBotTags, setShowAgentBotTags } = useUiPreferences();
  const [botTagError, setBotTagError] = useState<string | null>(null);
  const [closeToTray, setCloseToTray] = useState<boolean | null>(null);
  const [closeToTrayError, setCloseToTrayError] = useState<string | null>(null);
  const [workspaces, setWorkspaces] = useState<WorkspaceSummary[]>([]);
  const [workspaceBusy, setWorkspaceBusy] = useState<string | null>(null);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [workspaceNotice, setWorkspaceNotice] = useState<string | null>(null);
  const [newWorkspaceName, setNewWorkspaceName] = useState("");
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [editingProfile, setEditingProfile] = useState(false);
  const [profileName, setProfileName] = useState("");
  const [profileEmail, setProfileEmail] = useState("");
  const [profileBusy, setProfileBusy] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const [sessionDecisions, setSessionDecisions] = useState<
    SessionDecisionEntry[]
  >([]);
  const [sessionBusy, setSessionBusy] = useState(false);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [rules, setRules] = useState<PermissionRule[]>([]);
  const [ruleTool, setRuleTool] = useState("execute_code");
  const [ruleCommand, setRuleCommand] = useState("");
  const [ruleDecision, setRuleDecision] = useState<RuleDecision>("ask");
  const [ruleBusy, setRuleBusy] = useState(false);
  const [ruleError, setRuleError] = useState<string | null>(null);
  const [bypassCritical, setBypassCritical] = useState<BypassCritical>("allow");
  const [policyBusy, setPolicyBusy] = useState(false);
  const [policyError, setPolicyError] = useState<string | null>(null);
  const [auditEvents, setAuditEvents] = useState<AuditEvent[]>([]);
  const [auditSource, setAuditSource] = useState("all");
  const [auditBusy, setAuditBusy] = useState(false);
  const [auditError, setAuditError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    listWorkspaces()
      .then((next) => {
        if (!cancelled) setWorkspaces(next);
      })
      .catch((error) => {
        if (!cancelled) setWorkspaceError(errorMessage(error));
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (section !== "general") return;
    let cancelled = false;
    getCloseToTray()
      .then((stored) => {
        if (!cancelled) setCloseToTray(stored);
      })
      .catch((error) => {
        if (!cancelled) setCloseToTrayError(errorMessage(error));
      });
    return () => {
      cancelled = true;
    };
  }, [section]);

  useEffect(() => {
    if (section !== "approvals") return;
    let cancelled = false;
    setSessionError(null);
    // Session decisions are per process, so a server workspace has none and
    // the client refuses the call by design.
    if (!localRuntime) {
      setSessionDecisions([]);
      return;
    }
    getSessionDecisions()
      .then((entries) => {
        if (!cancelled) setSessionDecisions(entries);
      })
      .catch((error) => {
        if (!cancelled) setSessionError(errorMessage(error));
      });
    return () => {
      cancelled = true;
    };
  }, [section, localRuntime]);

  useEffect(() => {
    if (section !== "approvals") return;
    let cancelled = false;
    setRuleError(null);
    listPermissionRules()
      .then((next) => {
        if (!cancelled) setRules(next);
      })
      .catch((error) => {
        if (!cancelled) setRuleError(errorMessage(error));
      });
    return () => {
      cancelled = true;
    };
  }, [section]);

  useEffect(() => {
    if (section !== "approvals") return;
    let cancelled = false;
    setPolicyError(null);
    getWorkspacePolicy()
      .then((policy) => {
        if (!cancelled) setBypassCritical(policy.bypass_critical);
      })
      .catch((error) => {
        if (!cancelled) setPolicyError(errorMessage(error));
      });
    return () => {
      cancelled = true;
    };
  }, [section]);

  useEffect(() => {
    if (section !== "audit") return;
    let cancelled = false;
    setAuditBusy(true);
    setAuditError(null);
    getAuditEvents(200, auditSource === "all" ? null : auditSource)
      .then((events) => {
        if (!cancelled) setAuditEvents(events);
      })
      .catch((error) => {
        if (!cancelled) setAuditError(errorMessage(error));
      })
      .finally(() => {
        if (!cancelled) setAuditBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [section, auditSource]);

  async function handleClearSessionDecisions() {
    setSessionBusy(true);
    setSessionError(null);
    try {
      await clearSessionDecisions();
      setSessionDecisions([]);
    } catch (error) {
      setSessionError(errorMessage(error));
    } finally {
      setSessionBusy(false);
    }
  }

  async function handleBypassCritical(value: BypassCritical) {
    setPolicyBusy(true);
    setPolicyError(null);
    try {
      const policy = await saveBypassCritical(value);
      setBypassCritical(policy.bypass_critical);
    } catch (error) {
      setPolicyError(errorMessage(error));
    } finally {
      setPolicyBusy(false);
    }
  }

  async function handleCreateRule(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const tool = ruleTool.trim();
    if (!tool) return;
    setRuleBusy(true);
    setRuleError(null);
    try {
      const rule = await createPermissionRule(
        tool,
        ruleCommand.trim() || null,
        ruleDecision,
      );
      setRules((current) => [...current, rule]);
      setRuleCommand("");
    } catch (error) {
      setRuleError(errorMessage(error));
    } finally {
      setRuleBusy(false);
    }
  }

  async function handleToggleRule(rule: PermissionRule) {
    setRuleBusy(true);
    setRuleError(null);
    try {
      const updated = await setPermissionRuleEnabled(rule.id, !rule.enabled);
      setRules((current) =>
        current.map((candidate) =>
          candidate.id === updated.id ? updated : candidate,
        ),
      );
    } catch (error) {
      setRuleError(errorMessage(error));
    } finally {
      setRuleBusy(false);
    }
  }

  async function handleDeleteRule(rule: PermissionRule) {
    const label = rule.command ? `${rule.tool}: ${rule.command}` : rule.tool;
    if (!window.confirm(t("settings.approvals.deleteConfirm", { label }))) return;
    setRuleBusy(true);
    setRuleError(null);
    try {
      await deletePermissionRule(rule.id);
      setRules((current) => current.filter((candidate) => candidate.id !== rule.id));
    } catch (error) {
      setRuleError(errorMessage(error));
    } finally {
      setRuleBusy(false);
    }
  }

  async function handleSwitchWorkspace(database: string) {    setWorkspaceBusy(`switch-${database}`);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      await switchWorkspace(database);
      // The backend restarts the app into the selected database.
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      setWorkspaceBusy(null);
    }
  }

  async function handleExportWorkspace() {
    setWorkspaceError(null);
    setWorkspaceNotice(null);

    let target: string | null = null;
    try {
      target = await save({
        title: t("settings.workspace.exportTitle"),
        defaultPath: packageFileName(session?.workspace.name),
        filters: [
          {
            name: t("settings.workspace.packageFilter"),
            extensions: ["cowpkg"],
          },
        ],
      });
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      return;
    }
    if (!target) return;

    setWorkspaceBusy("export");
    try {
      const written = await exportWorkspace(target);
      setWorkspaceNotice(
        t("settings.workspace.exportedTo", { path: written }),
      );
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(null);
    }
  }

  async function handleCreateWorkspace() {
    const name = newWorkspaceName.trim();
    if (!name) return;

    setWorkspaceBusy("create");
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      await createWorkspace(name);
      // The backend registers the workspace and restarts the app.
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      setWorkspaceBusy(null);
    }
  }

  async function handleImportWorkspace() {
    setWorkspaceError(null);
    setWorkspaceNotice(null);

    let selected: string | null = null;
    try {
      selected = await open({
        title: t("settings.workspace.importTitle"),
        multiple: false,
        directory: false,
        filters: [
          {
            name: t("settings.workspace.packageFilter"),
            extensions: ["cowpkg"],
          },
        ],
      });
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      return;
    }
    if (typeof selected !== "string") return;

    setWorkspaceBusy("import");
    try {
      await importWorkspace(selected);
      // The backend registers the workspace and restarts the app.
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      setWorkspaceBusy(null);
    }
  }

  async function handleAddProject() {
    const selection = await open({
      directory: true,
      multiple: false,
      title: t("settings.projects.registerTitle"),
    });
    if (typeof selection === "string" && selection.trim()) {
      await onAddProject(selection);
    }
  }

  function handleRemoveProject(project: Project) {
    if (
      window.confirm(
        t("settings.projects.removeConfirm", { name: project.name }),
      )
    ) {
      void onRemoveProject(project);
    }
  }

  function startEditProfile() {
    setProfileName(session?.user.name ?? "");
    setProfileEmail(session?.user.email ?? "");
    setProfileError(null);
    setEditingProfile(true);
  }

  function decisionLabel(decision: string): string {
    switch (decision) {
      case "allow":
        return t("settings.approvals.allow");
      case "ask":
        return t("settings.approvals.ask");
      case "deny":
        return t("settings.approvals.deny");
      default:
        return decision;
    }
  }

  function auditSourceLabel(source: string | null | undefined): string {
    switch (source) {
      case "rule":
        return t("settings.audit.rule");
      case "model":
        return t("settings.audit.model");
      case "session":
        return t("settings.audit.session");
      case "human":
        return t("settings.audit.human");
      case "policy":
        return t("settings.audit.policy");
      case "system":
        return t("settings.audit.system");
      default:
        return source ?? t("settings.audit.unknown");
    }
  }

  async function handleProfileSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setProfileBusy(true);
    setProfileError(null);
    try {
      const updated = await updateUserProfile(profileName, profileEmail);
      onSessionChange(updated);
      setEditingProfile(false);
    } catch (error) {
      setProfileError(errorMessage(error));
    } finally {
      setProfileBusy(false);
    }
  }

  return (
    <div className="settings-view">
      <header className="view-header">
        <h2 className="view-title">{t("settings.header.title")}</h2>
        <p className="view-subtitle">
          {t("settings.header.subtitle")}
        </p>
      </header>

      <div className="settings-layout">
        <div className="settings-content">
          {section === "general" ? (
            <section className="settings-section" aria-labelledby="settings-general">
              <div className="settings-section-head">
                <h3 id="settings-general">{t("settings.general.title")}</h3>
                <p className="settings-hint">{t("settings.general.hint")}</p>
              </div>
              <div className="settings-language">
                <div>
                  <span className="settings-card-title">
                    {t("language.title")}
                  </span>
                  <p className="settings-hint">{t("language.description")}</p>
                </div>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={onOpenLanguage}
                >
                  {localeDefinitions[locale].shortName}
                </button>
              </div>
              <div className="settings-language">
                <div>
                  <span className="settings-card-title">
                    {t("settings.general.botTagsTitle")}
                  </span>
                  <p className="settings-hint">
                    {t("settings.general.botTagsHint")}
                  </p>
                </div>
                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={showAgentBotTags}
                    onChange={(event) => {
                      setBotTagError(null);
                      setShowAgentBotTags(event.target.checked).catch((error) =>
                        setBotTagError(errorMessage(error)),
                      );
                    }}
                  />
                  {t("settings.general.botTagsToggle")}
                </label>
              </div>
              {botTagError ? (
                <p className="settings-status is-error" role="alert">
                  {botTagError}
                </p>
              ) : null}
              <div className="settings-language">
                <div>
                  <span className="settings-card-title">
                    {t("settings.general.closeToTrayTitle")}
                  </span>
                  <p className="settings-hint">
                    {t("settings.general.closeToTrayHint")}
                  </p>
                </div>
                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={closeToTray ?? true}
                    disabled={closeToTray === null}
                    onChange={(event) => {
                      const enabled = event.target.checked;
                      setCloseToTrayError(null);
                      setCloseToTray(enabled);
                      saveCloseToTray(enabled).catch((error) => {
                        setCloseToTray(!enabled);
                        setCloseToTrayError(errorMessage(error));
                      });
                    }}
                  />
                  {t("settings.general.closeToTrayToggle")}
                </label>
              </div>
              {closeToTrayError ? (
                <p className="settings-status is-error" role="alert">
                  {closeToTrayError}
                </p>
              ) : null}
            </section>
          ) : null}

          {section === "model" ? (
            <section className="settings-section" aria-labelledby="settings-model">
              <div className="settings-section-head">
                <h3 id="settings-model">{t("settings.models.title")}</h3>
                <p className="settings-hint">
                  {t("settings.models.hint")}
                </p>
              </div>
              <ModelProviderSettings />
            </section>
          ) : null}

          {section === "team" ? (
            <>
              <section className="settings-section" aria-labelledby="settings-templates">
                <div className="settings-section-head">
                  <h3 id="settings-templates">{t("settings.team.templatesTitle")}</h3>
                  <p className="settings-hint">
                    {t("settings.team.templatesHint")}
                  </p>
                </div>
                {templates.length === 0 ? (
                  <p className="settings-empty">{t("settings.team.noTemplates")}</p>
                ) : (
                  <ul className="settings-list">
                    {templates.map((template) => (
                      <li key={template.id} className="settings-card">
                        <div className="settings-card-head">
                          <span className="settings-card-title">{template.name}</span>
                          <span className="settings-badge">{template.kind}</span>
                        </div>
                        {template.description ? (
                          <p className="settings-card-body">{template.description}</p>
                        ) : null}
                        <div className="settings-card-foot">
                          <span className="settings-meta-inline">
                            {t(
                              template.agents.length === 1
                                ? "settings.team.agentCountOne"
                                : "settings.team.agentCountOther",
                              { count: template.agents.length },
                            )}
                          </span>
                          <button
                            type="button"
                            className="ghost-button"
                            onClick={() => {
                              onSelectTemplate(template.id);
                              onAddAgents();
                            }}
                            disabled={busy === "add-agents"}
                          >
                            {busy === "add-agents" && templateId === template.id
                              ? t("settings.team.adding")
                              : t("settings.team.addAgents")}
                          </button>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <section className="settings-section" aria-labelledby="settings-agents">
                <div className="settings-section-head">
                  <h3 id="settings-agents">{t("settings.team.agentsTitle")}</h3>
                  <p className="settings-hint">
                    {agents.length === 0
                      ? t("settings.team.agentsEmpty")
                      : t(
                          agents.length === 1
                            ? "settings.team.agentsCountOne"
                            : "settings.team.agentsCountOther",
                          { count: agents.length },
                        )}
                  </p>
                </div>
                {agents.length === 0 ? (
                  <p className="settings-empty">{t("settings.team.noAgents")}</p>
                ) : (
                  <ul className="settings-list">
                    {agents.map((agent) => (
                      <li key={agent.id} className="settings-card">
                        <div className="settings-card-head">
                          <span className="agent-avatar" aria-hidden="true">
                            {agentInitials(agent.name)}
                          </span>
                          <div>
                            <span className="settings-card-title">
                              {agent.name}
                              <BotTag />
                            </span>
                            <p className="settings-card-subtitle">{agent.role}</p>
                          </div>
                          <button
                            type="button"
                            className="text-button settings-card-action"
                            onClick={() => onEditAgent(agent)}
                          >
                            {t("settings.team.edit")}
                          </button>
                        </div>
                        {agent.goal ? <p className="settings-card-body">{agent.goal}</p> : null}
                        <div className="settings-card-foot">
                          <span className="settings-meta-inline">
                            {agent.model ?? t("settings.team.defaultModel")}
                          </span>
                          <span className="settings-meta-inline">
                            {agent.tools.length === 0
                              ? t("settings.team.noTools")
                              : agent.tools.join(", ")}
                          </span>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            </>
          ) : null}

          {section === "memory" ? <MemorySettings agents={agents} /> : null}

          {section === "approvals" ? (
            <section
              className="settings-section"
              aria-labelledby="settings-approvals"
            >
              <div className="settings-section-head">
                <h3 id="settings-approvals">{t("settings.approvals.title")}</h3>
                <p className="settings-hint">
                  {t("settings.approvals.hint")}
                </p>
              </div>

              {sessionError ? (
                <p className="settings-status is-error" role="alert">
                  {sessionError}
                </p>
              ) : null}
              {!localRuntime ? (
                <p className="settings-hint">
                  {t("settings.server.sessionDecisionsHint")}
                </p>
              ) : sessionDecisions.length === 0 ? (
                <p className="settings-empty">
                  {t("settings.approvals.noSessionDecisions")}
                </p>
              ) : (
                <ul className="settings-list">
                  {sessionDecisions.map((entry) => (
                    <li key={entry.key} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">{entry.key}</span>
                        <span className="settings-badge">
                          {decisionLabel(entry.decision)}
                        </span>
                      </div>
                    </li>
                  ))}
                </ul>
              )}

              {localRuntime ? (
                <div className="form-actions">
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={handleClearSessionDecisions}
                    disabled={sessionBusy || sessionDecisions.length === 0}
                  >
                    {sessionBusy
                      ? t("settings.approvals.resetting")
                      : t("settings.approvals.reset")}
                  </button>
                </div>
              ) : null}

              <div className="settings-section-head settings-subsection">
                <h3>{t("settings.approvals.bypassTitle")}</h3>
                <p className="settings-hint">
                  {t("settings.approvals.bypassHint")}
                </p>
              </div>

              {policyError ? (
                <p className="settings-status is-error" role="alert">
                  {policyError}
                </p>
              ) : null}

              <label className="field settings-audit-filter">
                <span>{t("settings.approvals.criticalLabel")}</span>
                <select
                  value={bypassCritical}
                  onChange={(event) =>
                    void handleBypassCritical(
                      event.target.value as BypassCritical,
                    )
                  }
                  disabled={policyBusy}
                  aria-label={t("settings.approvals.criticalLabel")}
                >
                  <option value="allow">{t("settings.approvals.criticalAllow")}</option>
                  <option value="ask">{t("settings.approvals.criticalAsk")}</option>
                  <option value="block">{t("settings.approvals.criticalBlock")}</option>
                </select>
              </label>

              <div className="settings-section-head settings-subsection">
                <h3>{t("settings.approvals.rulesTitle")}</h3>
                <p className="settings-hint">
                  {t("settings.approvals.rulesHint")}
                </p>
              </div>

              {ruleError ? (
                <p className="settings-status is-error" role="alert">
                  {ruleError}
                </p>
              ) : null}

              <form className="settings-rule-form" onSubmit={handleCreateRule}>
                <label className="field">
                  <span>{t("settings.approvals.tool")}</span>
                  <input
                    value={ruleTool}
                    onChange={(event) => setRuleTool(event.target.value)}
                    placeholder={t("settings.approvals.toolPlaceholder")}
                    aria-label={t("settings.approvals.ruleToolAria")}
                  />
                </label>
                <label className="field">
                  <span>{t("settings.approvals.commandPattern")}</span>
                  <input
                    value={ruleCommand}
                    onChange={(event) => setRuleCommand(event.target.value)}
                    placeholder={t("settings.approvals.commandPlaceholder")}
                    aria-label={t("settings.approvals.ruleCommandAria")}
                  />
                </label>
                <label className="field">
                  <span>{t("settings.approvals.decision")}</span>
                  <select
                    value={ruleDecision}
                    onChange={(event) =>
                      setRuleDecision(event.target.value as RuleDecision)
                    }
                    aria-label={t("settings.approvals.ruleDecisionAria")}
                  >
                    <option value="allow">{t("settings.approvals.allow")}</option>
                    <option value="ask">{t("settings.approvals.ask")}</option>
                    <option value="deny">{t("settings.approvals.deny")}</option>
                  </select>
                </label>
                <button
                  type="submit"
                  className="primary-button"
                  disabled={ruleBusy}
                >
                  {ruleBusy
                    ? t("settings.approvals.adding")
                    : t("settings.approvals.addRule")}
                </button>
              </form>

              {rules.length === 0 ? (
                <p className="settings-empty">
                  {t("settings.approvals.noRules")}
                </p>
              ) : (
                <ul className="settings-list">
                  {rules.map((rule) => (
                    <li key={rule.id} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">
                          {rule.tool === "*"
                            ? t("settings.approvals.allTools")
                            : rule.tool}
                          {rule.command ? `: ${rule.command}` : ""}
                        </span>
                        <span className={`settings-badge rule-${rule.decision}`}>
                          {decisionLabel(rule.decision)}
                        </span>
                      </div>
                      <div className="settings-card-actions">
                        <span className="settings-meta-inline">
                          {rule.source === "imported"
                            ? t("settings.approvals.importedReview")
                            : rule.enabled
                              ? t("settings.approvals.enabled")
                              : t("settings.approvals.disabled")}
                        </span>
                        <button
                          type="button"
                          className="ghost-button"
                          disabled={ruleBusy}
                          onClick={() => void handleToggleRule(rule)}
                        >
                          {rule.enabled
                            ? t("settings.approvals.disable")
                            : t("settings.approvals.enable")}
                        </button>
                        <button
                          type="button"
                          className="ghost-button danger-text"
                          disabled={ruleBusy}
                          onClick={() => void handleDeleteRule(rule)}
                        >
                          {t("settings.approvals.delete")}
                        </button>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : null}

          {section === "audit" ? (
            <section
              className="settings-section"
              aria-labelledby="settings-audit"
            >
              <div className="settings-section-head">
                <h3 id="settings-audit">{t("settings.audit.title")}</h3>
                <p className="settings-hint">
                  {t("settings.audit.hint")}
                </p>
              </div>

              <label className="field settings-audit-filter">
                <span>{t("settings.audit.source")}</span>
                <select
                  value={auditSource}
                  onChange={(event) => setAuditSource(event.target.value)}
                  aria-label={t("settings.audit.sourceFilterAria")}
                >
                  <option value="all">{t("settings.audit.allSources")}</option>
                  <option value="rule">{t("settings.audit.rule")}</option>
                  <option value="model">{t("settings.audit.model")}</option>
                  <option value="session">{t("settings.audit.session")}</option>
                  <option value="human">{t("settings.audit.human")}</option>
                  <option value="policy">{t("settings.audit.policy")}</option>
                  <option value="system">{t("settings.audit.system")}</option>
                </select>
              </label>

              {auditError ? (
                <p className="settings-status is-error" role="alert">
                  {auditError}
                </p>
              ) : null}
              {auditBusy ? (
                <p className="settings-empty">{t("settings.audit.loading")}</p>
              ) : auditEvents.length === 0 ? (
                <p className="settings-empty">{t("settings.audit.none")}</p>
              ) : (
                <ul className="settings-list">
                  {auditEvents.map((event) => (
                    <li key={event.id} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">
                          {event.action}
                        </span>
                        <span
                          className={`settings-badge audit-${
                            event.source ?? "none"
                          }`}
                        >
                          {auditSourceLabel(event.source)}
                        </span>
                      </div>
                      <p className="settings-meta-inline">
                        {event.actor} · {formatDateTime(event.created_at)}
                      </p>
                      <p className="audit-target">{event.target}</p>
                    </li>
                  ))}
                </ul>
              )}

              <PromptSnapshotAuditPanel />
            </section>
          ) : null}

          {section === "projects" ? (
            <section className="settings-section" aria-labelledby="settings-projects">
              <div className="settings-section-head">
                <h3 id="settings-projects">{t("settings.projects.title")}</h3>
                <p className="settings-hint">
                  {t("settings.projects.hint")}
                </p>
              </div>

              <div className="form-actions">
                {localRuntime ? (
                  <button
                    type="button"
                    className="primary-button"
                    onClick={() => void handleAddProject()}
                    disabled={busy === "add-project"}
                  >
                    {busy === "add-project"
                      ? t("settings.projects.adding")
                      : t("settings.projects.addFolder")}
                  </button>
                ) : null}
              </div>

              {projects.length === 0 ? (
                <p className="settings-empty">
                  {t("settings.projects.empty")}
                </p>
              ) : (
                <ul className="settings-list">
                  {projects.map((project) => (
                    <li key={project.id} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">{project.name}</span>
                        {project.available && project.current_branch ? (
                          <span className="settings-badge">
                            {project.current_branch}
                          </span>
                        ) : null}
                        {!project.available ? (
                          <span className="settings-badge">
                            {t("settings.projects.missing")}
                          </span>
                        ) : null}
                      </div>
                      <p className="settings-card-body settings-path">
                        {project.path}
                      </p>
                      <div className="settings-card-foot">
                        <span className="settings-meta-inline">
                          {project.available
                            ? t("settings.projects.available")
                            : t("settings.projects.databaseMissing")}
                        </span>
                        <div className="settings-card-actions">
                          <button
                            type="button"
                            className="ghost-button"
                            onClick={() => handleRemoveProject(project)}
                            disabled={busy === "remove-project"}
                          >
                            {t("settings.projects.remove")}
                          </button>
                        </div>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : null}

          {section === "workspace" ? (
            <>
              <section className="settings-section" aria-labelledby="settings-workspace">
                <div className="settings-section-head">
                  <h3 id="settings-workspace">{t("settings.workspace.title")}</h3>
                  <p className="settings-hint">
                    {t("settings.workspace.hint")}
                  </p>
                </div>
                <dl className="settings-meta">
                  <div>
                    <dt>{t("settings.workspace.workspace")}</dt>
                    <dd>{session?.workspace.name ?? "—"}</dd>
                  </div>
                  <div>
                    <dt>{t("settings.workspace.owner")}</dt>
                    <dd>{session?.user.name ?? "—"}</dd>
                  </div>
                  <div>
                    <dt>{t("settings.workspace.email")}</dt>
                    <dd>{session?.user.email ?? "—"}</dd>
                  </div>
                </dl>

                {editingProfile ? (
                  <form className="settings-profile-form" onSubmit={handleProfileSave}>
                    <div className="settings-form-row">
                      <label className="field">
                        <span>{t("settings.workspace.displayName")}</span>
                        <input
                          value={profileName}
                          onChange={(event) => setProfileName(event.target.value)}
                          autoComplete="name"
                          disabled={profileBusy}
                        />
                      </label>
                      <label className="field">
                        <span>{t("settings.workspace.email")}</span>
                        <input
                          type="email"
                          value={profileEmail}
                          onChange={(event) => setProfileEmail(event.target.value)}
                          autoComplete="email"
                          disabled={profileBusy}
                        />
                      </label>
                    </div>
                    {profileError ? (
                      <p className="settings-status is-error" role="alert">
                        {profileError}
                      </p>
                    ) : null}
                    <div className="form-actions">
                      <button
                        type="submit"
                        className="primary-button"
                        disabled={
                          profileBusy ||
                          !profileName.trim() ||
                          !profileEmail.trim()
                        }
                      >
                        {profileBusy
                          ? t("settings.workspace.saving")
                          : t("settings.workspace.saveProfile")}
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        onClick={() => setEditingProfile(false)}
                        disabled={profileBusy}
                      >
                        {t("settings.workspace.cancel")}
                      </button>
                    </div>
                  </form>
                ) : localRuntime ? (
                  <div className="form-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={startEditProfile}
                    >
                      {t("settings.workspace.editProfile")}
                    </button>
                  </div>
                ) : (
                  // The server has no profile-edit route yet, so the account
                  // name is read-only here rather than failing on save.
                  <p className="settings-hint">{t("settings.server.profileHint")}</p>
                )}
              </section>

              {localRuntime ? (
              <section
                className="settings-section"
                aria-labelledby="settings-local-workspaces"
              >
                <div className="settings-section-head">
                  <h3 id="settings-local-workspaces">
                    {t("settings.workspace.localTitle")}
                  </h3>
                  <p className="settings-hint">
                    {t("settings.workspace.localHint")}
                  </p>
                </div>

                {workspaceError ? (
                  <p className="settings-status is-error" role="alert">
                    {workspaceError}
                  </p>
                ) : null}
                {workspaceNotice ? (
                  <p className="settings-status is-ok" role="status">
                    {workspaceNotice}
                  </p>
                ) : null}

                <div className="workspace-create">
                  <label className="field">
                    <span>{t("settings.workspace.newWorkspace")}</span>
                    <input
                      value={newWorkspaceName}
                      onChange={(event) => setNewWorkspaceName(event.target.value)}
                      placeholder={t("settings.workspace.newWorkspacePlaceholder")}
                      autoComplete="off"
                      disabled={workspaceBusy !== null}
                    />
                  </label>
                  <button
                    type="button"
                    className="primary-button"
                    onClick={handleCreateWorkspace}
                    disabled={workspaceBusy !== null || !newWorkspaceName.trim()}
                  >
                    {workspaceBusy === "create"
                      ? t("settings.workspace.creating")
                      : t("settings.workspace.createWorkspace")}
                  </button>
                </div>

                {workspaces.length === 0 ? (
                  <p className="settings-empty">
                    {t("settings.workspace.noneRegistered")}
                  </p>
                ) : (
                  <ul className="settings-list">
                    {workspaces.map((workspace) => (
                      <li key={workspace.database} className="settings-card">
                        <div className="settings-card-head">
                          <span className="settings-card-title">{workspace.name}</span>
                          {workspace.active ? (
                            <span className="settings-badge settings-badge-active">
                              {t("settings.workspace.active")}
                            </span>
                          ) : null}
                          {!workspace.available ? (
                            <span className="settings-badge">
                              {t("settings.workspace.missing")}
                            </span>
                          ) : null}
                        </div>
                        <p className="settings-card-body">{workspace.database}</p>
                        <div className="settings-card-foot">
                          <span className="settings-meta-inline">
                            {workspace.active
                              ? t("settings.workspace.openNow")
                              : workspace.available
                                ? t("settings.workspace.available")
                                : t("settings.workspace.databaseMissing")}
                          </span>
                          <div className="settings-card-actions">
                            {!workspace.active && workspace.available ? (
                              <button
                                type="button"
                                className="ghost-button"
                                onClick={() => handleSwitchWorkspace(workspace.database)}
                                disabled={workspaceBusy !== null}
                              >
                                {workspaceBusy === `switch-${workspace.database}`
                                  ? t("settings.workspace.switching")
                                  : t("settings.workspace.switch")}
                              </button>
                            ) : null}
                          </div>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}

                <div className="form-actions workspace-actions">
                  <button
                    type="button"
                    className="primary-button"
                    onClick={handleExportWorkspace}
                    disabled={workspaceBusy !== null}
                  >
                    {workspaceBusy === "export"
                      ? t("settings.workspace.exporting")
                      : t("settings.workspace.export")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={handleImportWorkspace}
                    disabled={workspaceBusy !== null}
                  >
                    {workspaceBusy === "import"
                      ? t("settings.workspace.importing")
                      : t("settings.workspace.importPackage")}
                  </button>
                </div>
              </section>
              ) : null}
            </>
          ) : null}

          {section === "integrations" ? <IntegrationSettings /> : null}

          {section === "server" ? <ServerSettings /> : null}

          {section === "about" ? (
            <section className="settings-section" aria-labelledby="settings-about">
              <div className="settings-section-head">
                <h3 id="settings-about">{t("settings.about.title")}</h3>
                <p className="settings-hint">
                  {t("settings.about.hint")}
                </p>
              </div>

              <dl className="settings-meta">
                <div>
                  <dt>{t("settings.about.version")}</dt>
                  <dd>{updater.state.currentVersion ?? "—"}</dd>
                </div>
                <div>
                  <dt>{t("settings.about.databaseSchema")}</dt>
                  <dd>{updater.state.currentDatabaseSchema ?? "—"}</dd>
                </div>
                <div>
                  <dt>{t("settings.about.channel")}</dt>
                  <dd>{t("settings.about.stable")}</dd>
                </div>
                <div>
                  <dt>{t("settings.about.lastChecked")}</dt>
                  <dd>
                    {updater.state.checkedAt
                      ? formatDateTime(updater.state.checkedAt)
                      : t("settings.about.notChecked")}
                  </dd>
                </div>
              </dl>

              <label className="settings-checkbox about-toggle">
                <input
                  type="checkbox"
                  checked={updater.autoCheck}
                  onChange={(event) => {
                    setUpdateError(null);
                    updater.setAutoCheck(event.target.checked).catch((error) => {
                      setUpdateError(errorMessage(error));
                    });
                  }}
                />
                {t("settings.about.autoCheck")}
              </label>

              {updateError ? (
                <p className="settings-status is-error" role="alert">
                  {updateError}
                </p>
              ) : null}

              <div className="form-actions about-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={() => {
                    setUpdateError(null);
                    void updater.check();
                  }}
                  disabled={
                    updater.state.phase === "checking" ||
                    updater.state.phase === "downloading"
                  }
                >
                  {updater.state.phase === "checking"
                    ? t("settings.about.checking")
                    : t("settings.about.check")}
                </button>

                {updater.state.phase === "available" ||
                (updater.state.phase === "error" &&
                  updater.state.version !== null) ? (
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void updater.install()}
                    disabled={tasksRunning}
                  >
                    {t("settings.about.downloadInstall")}
                  </button>
                ) : null}

                {updater.state.phase === "ready" ? (
                  <button
                    type="button"
                    className="primary-button"
                    onClick={() => void updater.restart()}
                  >
                    {t("settings.about.restartNow")}
                  </button>
                ) : null}

                {updater.state.version ? (
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => {
                      setUpdateError(null);
                      openUrl(releaseUrl(updater.state.version ?? "")).catch(
                        (error) => {
                          setUpdateError(errorMessage(error));
                        },
                      );
                    }}
                  >
                    {t("settings.about.releaseNotes")}
                  </button>
                ) : null}
              </div>

              {updater.state.phase === "checking" ? (
                <p className="settings-status" role="status">
                  {t("settings.about.checkingReleases")}
                </p>
              ) : null}

              {updater.state.phase === "current" ? (
                <p className="settings-status is-ok" role="status">
                  {t("settings.about.upToDate")}
                </p>
              ) : null}

              {updater.state.phase === "available" ? (
                <p className="settings-status is-ok" role="status">
                  {t("settings.about.availableVersion", {
                    version: updater.state.version ?? "",
                    released: updater.state.date
                      ? t("settings.about.released", {
                          date: releaseDate(updater.state.date),
                        })
                      : "",
                  })}
                  {tasksRunning ? t("settings.about.finishTasks") : ""}
                </p>
              ) : null}

              {updater.state.phase === "downloading" ? (
                <UpdateProgress
                  className="settings-update-progress"
                  downloaded={updater.state.downloaded}
                  total={updater.state.total}
                />
              ) : null}

              {updater.state.phase === "ready" ? (
                <p className="settings-status is-ok" role="status">
                  {t("settings.about.installedRestart")}
                </p>
              ) : null}

              {updater.state.phase === "error" && updater.state.error ? (
                <p className="settings-status is-error" role="alert">
                  {updater.state.version
                    ? t("settings.about.updateFailed", {
                        error: updater.state.error,
                      })
                    : t("settings.about.checkFailed", {
                        error: updater.state.error,
                      })}
                </p>
              ) : null}

              <p className="settings-note">
                {t("settings.about.note")}
              </p>
            </section>
          ) : null}
        </div>
      </div>
    </div>
  );
}
