import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import {
  acceptInvite,
  connectServer,
  disconnectServer,
  getServerSettings,
  getSession,
  listServerWorkspaces,
  probeServer,
  redeemInvite,
  switchServerWorkspace,
  useLocalBackend,
} from "./api";
import { errorMessage } from "./errors";
import { useI18n } from "./i18n";
import SelectMenu from "./SelectMenu";
import ServerMembers from "./ServerMembers";
import type {
  ClientSession,
  ServerSettings as ServerSettingsView,
  ServerWorkspace,
} from "./types";

/**
 * Settings → Server (ADR-0053, ADR-0066).
 *
 * Two states share this panel. Connected devices list the current session and
 * the account's other workspaces; disconnected ones show a sign-in form that
 * first probes the server so the user chooses a workspace before anything is
 * saved. Every action here restarts CowBot, because the transport is chosen
 * once at startup rather than swapped under running turns.
 *
 * Membership has two ways in, and they are not the same form. Someone with no
 * account redeems an invitation, which creates the account; someone who
 * already has one accepts an invitation and only gains a workspace.
 */
export default function ServerSettings() {
  const { t } = useI18n();
  const [settings, setSettings] = useState<ServerSettingsView | null>(null);
  const [session, setSession] = useState<ClientSession | null>(null);
  const [baseUrl, setBaseUrl] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [workspaces, setWorkspaces] = useState<ServerWorkspace[]>([]);
  const [workspaceId, setWorkspaceId] = useState("");
  const [probed, setProbed] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // "Join with an invitation": the closed form stays out of the way until the
  // person who needs it says so.
  const [joinOpen, setJoinOpen] = useState(false);
  const [joinToken, setJoinToken] = useState("");
  const [joinName, setJoinName] = useState("");
  const [joinEmail, setJoinEmail] = useState("");
  const [joinPassword, setJoinPassword] = useState("");
  const [joinError, setJoinError] = useState<string | null>(null);

  // "Accept an invitation to a second workspace", for an account that already
  // exists here.
  const [acceptToken, setAcceptToken] = useState("");

  useEffect(() => {
    let cancelled = false;
    getServerSettings()
      .then((next) => {
        if (cancelled) return;
        setSettings(next);
        setBaseUrl(next.base_url ?? "");
        setEmail(next.email ?? "");
        setWorkspaceId(next.workspace_id ?? "");
      })
      .catch((err) => {
        if (!cancelled) setError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // A connected device can list the account's other workspaces without asking
  // for the password again.
  useEffect(() => {
    if (settings?.mode !== "server") return;
    let cancelled = false;
    listServerWorkspaces()
      .then((next) => {
        if (!cancelled) setWorkspaces(next);
      })
      .catch(() => {
        // A session that expired is reported where the user acts, not here.
      });
    return () => {
      cancelled = true;
    };
  }, [settings?.mode]);

  // The member panel needs the caller's role and what it may grant, which the
  // session carries. A local workspace answers too, so the panel is not gated
  // on the backend here; the roles decide what renders.
  useEffect(() => {
    let cancelled = false;
    getSession()
      .then((next) => {
        if (!cancelled) setSession(next);
      })
      .catch(() => {
        // The panel simply does not render; the rest of the screen still works.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleProbe(event?: FormEvent) {
    event?.preventDefault();
    setBusy("probe");
    setError(null);
    setNotice(null);
    try {
      const probe = await probeServer(baseUrl, email, password);
      setWorkspaces(probe.workspaces);
      setProbed(true);
      const chosen = probe.workspaces.find((workspace) => workspace.id === workspaceId);
      setWorkspaceId(chosen?.id ?? probe.workspaces[0]?.id ?? "");
    } catch (err) {
      setProbed(false);
      setWorkspaces([]);
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleConnect() {
    const workspace = workspaces.find((candidate) => candidate.id === workspaceId);
    if (!workspace) return;
    setBusy("connect");
    setError(null);
    try {
      await connectServer(baseUrl, email, password, workspace.id);
      // The backend saves the connection and restarts into it.
    } catch (err) {
      setError(errorMessage(err));
      setBusy(null);
    }
  }

  async function handleRedeem(event: FormEvent) {
    event.preventDefault();
    setBusy("redeem");
    setJoinError(null);
    try {
      await redeemInvite(baseUrl, joinToken, joinName, joinEmail, joinPassword);
      // The backend saves the session and restarts into the new workspace.
    } catch (err) {
      setJoinError(t("settings.server.joinFail", { error: errorMessage(err) }));
      setBusy(null);
    }
  }

  async function handleAccept() {
    if (!acceptToken.trim()) return;
    setBusy("accept");
    setError(null);
    setNotice(null);
    try {
      const joined = await acceptInvite(acceptToken.trim());
      setAcceptToken("");
      setNotice(t("settings.server.accepted", { workspace: joined.name }));
      // The new workspace is a restart away, so it belongs in this picker.
      setWorkspaces(await listServerWorkspaces());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleSwitchWorkspace() {
    if (!workspaceId || workspaceId === settings?.workspace_id) return;
    setBusy("switch");
    setError(null);
    try {
      await switchServerWorkspace(workspaceId);
      // The backend saves the choice and restarts into it.
    } catch (err) {
      setError(errorMessage(err));
      setBusy(null);
    }
  }

  async function handleUseLocal() {
    setBusy("local");
    setError(null);
    try {
      await useLocalBackend();
      // The backend restarts into the local workspace.
    } catch (err) {
      setError(errorMessage(err));
      setBusy(null);
    }
  }

  async function handleDisconnect() {
    if (!window.confirm(t("settings.server.disconnectConfirm"))) return;
    setBusy("disconnect");
    setError(null);
    try {
      await disconnectServer();
      // The backend forgets the session and restarts locally.
    } catch (err) {
      setError(errorMessage(err));
      setBusy(null);
    }
  }

  async function handleTest() {
    if (!settings?.base_url || !settings.email) return;
    setBusy("test");
    setError(null);
    setNotice(null);
    try {
      await listServerWorkspaces();
      setNotice(t("settings.server.testOk"));
    } catch (err) {
      setError(t("settings.server.testFailed", { error: errorMessage(err) }));
    } finally {
      setBusy(null);
    }
  }

  const connected = settings?.mode === "server";
  const workspaceOptions = workspaces.map((workspace) => ({
    value: workspace.id,
    label: workspace.name,
  }));
  const selectedWorkspace = workspaces.find(
    (workspace) => workspace.id === workspaceId,
  );

  return (
    <>
      <section className="settings-section" aria-labelledby="settings-server-current">
        <div className="settings-section-head">
          <h3 id="settings-server-current">{t("settings.server.currentTitle")}</h3>
          <p className="settings-hint">{t("settings.server.currentHint")}</p>
        </div>

        {settings === null ? null : (
          <dl className="settings-meta">
            <div>
              <dt>{t("settings.server.mode")}</dt>
              <dd>
                {connected
                  ? t("settings.server.modeServer")
                  : t("settings.server.modeLocal")}
              </dd>
            </div>
            <div>
              <dt>{t("settings.server.serverUrl")}</dt>
              <dd>{settings.base_url ?? "—"}</dd>
            </div>
            <div>
              <dt>{t("settings.server.workspace")}</dt>
              <dd>{settings.workspace_name ?? "—"}</dd>
            </div>
          </dl>
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
      </section>

      {connected ? (
        <section className="settings-section" aria-labelledby="settings-server-session">
          <div className="settings-section-head">
            <h3 id="settings-server-session">{t("settings.server.title")}</h3>
            <p className="settings-hint">
              {t("settings.server.connected", {
                workspace: settings?.workspace_name ?? "",
                email: settings?.email ?? "",
              })}
            </p>
          </div>

          <div className="settings-rule-form">
            <div className="field">
              <span>{t("settings.server.workspace")}</span>
              <SelectMenu
                value={workspaceId}
                options={
                  workspaceOptions.length > 0
                    ? workspaceOptions
                    : [
                        {
                          value: settings?.workspace_id ?? "",
                          label: settings?.workspace_name ?? "",
                        },
                      ]
                }
                onChange={setWorkspaceId}
                ariaLabel={t("settings.server.workspace")}
                disabled={busy !== null}
              />
            </div>
          </div>

          <div className="form-actions">
            <button
              type="button"
              className="primary-button"
              onClick={handleSwitchWorkspace}
              disabled={
                busy !== null ||
                !selectedWorkspace ||
                selectedWorkspace.id === settings?.workspace_id
              }
            >
              {busy === "switch"
                ? t("settings.server.switchingWorkspace")
                : t("settings.server.switchWorkspace")}
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={handleTest}
              disabled={busy !== null}
            >
              {busy === "test"
                ? t("settings.server.testing")
                : t("settings.server.test")}
            </button>
            <button
              type="button"
              className="ghost-button"
              onClick={handleUseLocal}
              disabled={busy !== null}
            >
              {busy === "local"
                ? t("settings.server.usingLocal")
                : t("settings.server.useLocal")}
            </button>
            <button
              type="button"
              className="ghost-button danger-text"
              onClick={handleDisconnect}
              disabled={busy !== null}
            >
              {busy === "disconnect"
                ? t("settings.server.disconnectBusy")
                : t("settings.server.disconnect")}
            </button>
          </div>

          <p className="settings-note">{t("settings.server.localOnlyHint")}</p>

          <div className="settings-subsection">
            <div className="settings-section-head">
              <h4>{t("settings.server.acceptTitle")}</h4>
              <p className="settings-hint">{t("settings.server.acceptHint")}</p>
            </div>
            <div className="settings-rule-form">
              <label className="field">
                <span>{t("settings.server.joinToken")}</span>
                <input
                  type="text"
                  value={acceptToken}
                  onChange={(event) => setAcceptToken(event.target.value)}
                  spellCheck={false}
                  autoComplete="off"
                  disabled={busy !== null}
                />
              </label>
              <div className="form-actions">
                <button
                  type="button"
                  className="ghost-button"
                  onClick={handleAccept}
                  disabled={busy !== null || !acceptToken.trim()}
                >
                  {busy === "accept"
                    ? t("settings.server.acceptBusy")
                    : t("settings.server.acceptSubmit")}
                </button>
              </div>
            </div>
          </div>
        </section>
      ) : (
        <section className="settings-section" aria-labelledby="settings-server-connect">
          <div className="settings-section-head">
            <h3 id="settings-server-connect">{t("settings.server.title")}</h3>
            <p className="settings-hint">{t("settings.server.hint")}</p>
          </div>

          <form className="settings-profile-form" onSubmit={handleProbe}>
            <label className="field">
              <span>{t("settings.server.serverUrl")}</span>
              <input
                type="url"
                value={baseUrl}
                onChange={(event) => {
                  setBaseUrl(event.target.value);
                  setProbed(false);
                }}
                placeholder={t("settings.server.serverUrlPlaceholder")}
                autoComplete="url"
                inputMode="url"
                disabled={busy !== null}
              />
            </label>
            <p className="settings-hint">{t("settings.server.serverUrlHint")}</p>

            <div className="settings-form-row">
              <label className="field">
                <span>{t("settings.server.email")}</span>
                <input
                  type="email"
                  value={email}
                  onChange={(event) => {
                    setEmail(event.target.value);
                    setProbed(false);
                  }}
                  autoComplete="username"
                  disabled={busy !== null}
                />
              </label>
              <label className="field">
                <span>{t("settings.server.password")}</span>
                <input
                  type="password"
                  value={password}
                  onChange={(event) => {
                    setPassword(event.target.value);
                    setProbed(false);
                  }}
                  autoComplete="current-password"
                  disabled={busy !== null}
                />
              </label>
            </div>

            {probed ? (
              <>
                <p className="settings-hint">
                  {workspaces.length === 1
                    ? t("settings.server.foundWorkspaceOne")
                    : t("settings.server.foundWorkspaceOther", {
                        count: workspaces.length,
                      })}
                </p>
                <label className="field">
                  <span>{t("settings.server.workspace")}</span>
                  <SelectMenu
                    value={workspaceId}
                    options={workspaceOptions}
                    onChange={setWorkspaceId}
                    ariaLabel={t("settings.server.workspace")}
                    disabled={busy !== null}
                  />
                </label>
              </>
            ) : null}

            {settings?.base_url && !probed ? (
              <p className="settings-hint">{t("settings.server.savedHint")}</p>
            ) : null}

            <div className="form-actions">
              {probed ? (
                <>
                  <button
                    type="button"
                    className="primary-button"
                    onClick={handleConnect}
                    disabled={busy !== null || !selectedWorkspace}
                  >
                    {busy === "connect"
                      ? t("settings.server.connecting")
                      : t("settings.server.connect")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => setProbed(false)}
                    disabled={busy !== null}
                  >
                    {t("settings.workspace.cancel")}
                  </button>
                </>
              ) : (
                <button
                  type="submit"
                  className="primary-button"
                  disabled={
                    busy !== null ||
                    !baseUrl.trim() ||
                    !email.trim() ||
                    !password
                  }
                >
                  {busy === "probe"
                    ? t("settings.server.signingIn")
                    : t("settings.server.signIn")}
                </button>
              )}
            </div>
          </form>

          {/* Someone with no account cannot sign in at all, so this is the only
              way in for them and it belongs where they would look for it. */}
          <div className="settings-subsection">
            {joinOpen ? (
              <form className="settings-profile-form" onSubmit={handleRedeem}>
                <div className="settings-section-head">
                  <h4>{t("settings.server.joinTitle")}</h4>
                  <p className="settings-hint">{t("settings.server.joinHint")}</p>
                </div>
                <label className="field">
                  <span>{t("settings.server.serverUrl")}</span>
                  <input
                    type="url"
                    value={baseUrl}
                    onChange={(event) => setBaseUrl(event.target.value)}
                    placeholder={t("settings.server.serverUrlPlaceholder")}
                    autoComplete="url"
                    inputMode="url"
                    disabled={busy !== null}
                  />
                </label>
                <label className="field">
                  <span>{t("settings.server.joinToken")}</span>
                  <input
                    type="text"
                    value={joinToken}
                    onChange={(event) => setJoinToken(event.target.value)}
                    spellCheck={false}
                    autoComplete="off"
                    disabled={busy !== null}
                  />
                </label>
                <div className="settings-form-row">
                  <label className="field">
                    <span>{t("settings.server.joinName")}</span>
                    <input
                      type="text"
                      value={joinName}
                      onChange={(event) => setJoinName(event.target.value)}
                      autoComplete="name"
                      disabled={busy !== null}
                    />
                  </label>
                  <label className="field">
                    <span>{t("settings.server.email")}</span>
                    <input
                      type="email"
                      value={joinEmail}
                      onChange={(event) => setJoinEmail(event.target.value)}
                      autoComplete="username"
                      disabled={busy !== null}
                    />
                  </label>
                </div>
                <label className="field">
                  <span>{t("settings.server.password")}</span>
                  <input
                    type="password"
                    value={joinPassword}
                    onChange={(event) => setJoinPassword(event.target.value)}
                    autoComplete="new-password"
                    disabled={busy !== null}
                  />
                </label>

                {joinError ? (
                  <p className="settings-status is-error" role="alert">
                    {joinError}
                  </p>
                ) : null}

                <div className="form-actions">
                  <button
                    type="submit"
                    className="primary-button"
                    disabled={
                      busy !== null ||
                      !baseUrl.trim() ||
                      !joinToken.trim() ||
                      !joinName.trim() ||
                      !joinEmail.trim() ||
                      !joinPassword
                    }
                  >
                    {busy === "redeem"
                      ? t("settings.server.joinBusy")
                      : t("settings.server.joinSubmit")}
                  </button>
                  <button
                    type="button"
                    className="ghost-button"
                    onClick={() => {
                      setJoinOpen(false);
                      setJoinError(null);
                    }}
                    disabled={busy !== null}
                  >
                    {t("settings.workspace.cancel")}
                  </button>
                </div>
              </form>
            ) : (
              <button
                type="button"
                className="settings-add-button"
                onClick={() => setJoinOpen(true)}
              >
                {t("settings.server.joinTitle")}
              </button>
            )}
          </div>
        </section>
      )}

      {session ? <ServerMembers grantableRoles={session.grantable_roles} /> : null}
    </>
  );
}

