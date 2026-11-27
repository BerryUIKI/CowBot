# ADR 0066: An invitation is a credential

- Status: Accepted
- Date: 2026-11-28
- Discussion: PR for `feature/desktop-members-and-invites`
- Related: [ADR-0043](0043-self-hosted-server-auth.md), [ADR-0044](0044-workspace-membership-and-roles.md), [ADR-0045](0045-collaboration-api-and-remote-client.md), [ADR-0053](0053-desktop-server-mode.md)

## Context

ADR-0044 shipped workspaces, roles, and invitations, and ADR-0045 shipped the
collaboration API and a `RemoteClient` that speaks it. Together they read as a
complete story: an owner invites someone, that person joins the workspace, and
the team collaborates. Phase 2's exit criterion says exactly that.

It was not true, for two reasons that were each invisible from inside their own
milestone.

**Nothing could create an account.** `AuthService::bootstrap` does, once, on an
empty database. Every other `create_user` call site is a local bootstrap, a
package import, or an integration principal. There is no sign-up route, no
server subcommand, and no CLI verb. A deployment therefore has exactly one
account: the owner's.

**The accept route needed a session.** `POST /api/v1/invites/accept` takes an
`AuthenticatedUser`. Someone without an account has no way to obtain one, so the
route was reachable only by people who did not need it. Neither `curl` nor the
API could get around this — there was no token to send.

So a workspace could mint invitations all day and hand them to people who could
not use them. The desktop compounded it: no `Client` member mentioned members or
invitations, so the Settings → Server screen could not show a roster or a code
even for the people who were already in.

## Decision

### 1. Redeeming an invitation creates the account

`POST /api/v1/invites/redeem` is public. It takes the invitation token plus a
name, email, and password, and answers with the same body as sign-in — session
token, user, and the workspaces the new account belongs to. The caller is signed
in and bound to the workspace by the one response.

The invitation is the credential. It is 256 random bits, single-use, and expires
in seven days; it arrives out of band, exactly where a password reset link would.
Anyone who has one was invited. Requiring something else as well — an operator
creating the account first — would make the invitation decorative and put a
person at a shell between two colleagues.

Everything that could go wrong is checked before anything is written: an unknown
or spent token is `404`, an expired one is `400` and is deleted, an email that
already has an account is `409`, and a password under ten characters is `400`.
The account, the membership, and the deletion of the invitation happen together;
the account's role is the invitation's role.

### 2. It shares sign-in's rate-limit bucket

Redeeming hashes a password on an unauthenticated request, which is the same
expensive thing sign-in does. Left unlimited it would be the cheaper of the two
ways to make a server spend CPU, so it counts against the same per-source
failure bucket (`AuthService::rate_limited` / `record_failure`, keyed on the
first `X-Forwarded-For` hop).

Failures that indicate guessing — unknown token, expired token, taken email —
are counted. A password below the minimum is not: that is a person typing, not
an attempt to find a token.

A successful redemption does not clear the bucket. Sign-in clears on success
because a legitimate user hitting the limit is usually mid-typo; here there is
nothing to reward, and letting one success reset a source's budget would hand
the limit to whoever holds a single valid code.

### 3. A 409 reveals that the email has an account

That is a small disclosure, and it is acceptable: reaching it requires a live
invitation in hand. The alternative — a generic failure — would leave the actual
case unhandled, someone who already has an account being invited to a workspace,
which is common once a team has more than one shared space.

That case has its own path: sign in and `POST /api/v1/invites/accept`, which
adds a workspace to an existing account. The desktop offers it beside the
redeem form, so the 409 is not a dead end.

### 4. The server states what a role may grant

`Role::grantable` answers "which roles may this one hand out", and every
workspace the API returns carries `grantable_roles` beside `role`.

This is deliberately server-side. The desktop's role picker needs the answer,
and deriving it there from `can_grant` would be a second copy of the policy on
the side that does not enforce it. The two would agree until one changed, and
then a picker would offer a role the route refuses.

The member panel has no "may I manage this person" flag either, and does not
invent one: it shows controls on a row whose current role is in `grantable_roles`,
which is exactly the set of people an admin may act on. Where the server refuses
anyway — demoting the last owner — its message is shown rather than predicted.

### 5. The desktop learns its own role

`ClientSession` now carries `role` and `grantable_roles`. `RemoteClient` had
been deserializing `/api/v1/me`'s workspaces and dropping `role` on the floor;
it now keeps both. A local workspace answers too, from the membership row
`bootstrap_local_named` writes, so one screen works in both modes and the panel
decides what to render from the roles rather than from the backend.

### 6. The member list is a snapshot

There is no membership event on the bus. Adding one would mean a payload
carrying its own workspace id, so `stream.rs`'s `Scope::admit` could place it —
a small extension to a filter that deliberately drops anything it cannot place.

Not now. A small team's roster changes rarely, the panel is a settings screen
rather than a live surface, and a refresh button costs nothing. Recorded as a
limitation rather than a plan.

## Alternatives considered

- **An operator CLI verb that creates accounts.** Keeps the server closed and
  puts a person at a shell between two colleagues every time a team grows. Also
  leaves the invitation itself with no job.
- **Open self-registration.** Turns a private deployment into a public sign-up
  form, which is a different product.
- **Split `sign_in` into "authenticate" and "bind to a workspace"** so an account
  with no workspace can sign in and then paste a code. Correct in shape, but it
  changes three call sites (`probe`, `resume`, `connect`) to tolerate a zero
  workspace list, and the desktop would still need a screen for "you belong to
  nothing yet". The redeem route adds a door instead of widening an existing one.
- **Derive `grantable_roles` in the desktop.** One more place for the policy to
  live, on the wrong side of the boundary.
- **Notify members over the live stream.** See §6.

## Consequences

- **A deployment can now gain accounts without an operator.** That is the point,
  and it makes the invitation token a security-relevant secret: whoever holds it
  gets an account at the invited role. Its length, single use, and expiry are
  what bound the damage, and revoking one from Settings takes effect immediately.
- **No mail is sent.** The code travels however the inviter chooses to send it.
  A deployment that wants an approval step between invitation and account has no
  place to put one.
- **An invitation's token exists in exactly two places**: the response that
  created it and whatever the inviter pasted it into. The server keeps a hash,
  so a leaked database does not yield working codes and a listed invitation
  cannot be re-shared from the app.
- **The desktop can manage membership but not create a workspace.** Creating one
  is still an API or owner-side operation; the panel edits the workspace the
  device has open.
- **A profile edit in server mode is still missing.** ADR-0053's consequence
  stands; this ADR does not touch it.
