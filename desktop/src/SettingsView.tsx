import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

import ModelProviderSettings from "./ModelProviderSettings";
import UpdateProgress from "./UpdateProgress";
import { errorMessage } from "./errors";
import { formatDateTime } from "./format";
import {
  clearSessionDecisions,
  createPermissionRule,
  createWorkspace,
  deletePermissionRule,
  exportWorkspace,
  getAuditEvents,
  getSessionDecisions,
  getWorkspacePolicy,
  importWorkspace,
  listPermissionRules,
  listWorkspaces,
  setBypassCritical as saveBypassCritical,
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
  RuleDecision,
  SessionDecisionEntry,
  TemplateDocument,
  WorkspaceSummary,
} from "./types";
import type { Updater } from "./updater";

interface SettingsViewProps {
  session: ClientSession | null;
  agents: AgentProfile[];
  templates: TemplateDocument[];
  templateId: string;
  busy: string | null;
  updater: Updater;
  tasksRunning: boolean;
  section: SettingsSection;
  onSelectTemplate: (templateId: string) => void;
  onAddAgents: () => void;
  onEditAgent: (agent: AgentProfile) => void;
  onSessionChange: (session: ClientSession) => void;
}

export type SettingsSection =
  | "model"
  | "team"
  | "approvals"
  | "audit"
  | "workspace"
  | "about";

export const settingsSections: { id: SettingsSection; label: string }[] = [
  { id: "model", label: "Model" },
  { id: "team", label: "Team" },
  { id: "approvals", label: "Approvals" },
  { id: "audit", label: "Audit" },
  { id: "workspace", label: "Workspace" },
  { id: "about", label: "About" },
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
    : date.toLocaleDateString(undefined, {
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
  onSelectTemplate,
  onAddAgents,
  onEditAgent,
  onSessionChange,
}: SettingsViewProps) {
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
    if (section !== "approvals") return;
    let cancelled = false;
    setSessionError(null);
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
  }, [section]);

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
    if (!window.confirm(`Delete the workspace rule "${label}"?`)) return;
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
        title: "Export workspace",
        defaultPath: packageFileName(session?.workspace.name),
        filters: [{ name: "CowBot package", extensions: ["cowpkg"] }],
      });
    } catch (error) {
      setWorkspaceError(errorMessage(error));
      return;
    }
    if (!target) return;

    setWorkspaceBusy("export");
    try {
      const written = await exportWorkspace(target);
      setWorkspaceNotice(`Exported to ${written}`);
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
        title: "Import workspace package",
        multiple: false,
        directory: false,
        filters: [{ name: "CowBot package", extensions: ["cowpkg"] }],
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

  function startEditProfile() {
    setProfileName(session?.user.name ?? "");
    setProfileEmail(session?.user.email ?? "");
    setProfileError(null);
    setEditingProfile(true);
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
        <h2 className="view-title">Settings</h2>
        <p className="view-subtitle">
          Model providers, team, and workspace details for this workspace.
        </p>
      </header>

      <div className="settings-layout">
        <div className="settings-content">
          {section === "model" ? (
            <section className="settings-section" aria-labelledby="settings-model">
              <div className="settings-section-head">
                <h3 id="settings-model">Model providers</h3>
                <p className="settings-hint">
                  Connect an OpenAI-compatible, Anthropic-compatible, or Responses-compatible
                  endpoint. Without an active provider, CowBot uses the offline mock model.
                </p>
              </div>
              <ModelProviderSettings />
            </section>
          ) : null}

          {section === "team" ? (
            <>
              <section className="settings-section" aria-labelledby="settings-templates">
                <div className="settings-section-head">
                  <h3 id="settings-templates">Agent templates</h3>
                  <p className="settings-hint">
                    The Main Assistant is the primary agent; specialist templates
                    add more agents to this workspace.
                  </p>
                </div>
                {templates.length === 0 ? (
                  <p className="settings-empty">No templates available.</p>
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
                            {template.agents.length} agent
                            {template.agents.length === 1 ? "" : "s"}
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
                              ? "Adding…"
                              : "Add agents"}
                          </button>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>

              <section className="settings-section" aria-labelledby="settings-agents">
                <div className="settings-section-head">
                  <h3 id="settings-agents">Agents</h3>
                  <p className="settings-hint">
                    {agents.length === 0
                      ? "No agents yet. Add a template above."
                      : `${agents.length} agent${agents.length === 1 ? "" : "s"} in this workspace.`}
                  </p>
                </div>
                {agents.length === 0 ? (
                  <p className="settings-empty">No agents yet.</p>
                ) : (
                  <ul className="settings-list">
                    {agents.map((agent) => (
                      <li key={agent.id} className="settings-card">
                        <div className="settings-card-head">
                          <span className="agent-avatar" aria-hidden="true">
                            {agentInitials(agent.name)}
                          </span>
                          <div>
                            <span className="settings-card-title">{agent.name}</span>
                            <p className="settings-card-subtitle">{agent.role}</p>
                          </div>
                          <button
                            type="button"
                            className="text-button settings-card-action"
                            onClick={() => onEditAgent(agent)}
                          >
                            Edit
                          </button>
                        </div>
                        {agent.goal ? <p className="settings-card-body">{agent.goal}</p> : null}
                        <div className="settings-card-foot">
                          <span className="settings-meta-inline">
                            {agent.model ?? "default model"}
                          </span>
                          <span className="settings-meta-inline">
                            {agent.tools.length === 0
                              ? "no tools"
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

          {section === "approvals" ? (
            <section
              className="settings-section"
              aria-labelledby="settings-approvals"
            >
              <div className="settings-section-head">
                <h3 id="settings-approvals">Session approvals</h3>
                <p className="settings-hint">
                  Decisions remembered for this app session only. Restarting
                  CowBot clears them. Rules and plan mode follow in this
                  milestone.
                </p>
              </div>

              {sessionError ? (
                <p className="settings-status is-error" role="alert">
                  {sessionError}
                </p>
              ) : null}
              {sessionDecisions.length === 0 ? (
                <p className="settings-empty">
                  No session decisions yet. Use "Approve for session" on an
                  approval card.
                </p>
              ) : (
                <ul className="settings-list">
                  {sessionDecisions.map((entry) => (
                    <li key={entry.key} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">{entry.key}</span>
                        <span className="settings-badge">
                          {entry.decision}
                        </span>
                      </div>
                    </li>
                  ))}
                </ul>
              )}

              <div className="form-actions">
                <button
                  type="button"
                  className="ghost-button"
                  onClick={handleClearSessionDecisions}
                  disabled={sessionBusy || sessionDecisions.length === 0}
                >
                  {sessionBusy ? "Resetting…" : "Reset session decisions"}
                </button>
              </div>

              <div className="settings-section-head settings-subsection">
                <h3>Bypass permissions</h3>
                <p className="settings-hint">
                  Bypass permissions runs every tool call. Critical-risk calls
                  under Bypass follow this setting; every other mode always
                  blocks Critical calls. Selecting Bypass shows a warning each
                  time and keeps a red marker while it is active.
                </p>
              </div>

              {policyError ? (
                <p className="settings-status is-error" role="alert">
                  {policyError}
                </p>
              ) : null}

              <label className="field settings-audit-filter">
                <span>Critical calls under Bypass</span>
                <select
                  value={bypassCritical}
                  onChange={(event) =>
                    void handleBypassCritical(
                      event.target.value as BypassCritical,
                    )
                  }
                  disabled={policyBusy}
                  aria-label="Critical calls under Bypass"
                >
                  <option value="allow">Allow (default)</option>
                  <option value="ask">Ask with a red approval card</option>
                  <option value="block">Block</option>
                </select>
              </label>

              <div className="settings-section-head settings-subsection">
                <h3>Workspace rules</h3>
                <p className="settings-hint">
                  Allow, ask, or deny rules evaluated before the model
                  evaluator. Deny wins over ask, and ask over allow. Rules are
                  stored with this workspace and exported with it; imported
                  rules start disabled until reviewed.
                </p>
              </div>

              {ruleError ? (
                <p className="settings-status is-error" role="alert">
                  {ruleError}
                </p>
              ) : null}

              <form className="settings-rule-form" onSubmit={handleCreateRule}>
                <label className="field">
                  <span>Tool</span>
                  <input
                    value={ruleTool}
                    onChange={(event) => setRuleTool(event.target.value)}
                    placeholder="execute_code or *"
                    aria-label="Rule tool"
                  />
                </label>
                <label className="field">
                  <span>Command pattern</span>
                  <input
                    value={ruleCommand}
                    onChange={(event) => setRuleCommand(event.target.value)}
                    placeholder="git * (optional)"
                    aria-label="Rule command pattern"
                  />
                </label>
                <label className="field">
                  <span>Decision</span>
                  <select
                    value={ruleDecision}
                    onChange={(event) =>
                      setRuleDecision(event.target.value as RuleDecision)
                    }
                    aria-label="Rule decision"
                  >
                    <option value="allow">Allow</option>
                    <option value="ask">Ask</option>
                    <option value="deny">Deny</option>
                  </select>
                </label>
                <button
                  type="submit"
                  className="primary-button"
                  disabled={ruleBusy}
                >
                  {ruleBusy ? "Adding…" : "Add rule"}
                </button>
              </form>

              {rules.length === 0 ? (
                <p className="settings-empty">
                  No workspace rules yet. Rules apply to every agent in this
                  workspace.
                </p>
              ) : (
                <ul className="settings-list">
                  {rules.map((rule) => (
                    <li key={rule.id} className="settings-card">
                      <div className="settings-card-head">
                        <span className="settings-card-title">
                          {rule.tool === "*" ? "all tools" : rule.tool}
                          {rule.command ? `: ${rule.command}` : ""}
                        </span>
                        <span className={`settings-badge rule-${rule.decision}`}>
                          {rule.decision}
                        </span>
                      </div>
                      <div className="settings-card-actions">
                        <span className="settings-meta-inline">
                          {rule.source === "imported"
                            ? "imported — review before enabling"
                            : rule.enabled
                              ? "enabled"
                              : "disabled"}
                        </span>
                        <button
                          type="button"
                          className="ghost-button"
                          disabled={ruleBusy}
                          onClick={() => void handleToggleRule(rule)}
                        >
                          {rule.enabled ? "Disable" : "Enable"}
                        </button>
                        <button
                          type="button"
                          className="ghost-button danger-text"
                          disabled={ruleBusy}
                          onClick={() => void handleDeleteRule(rule)}
                        >
                          Delete
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
                <h3 id="settings-audit">Audit</h3>
                <p className="settings-hint">
                  Every tool decision records its source: a workspace rule, the
                  model evaluator, a session decision, a human approval, the
                  active approval policy, or the runtime.
                </p>
              </div>

              <label className="field settings-audit-filter">
                <span>Source</span>
                <select
                  value={auditSource}
                  onChange={(event) => setAuditSource(event.target.value)}
                  aria-label="Audit source filter"
                >
                  <option value="all">All sources</option>
                  <option value="rule">Rule</option>
                  <option value="model">Model</option>
                  <option value="session">Session</option>
                  <option value="human">Human</option>
                  <option value="policy">Policy</option>
                  <option value="system">System</option>
                </select>
              </label>

              {auditError ? (
                <p className="settings-status is-error" role="alert">
                  {auditError}
                </p>
              ) : null}
              {auditBusy ? (
                <p className="settings-empty">Loading audit events…</p>
              ) : auditEvents.length === 0 ? (
                <p className="settings-empty">No audit events for this source.</p>
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
                          {event.source ?? "unknown"}
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
            </section>
          ) : null}

          {section === "workspace" ? (
            <>
              <section className="settings-section" aria-labelledby="settings-workspace">
                <div className="settings-section-head">
                  <h3 id="settings-workspace">Workspace</h3>
                  <p className="settings-hint">
                    Local workspace identity. Data stays on this machine.
                  </p>
                </div>
                <dl className="settings-meta">
                  <div>
                    <dt>Workspace</dt>
                    <dd>{session?.workspace.name ?? "—"}</dd>
                  </div>
                  <div>
                    <dt>Owner</dt>
                    <dd>{session?.user.name ?? "—"}</dd>
                  </div>
                  <div>
                    <dt>Email</dt>
                    <dd>{session?.user.email ?? "—"}</dd>
                  </div>
                </dl>

                {editingProfile ? (
                  <form className="settings-profile-form" onSubmit={handleProfileSave}>
                    <div className="settings-form-row">
                      <label className="field">
                        <span>Display name</span>
                        <input
                          value={profileName}
                          onChange={(event) => setProfileName(event.target.value)}
                          autoComplete="name"
                          disabled={profileBusy}
                        />
                      </label>
                      <label className="field">
                        <span>Email</span>
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
                        {profileBusy ? "Saving…" : "Save profile"}
                      </button>
                      <button
                        type="button"
                        className="ghost-button"
                        onClick={() => setEditingProfile(false)}
                        disabled={profileBusy}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                ) : (
                  <div className="form-actions">
                    <button
                      type="button"
                      className="ghost-button"
                      onClick={startEditProfile}
                    >
                      Edit profile
                    </button>
                  </div>
                )}
              </section>

              <section
                className="settings-section"
                aria-labelledby="settings-local-workspaces"
              >
                <div className="settings-section-head">
                  <h3 id="settings-local-workspaces">Local workspaces</h3>
                  <p className="settings-hint">
                    Each workspace lives in its own local database. Creating,
                    importing, and switching restart CowBot; finish running tasks
                    first.
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
                    <span>New workspace</span>
                    <input
                      value={newWorkspaceName}
                      onChange={(event) => setNewWorkspaceName(event.target.value)}
                      placeholder="Team workspace"
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
                    {workspaceBusy === "create" ? "Creating…" : "Create workspace"}
                  </button>
                </div>

                {workspaces.length === 0 ? (
                  <p className="settings-empty">No workspaces registered.</p>
                ) : (
                  <ul className="settings-list">
                    {workspaces.map((workspace) => (
                      <li key={workspace.database} className="settings-card">
                        <div className="settings-card-head">
                          <span className="settings-card-title">{workspace.name}</span>
                          {workspace.active ? (
                            <span className="settings-badge settings-badge-active">
                              Active
                            </span>
                          ) : null}
                          {!workspace.available ? (
                            <span className="settings-badge">Missing</span>
                          ) : null}
                        </div>
                        <p className="settings-card-body">{workspace.database}</p>
                        <div className="settings-card-foot">
                          <span className="settings-meta-inline">
                            {workspace.active
                              ? "Open now"
                              : workspace.available
                                ? "Available"
                                : "Database file not found"}
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
                                  ? "Switching…"
                                  : "Switch"}
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
                    {workspaceBusy === "export" ? "Exporting…" : "Export workspace"}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={handleImportWorkspace}
                    disabled={workspaceBusy !== null}
                  >
                    {workspaceBusy === "import" ? "Importing…" : "Import package"}
                  </button>
                </div>
              </section>
            </>
          ) : null}

          {section === "about" ? (
            <section className="settings-section" aria-labelledby="settings-about">
              <div className="settings-section-head">
                <h3 id="settings-about">About CowBot</h3>
                <p className="settings-hint">
                  Desktop version and updates. CowBot checks GitHub Releases for
                  stable versions.
                </p>
              </div>

              <dl className="settings-meta">
                <div>
                  <dt>Version</dt>
                  <dd>{updater.state.currentVersion ?? "—"}</dd>
                </div>
                <div>
                  <dt>Update channel</dt>
                  <dd>Stable · GitHub Releases</dd>
                </div>
                <div>
                  <dt>Last checked</dt>
                  <dd>
                    {updater.state.checkedAt
                      ? formatDateTime(updater.state.checkedAt)
                      : "Not checked yet"}
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
                Check for updates automatically when CowBot starts
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
                    ? "Checking…"
                    : "Check for updates"}
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
                    Download and install
                  </button>
                ) : null}

                {updater.state.phase === "ready" ? (
                  <button
                    type="button"
                    className="primary-button"
                    onClick={() => void updater.restart()}
                  >
                    Restart now
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
                    View release notes
                  </button>
                ) : null}
              </div>

              {updater.state.phase === "checking" ? (
                <p className="settings-status" role="status">
                  Checking GitHub Releases…
                </p>
              ) : null}

              {updater.state.phase === "current" ? (
                <p className="settings-status is-ok" role="status">
                  CowBot is up to date.
                </p>
              ) : null}

              {updater.state.phase === "available" ? (
                <p className="settings-status is-ok" role="status">
                  Version {updater.state.version} is available
                  {updater.state.date
                    ? ` (released ${releaseDate(updater.state.date)})`
                    : ""}
                  .
                  {tasksRunning
                    ? " Finish running tasks before installing."
                    : ""}
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
                  Installed. Restart CowBot to finish updating.
                </p>
              ) : null}

              {updater.state.phase === "error" && updater.state.error ? (
                <p className="settings-status is-error" role="alert">
                  {updater.state.version
                    ? `Update failed: ${updater.state.error}`
                    : `Could not check for updates: ${updater.state.error}`}
                </p>
              ) : null}

              <p className="settings-note">
                Automatic updates work for the NSIS installer on Windows, the app
                installed from the DMG on macOS, and the AppImage on Linux. MSI
                and deb installs update by downloading a new release. Builds are
                unsigned, so macOS may ask you to allow CowBot again after an
                update. When enabled, CowBot makes one request to GitHub at
                startup; no workspace data is sent.
              </p>
            </section>
          ) : null}
        </div>
      </div>
    </div>
  );
}
