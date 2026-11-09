import { useState } from "react";

import ModelProviderSettings from "./ModelProviderSettings";
import icon from "./assets/cowbot-icon.svg";
import wordmark from "./assets/cowbot-wordmark-dark.svg";
import { errorMessage } from "./errors";
import type { AgentProfile, TemplateDocument } from "./types";

interface OnboardingProps {
  templates: TemplateDocument[];
  agents: AgentProfile[];
  onAddAgents: (templateId: string) => Promise<AgentProfile[]>;
  onComplete: () => Promise<void>;
}

const steps = ["Welcome", "Model", "Team"] as const;

export default function Onboarding({
  templates,
  agents,
  onAddAgents,
  onComplete,
}: OnboardingProps) {
  const [step, setStep] = useState(0);
  const [addingTemplate, setAddingTemplate] = useState<string | null>(null);
  const [addedTemplateIds, setAddedTemplateIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [finishing, setFinishing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const busy = addingTemplate !== null || finishing;

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
        `Added ${added.length} agent${added.length === 1 ? "" : "s"} from ${template.name}.`,
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
      aria-label="CowBot setup"
    >
      <header className="onboarding-header">
        <img className="wordmark" src={wordmark} alt="CowBot" />
        <button
          type="button"
          className="text-button"
          onClick={finish}
          disabled={busy}
        >
          {finishing ? "Finishing…" : "Skip setup"}
        </button>
      </header>

      <main className="onboarding-main">
        <div className="onboarding-panel">
          {step === 0 ? (
            <>
              <span className="empty-icon-tile onboarding-icon-tile">
                <img className="empty-icon" src={icon} alt="" />
              </span>
              <p className="onboarding-kicker">Welcome to CowBot</p>
              <h1 className="onboarding-title">
                A local workspace for your agent team.
              </h1>
              <p className="onboarding-copy">
                CowBot runs agents on this machine. Messages, tasks, and run
                history stay in a local database; only model requests leave the
                device, to the endpoint you configure.
              </p>
              <ul className="onboarding-points">
                <li>Every agent has a conversation; channels keep the team together.</li>
                <li>Tool calls that touch your machine wait for approval.</li>
                <li>Switch models or go offline at any time in Settings.</li>
              </ul>
              <div className="onboarding-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={() => goToStep(1)}
                  autoFocus
                >
                  Get started
                </button>
              </div>
            </>
          ) : step === 1 ? (
            <>
              <p className="onboarding-kicker">Step 2 of 3</p>
              <h1 className="onboarding-title">Connect a model</h1>
              <p className="onboarding-copy">
                Pick a provider from the catalogue, or choose Custom to enter
                your own endpoint, and connect it to run agents with a real
                model. Skip this step and CowBot will use the offline mock model
                until you configure one.
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
                  Next
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => goToStep(0)}
                >
                  Back
                </button>
              </div>
            </>
          ) : (
            <>
              <p className="onboarding-kicker">Step 3 of 3</p>
              <h1 className="onboarding-title">Assemble a starter team</h1>
              <p className="onboarding-copy">
                A template adds agents with roles, goals, and tool access. You can
                add more from Settings later.
              </p>
              {templates.length === 0 ? (
                <p className="settings-empty">No templates available.</p>
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
                              Added
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
                            {template.agents.length} agent
                            {template.agents.length === 1 ? "" : "s"}
                          </span>
                          <button
                            type="button"
                            className="ghost-button"
                            onClick={() => handleAddAgents(template)}
                            disabled={busy || added}
                          >
                            {addingTemplate === template.id
                              ? "Adding…"
                              : added
                                ? "Added"
                                : "Add agents"}
                          </button>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              )}
              <p className="settings-hint">
                {agents.length === 0
                  ? "No agents in this workspace yet."
                  : `${agents.length} agent${agents.length === 1 ? "" : "s"} in this workspace.`}
              </p>
              <div className="onboarding-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={finish}
                  disabled={busy}
                >
                  {finishing ? "Finishing…" : "Finish setup"}
                </button>
                <button
                  type="button"
                  className="ghost-button"
                  onClick={() => goToStep(1)}
                  disabled={busy}
                >
                  Back
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
              Step {step + 1} of {steps.length}
            </span>
          </footer>
        </div>
      </main>
    </div>
  );
}
