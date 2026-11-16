import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import ModelProviderSettings from "./ModelProviderSettings";
import icon from "./assets/cowbot-icon.svg";
import { setDatabaseDirectory } from "./api";
import { errorMessage } from "./errors";
import { useI18n, useT } from "./i18n";
import { localeDefinitions } from "./i18n/catalog";
import type { AgentProfile, TemplateDocument } from "./types";

interface OnboardingProps {
  templates: TemplateDocument[];
  agents: AgentProfile[];
  databaseDir: string;
  onAddAgents: (templateId: string) => Promise<AgentProfile[]>;
  onComplete: () => Promise<void>;
  onOpenLanguage: () => void;
}

const steps = ["Welcome", "Model", "Team"] as const;

export default function Onboarding({
  templates,
  agents,
  databaseDir,
  onAddAgents,
  onComplete,
  onOpenLanguage,
}: OnboardingProps) {
  const t = useT();
  const { locale } = useI18n();
  const [step, setStep] = useState(0);
  const [addingTemplate, setAddingTemplate] = useState<string | null>(null);
  const [addedTemplateIds, setAddedTemplateIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [finishing, setFinishing] = useState(false);
  const [changingFolder, setChangingFolder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const busy = addingTemplate !== null || finishing || changingFolder;

  async function chooseDatabaseFolder() {
    setError(null);
    setNotice(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("onboarding.databaseTitle"),
      });
      if (typeof selected !== "string") return;
      setChangingFolder(true);
      // The backend writes the setting and restarts into the new folder.
      await setDatabaseDirectory(selected);
    } catch (err) {
      setChangingFolder(false);
      setError(errorMessage(err));
    }
  }

  function goToStep(next: number) {
    setStep(next);
    setError(null);
    setNotice(null);
  }

  async function handleAddAgents(template: TemplateDocument) {
    setAddingTemplate(template.id);
    setError(null);
    setNotice(null);
    try {
      const added = await onAddAgents(template.id);
      setAddedTemplateIds((current) => new Set(current).add(template.id));
      setNotice(
        t(
          added.length === 1
            ? "onboarding.addedNoticeOne"
            : "onboarding.addedNoticeOther",
          { count: added.length, name: template.name },
        ),
      );
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setAddingTemplate(null);
    }
  }

  async function finish() {
    setFinishing(true);
    setError(null);
    try {
      await onComplete();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setFinishing(false);
    }
  }

  return (
    <div
      className="onboarding"
      role="dialog"
      aria-modal="true"
      aria-label={t("onboarding.setupAria")}
    >
      <header className="onboarding-header">
        <div className="onboarding-header-actions">
          <button
            type="button"
            className="ghost-button onboarding-language"
            onClick={onOpenLanguage}
          >
            <svg
              className="onboarding-globe"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              aria-hidden="true"
            >
              <circle cx="12" cy="12" r="9" />
              <path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18" />
            </svg>
            {localeDefinitions[locale].shortName}
          </button>
          <button
            type="button"
            className="text-button"
            onClick={finish}
            disabled={busy}
          >
            {finishing ? t("onboarding.finishing") : t("onboarding.skip")}
          </button>
        </div>
      </header>

      <main className="onboarding-main">
        <div className="onboarding-panel">
          {step === 0 ? (
            <>
              <span className="empty-icon-tile onboarding-icon-tile">
                <img className="empty-icon" src={icon} alt="" />
              </span>
              <p className="onboarding-kicker">{t("onboarding.welcomeKicker")}</p>
              <h1 className="onboarding-title">
                {t("onboarding.welcomeTitle")}
              </h1>
              <p className="onboarding-copy">
                {t("onboarding.welcomeCopy")}
              </p>
              <ul className="onboarding-points">
                <li>{t("onboarding.pointConversations")}</li>
                <li>{t("onboarding.pointApprovals")}</li>
                <li>{t("onboarding.pointModels")}</li>
              </ul>
              <div className="onboarding-database">
                <div className="onboarding-database-head">
                  <span className="onboarding-database-title">
                    {t("onboarding.databaseTitle")}
                  </span>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => void chooseDatabaseFolder()}
                    disabled={busy}
                  >
                    {changingFolder
                      ? t("onboarding.databaseChanging")
                      : t("onboarding.databaseChange")}
                  </button>
                </div>
                <p className="settings-hint">{t("onboarding.databaseHint")}</p>
                <code className="startup-path">{databaseDir}</code>
              </div>
              <div className="onboarding-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={() => goToStep(1)}
                  autoFocus
                >
                  {t("onboarding.getStarted")}
                </button>
              </div>
            </>
          ) : step === 1 ? (
            <>
              <p className="onboarding-kicker">
                {t("onboarding.stepOf", { current: 2, total: steps.length })}
              </p>
              <h1 className="onboarding-title">{t("onboarding.connectTitle")}</h1>
              <p className="onboarding-copy">
                {t("onboarding.connectCopy")}
              </p>
              <div className="onboarding-provider">
                <ModelProviderSettings showConfigPath={false} />
              </div>
              <div className="onboarding-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={() => goToStep(2)}
                >
                  {t("onboarding.next")}
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => goToStep(0)}
                >
                  {t("onboarding.back")}
                </button>
              </div>
            </>
          ) : (
            <>
              <p className="onboarding-kicker">
                {t("onboarding.stepOf", { current: 3, total: steps.length })}
              </p>
              <h1 className="onboarding-title">{t("onboarding.teamTitle")}</h1>
              <p className="onboarding-copy">
                {t("onboarding.teamCopy")}
              </p>
              {templates.length === 0 ? (
                <p className="settings-empty">{t("onboarding.noTemplates")}</p>
              ) : (
                <ul className="settings-list onboarding-templates">
                  {templates.map((template) => {
                    const added = addedTemplateIds.has(template.id);
                    return (
                      <li key={template.id} className="settings-card">
                        <div className="settings-card-head">
                          <span className="settings-card-title">
                            {template.name}
                          </span>
                          <span className="settings-badge">{template.kind}</span>
                          {added ? (
                            <span className="settings-badge settings-badge-active">
                              {t("onboarding.added")}
                            </span>
                          ) : null}
                        </div>
                        {template.description ? (
                          <p className="settings-card-body">
                            {template.description}
                          </p>
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
                            onClick={() => handleAddAgents(template)}
                            disabled={busy || added}
                          >
                            {addingTemplate === template.id
                              ? t("onboarding.adding")
                              : added
                                ? t("onboarding.added")
                                : t("onboarding.addAgents")}
                          </button>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              )}
              <p className="settings-hint">
                {agents.length === 0
                  ? t("onboarding.noAgents")
                  : t(
                      agents.length === 1
                        ? "onboarding.agentsCountOne"
                        : "onboarding.agentsCountOther",
                      { count: agents.length },
                    )}
              </p>
              <div className="onboarding-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={finish}
                  disabled={busy}
                >
                  {finishing
                    ? t("onboarding.finishing")
                    : t("onboarding.finish")}
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => goToStep(1)}
                  disabled={busy}
                >
                  {t("onboarding.back")}
                </button>
              </div>
            </>
          )}

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

          <footer className="onboarding-footer">
            <span className="onboarding-dots" aria-hidden="true">
              {steps.map((label, index) => (
                <span
                  key={label}
                  className={`onboarding-dot${index === step ? " is-active" : ""}`}
                />
              ))}
            </span>
            <span className="settings-meta-inline">
              {t("onboarding.stepOf", {
                current: step + 1,
                total: steps.length,
              })}
            </span>
          </footer>
        </div>
      </main>
    </div>
  );
}
