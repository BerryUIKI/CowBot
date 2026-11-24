# ADR 0063: Trunk-based development replaces GitFlow

- Status: Accepted
- Date: 2026-11-24
- Related: [ADR-0014](0014-desktop-release-builds.md), [ADR-0018](0018-desktop-auto-updates.md)
- Supersedes: the GitFlow convention recorded in `CLAUDE.md`, `docs/COLLABORATION.md`, and `docs/ROADMAP.md` section 8. Those were process notes rather than an accepted ADR, so nothing else is superseded.

## Context

The repository ran GitFlow: `main` held release-ready history, `dev` was the
integration branch, features merged into `dev`, and a `release/<version>` branch
was cut from `dev` and merged into both `main` and `dev` — with `hotfix/<name>`
starting from `main` and reaching both as well.

In practice the two long-lived branches stopped carrying different things. At the
0.1.17 release, `main` and `dev` pointed at the same commit
(`22f6ce0`), and `v0.1.17` was tagged on it. The second branch was not protecting
`main` from unfinished work; it was a duplicate that every landing had to be
promoted across, and every release paid for two merges per change.

GitFlow's costs are proportional to the number of long-lived branches and to how
long branches live. For a small, continuously released project with one
integration point, that is two extra merges per release and a second protected
branch whose state has to be reasoned about.

## Decision

Adopt trunk-based development with `main` as the trunk and the only long-lived
branch.

- All work is a short-lived branch created from `main` — convention
  `feature/<name>`, used for features, fixes, and documentation alike — pushed to
  origin and landed through a pull request against `main`
  (`gh pr create --base main`).
- `main` is always releasable. A release is a `v<version>` tag cut from a reviewed
  commit on `main`; the version bump lands as an ordinary short-lived branch and
  pull request, not a `release/*` branch.
- An urgent fix is an ordinary short-lived branch like any other. There is no
  `hotfix/*`.
- `dev`, `release/*`, and `hotfix/*` are retired. `dev` was deleted after
  confirming it was identical to `main`, and the repository's default branch is
  `main`.
- CI runs on pull requests to `main` and pushes to `main` only.

`main` stays protected: pull requests required, force-pushes and deletion
blocked, and the CI jobs (workspace and desktop) required before merge.

## Alternatives considered

- **Keep GitFlow and shorten branch lifetimes.** The release promotion cost stays,
  and the two branches drift apart again the moment a promotion is skipped. The
  problem was the branch that had no distinct job, not lifetime.
- **Keep `dev` as the trunk and retire `main`.** Equally workable, and it was the
  option that would have preserved the most existing muscle memory. Rejected
  because `main` is already the GitHub default branch, already carries the
  release tags, and is what external links, the landing page, and release tooling
  reference — pointing the trunk at it needed no repointing anywhere else.
- **Trunk-based with release branches cut at freeze time.** This is the variant
  GitFlow's `release/*` branch approximates, and it is the right shape when a
  release needs a stabilization window. CowBot cuts a tag on a commit that CI has
  already validated, so no freeze branch is needed today. If a release ever needs
  one, it belongs in a follow-up ADR rather than in a revival of the old process.

## Consequences

- One merge per change instead of two per release, and one protected branch whose
  state has to be reasoned about.
- `main` must stay green. A red trunk blocks everyone, so a failing check is
  fixed before new work starts rather than deferred to a release branch.
- Changes need enough test coverage to land behind a required CI run, because
  there is no integration branch to absorb a broken merge.
- Contributors branch from `main` and open pull requests against `main`;
  `README.md`, `CONTRIBUTING.md`, `docs/COLLABORATION.md`, `docs/DEVELOPMENT.md`,
  `docs/RELEASING.md`, `docs/ROADMAP.md`, the pull request template, and the
  landing page all state that.
- The release checklist no longer merges a release branch back. It confirms the
  reviewed version-bump commit is on `main` with a green run, then tags it.
- `SECURITY.md` and `docs/LIMITATIONS.md` name `main` as the branch under
  development.
