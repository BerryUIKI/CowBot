# ADR 0044: Workspace membership, role enforcement, and invitations

- Status: Accepted
- Date: 2026-11-14
- Discussion: Phase 2 milestone 2.2 in [the roadmap](../ROADMAP.md).
- Supersedes: None. Extends [ADR-0043](0043-self-hosted-server-auth.md), which built the account surface without any workspace route.

## Context

ADR-0043 shipped sign-in and sessions but no route that touches a workspace.
`memberships` has carried a role since the initial schema, and nothing had ever
read it for a decision: the local runtime has one user and no permission to
check. Milestone 2.2 is where that changes — a workspace endpoint that any
member can call is only safe if the server can answer "what may this person
do here", consistently and in one place.

## Decision

### 1. A role policy with one small API

`Role` in `cowbot-core` gains the questions the server asks, next to the data
it already held:

| Method | Owner | Admin | Member |
| --- | --- | --- | --- |
| `can_write` | yes | yes | yes |
| `can_invite` | yes | yes | no |
| `can_administer` | yes | yes | no |
| `can_transfer_ownership` | yes | no | no |
| `can_manage(target)` | anyone | Member only | nobody |
| `can_grant(role)` | any role | Member only | nobody |

Two rules are deliberately stricter than a rank comparison:

- **An Admin cannot promote a peer.** Equal rank would allow it, but peer
  promotion lets one admin escalate another without an owner's involvement.
- **Only an Owner grants Owner.** An Admin's ceiling is Member.

The policy is on `Role`, not in the server, so a desktop or CLI that later
enforces the same roles uses the same answers rather than a second copy.

### 2. A non-member sees `404`, not `403`

Every workspace route resolves the caller's membership first. No membership
means `404 Not Found` — the same response a workspace id that never existed
gets. A `403` would tell an unauthenticated probe that the workspace exists,
which lets an attacker map a deployment by id. `403` is reserved for a member
whose *role* is insufficient, and that fact is not secret from them.

### 3. The last owner is protected at the boundary, not by luck

Demoting or removing the last Owner is a `409 Conflict`. `count_owners` runs in
the store rather than a cached count, so two concurrent demotions cannot both
observe "two owners exist". The workspace always keeps someone who can
administer it; a second Owner can be added first, then the first can step
down, which the tests cover.

### 4. Invitations are single-use, hashed, and role-bounded

An invite is a random 256-bit token, stored only as a SHA-256 hash, valid for
seven days, and deleted the moment it is accepted. Accepting marks the
`member.joined` audit event. The listing endpoint never returns a token or a
hash — only the id, role, and expiry — so an admin reviewing pending invites
cannot leak one.

Creating an invite applies the same `can_grant` rule as changing a role: an
Admin can invite Members, not Admins or Owners. Revoking checks that the invite
belongs to the workspace in the path, so an id from another workspace cannot be
deleted.

### 5. Membership changes are audited as human actions

`member.role_changed`, `member.removed`, `member.joined`, `invite.created`,
`invite.revoked`, and `workspace.renamed` append `AuditEvent` rows with
`source: "human"` and the acting user id. The audit table already existed; this
is the first writer of it from the server. An API-token route (2.3) would record
the token as the actor instead.

### 6. Workspace creation and renaming

Any signed-in user may create a workspace and becomes its Owner — a deployment
with one server should let a user start their own team, and the alternative
(an operator-only endpoint) would need a second bootstrap credential. Renaming
requires `can_administer` (Owner or Admin).

## Alternatives considered

**Rank-only permission checks.** Simpler, but `rank(admin) <= rank(admin)` would
let an admin create another admin, and a single compromised admin account would
become able to clone itself. The explicit `can_grant` table is one line more and
removes that.

**`403` for non-members.** Conventional, and leaks workspace existence to
anyone holding a token.

**Put the role check in an extractor.** A `WorkspaceMember` extractor is
tempting, but the path parameter names the workspace in several different
positions (`/workspaces/{id}`, `/workspaces/{id}/members/{user}`), and the
extractor would have to know each route's shape. An explicit `require_member`
call at the top of each handler is easier to audit: every handler opens with
the same line, and a missing one is visible in review.

**Let an Admin invite an Admin.** "An admin can bring in a peer" is a common
policy, but it makes role escalation a two-step operation for one compromised
account: invite yourself as Admin from a new email, then act as an Admin
reviewer. Requiring an Owner breaks the chain.

## Consequences

- Every workspace route is one `require_member` call plus zero or one role
  question. Adding a route means adding that line; the tests exercise the
  negative case for each route that exists.
- `cowbot-store` gained `set_member_role`, `remove_member`, `count_owners`,
  `list_invites`, `rename_workspace`, and `set_invite_expiry`. The last exists
  so the expiry path can be tested without waiting a week, and is a reasonable
  operation for an admin shortening an invite later.
- Invites are bearer tokens with no email binding: whoever holds the token
  joins. That is the same trade-off as any share link, and a deployment that
  needs stricter control can keep the invite window short or revoke it.
- Channel, task, and approval routes are 2.2's remaining work; they reuse
  `require_member` and add their own `can_write` / `can_administer` checks.
- The desktop does not speak this API yet (2.3). Until then the routes are
  exercised by the test suite.
