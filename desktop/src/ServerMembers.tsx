import { useCallback, useEffect, useState } from "react";

import {
  createInvite,
  getMembers,
  listInvites,
  removeMember,
  revokeInvite,
  setMemberRole,
} from "./api";
import { errorMessage } from "./errors";
import { currentIntlLocale, useI18n } from "./i18n";
import SelectMenu from "./SelectMenu";
import type { InviteGrant, InviteView, MemberView, Role } from "./types";

/**
 * Settings → Server → Members and Invitations (ADR-0066).
 *
 * The panel renders what this account may actually do: a plain member sees no
 * member panel at all, and the role pickers offer only what the server said
 * this account may grant. Nothing here re-derives the policy — the server
 * states `grantable_roles` with every workspace it returns.
 *
 * Controls appear on a row whose *current* role is in that set. For an admin
 * that is exactly the set of people they may manage (members, not peers); for
 * an owner it is everyone. Where the server refuses anyway — demoting the last
 * owner — its message is shown rather than predicted here.
 *
 * The list is a snapshot. There is no membership event on the live stream, so
 * someone joining from another device shows up on the next open or refresh.
 */

interface Props {
  /** What this account may hand out; empty means no member panel at all. */
  grantableRoles: Role[];
}

export default function ServerMembers({ grantableRoles }: Props) {
  const { t } = useI18n();
  const [members, setMembers] = useState<MemberView[]>([]);
  const [invites, setInvites] = useState<InviteView[]>([]);
  const [granted, setGranted] = useState<InviteGrant | null>(null);
  const [copied, setCopied] = useState(false);
  const [inviteRole, setInviteRole] = useState<Role>(grantableRoles[0] ?? "member");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const roleLabel = useCallback(
    (role: Role): string =>
      role === "owner"
        ? t("settings.server.roleOwner")
        : role === "admin"
          ? t("settings.server.roleAdmin")
          : t("settings.server.roleMember"),
    [t],
  );

  const refresh = useCallback(async () => {
    const [nextMembers, nextInvites] = await Promise.all([getMembers(), listInvites()]);
    setMembers(nextMembers);
    setInvites(nextInvites);
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

  async function handleRefresh() {
    setBusy("refresh");
    setError(null);
    setNotice(null);
    try {
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleRole(member: MemberView, role: Role) {
    if (role === member.role) return;
    setBusy(member.user_id);
    setError(null);
    setNotice(null);
    try {
      const updated = await setMemberRole(member.user_id, role);
      setMembers((current) =>
        current.map((candidate) =>
          candidate.user_id === updated.user_id ? updated : candidate,
        ),
      );
      setNotice(
        t("settings.server.memberRoleChanged", {
          name: updated.name,
          role: roleLabel(updated.role),
        }),
      );
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleRemove(member: MemberView) {
    if (!window.confirm(t("settings.server.memberRemoveConfirm", { name: member.name }))) {
      return;
    }
    setBusy(member.user_id);
    setError(null);
    setNotice(null);
    try {
      await removeMember(member.user_id);
      setMembers((current) =>
        current.filter((candidate) => candidate.user_id !== member.user_id),
      );
      setNotice(t("settings.server.memberRemoved", { name: member.name }));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleCreateInvite() {
    setBusy("invite");
    setError(null);
    setNotice(null);
    setCopied(false);
    try {
      const created = await createInvite(inviteRole);
      // The token is never in the list, so it is held here until the user moves
      // on. Revoking it clears it; that is the only way to make it disappear.
      setGranted(created);
      setInvites((current) => [...current, created]);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleCopy() {
    if (!granted) return;
    try {
      await navigator.clipboard.writeText(granted.token);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function handleRevoke(invite: InviteView) {
    if (!window.confirm(t("settings.server.inviteRevokeConfirm"))) return;
    setBusy(invite.id);
    setError(null);
    setNotice(null);
    try {
      await revokeInvite(invite.id);
      setInvites((current) => current.filter((candidate) => candidate.id !== invite.id));
      if (granted?.id === invite.id) setGranted(null);
      setNotice(t("settings.server.inviteRevoked"));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  const roleOptions = grantableRoles.map((role) => ({ value: role, label: roleLabel(role) }));

  /** The roles a row's picker offers: what may be granted, plus what it is now. */
  function choicesFor(member: MemberView) {
    const current = { value: member.role, label: roleLabel(member.role) };
    return roleOptions.some((option) => option.value === member.role)
      ? roleOptions
      : [current, ...roleOptions];
  }

  return (
    <>
      <section className="settings-section" aria-labelledby="settings-server-members">
        <div className="settings-section-head">
          <h3 id="settings-server-members">{t("settings.server.membersTitle")}</h3>
          <p className="settings-hint">{t("settings.server.membersHint")}</p>
        </div>

        <ul className="settings-list">
          {members.map((member) => {
            // Manageable is not a field the API returns; what this account may
            // grant is the closest thing it says, and the two agree for every
            // role but Owner-as-target (where the server has a last-owner rule
            // of its own that it reports itself).
            const manage = grantableRoles.includes(member.role);
            const choices = choicesFor(member);
            const canChangeRole = manage && choices.length > 1;
            return (
              <li key={member.user_id} className="settings-card">
                <div className="settings-card-head">
                  <span className="settings-card-title">{member.name}</span>
                  <span className="settings-card-action settings-meta-inline">
                    {member.email}
                  </span>
                </div>
                <div className="settings-card-foot">
                  {canChangeRole ? (
                    <SelectMenu
                      value={member.role}
                      options={choices}
                      onChange={(value) => handleRole(member, value as Role)}
                      ariaLabel={t("settings.server.inviteRole")}
                      disabled={busy !== null}
                    />
                  ) : (
                    <span className="settings-badge">{roleLabel(member.role)}</span>
                  )}
                  {manage ? (
                    <button
                      type="button"
                      className="ghost-button danger-text"
                      onClick={() => handleRemove(member)}
                      disabled={busy !== null}
                    >
                      {busy === member.user_id
                        ? t("settings.server.memberRemoveBusy")
                        : t("settings.server.memberRemove")}
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
        {members.length === 0 ? (
          <p className="settings-note">{t("settings.server.membersEmpty")}</p>
        ) : null}

        <div className="form-actions">
          <button
            type="button"
            className="ghost-button"
            onClick={handleRefresh}
            disabled={busy !== null}
          >
            {busy === "refresh"
              ? t("settings.server.membersRefreshing")
              : t("settings.server.membersRefresh")}
          </button>
        </div>

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

      {grantableRoles.length > 0 ? (
        <section className="settings-section" aria-labelledby="settings-server-invites">
          <div className="settings-section-head">
            <h3 id="settings-server-invites">{t("settings.server.invitesTitle")}</h3>
            <p className="settings-hint">{t("settings.server.invitesHint")}</p>
          </div>

          <div className="settings-rule-form">
            <div className="field">
              <span>{t("settings.server.inviteRole")}</span>
              <SelectMenu
                value={inviteRole}
                options={roleOptions}
                onChange={(value) => setInviteRole(value as Role)}
                ariaLabel={t("settings.server.inviteRole")}
                disabled={busy !== null}
              />
            </div>
            <div className="form-actions">
              <button
                type="button"
                className="primary-button"
                onClick={handleCreateInvite}
                disabled={busy !== null}
              >
                {busy === "invite"
                  ? t("settings.server.inviteCreating")
                  : t("settings.server.inviteCreate")}
              </button>
            </div>
          </div>

          {granted ? (
            <div className="settings-active">
              <div className="settings-active-body">
                <p className="settings-meta-inline">{t("settings.server.inviteTokenLabel")}</p>
                <p className="settings-path">{granted.token}</p>
                <p className="settings-note">{t("settings.server.inviteTokenHint")}</p>
              </div>
              <button type="button" className="ghost-button" onClick={handleCopy}>
                {copied ? t("settings.server.inviteCopied") : t("settings.server.inviteCopy")}
              </button>
            </div>
          ) : null}

          {invites.length === 0 ? (
            <p className="settings-note">{t("settings.server.invitesEmpty")}</p>
          ) : (
            <ul className="settings-list">
              {invites.map((invite) => (
                <li key={invite.id} className="settings-card">
                  <div className="settings-card-foot">
                    <span className="settings-badge">{roleLabel(invite.role)}</span>
                    <span className="settings-meta-inline">{expiresOn(invite.expires_at)}</span>
                    <button
                      type="button"
                      className="ghost-button danger-text"
                      onClick={() => handleRevoke(invite)}
                      disabled={busy !== null}
                    >
                      {busy === invite.id
                        ? t("settings.server.inviteRevoking")
                        : t("settings.server.inviteRevoke")}
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      ) : null}
    </>
  );

  function expiresOn(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime())
      ? "—"
      : t("settings.server.inviteExpires", {
          date: date.toLocaleDateString(currentIntlLocale(), {
            year: "numeric",
            month: "short",
            day: "numeric",
          }),
        });
  }
}
