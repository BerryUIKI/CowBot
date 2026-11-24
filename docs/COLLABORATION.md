# Collaboration workflow

## Branches and pull requests

CowBot follows trunk-based development. `main` is the trunk and the only
long-lived branch; it is always releasable, and a release is a `v<version>` tag
cut from a reviewed commit on it.

| Branch | Start from | Merge into | Purpose |
| --- | --- | --- | --- |
| `main` | — | — | The trunk. Always releasable, protected |
| `feature/<name>` | `main` | `main` | Features, fixes, and documentation |

Branches are short-lived: create one per coherent change from current `main`,
push it to origin, and land it as a pull request against `main`
(`gh pr create --base main`). Merge `main` into the working branch daily so it
never drifts, and delete the branch once the pull request is merged.

There is no `dev`, `release/*`, or `hotfix/*` branch. Integration happens on
`main` through small pull requests, not on a second long-lived branch; an urgent
fix is an ordinary short-lived branch like any other. Contributors without write
access use forks. Do not force-push `main`; the repository conventions require a
reviewed pull request for it. Never push directly to `main`; see
[CLAUDE.md](../CLAUDE.md).

```sh
git fetch origin
git switch main
git pull --ff-only origin main
git switch -c feature/clarify-cli-docs
```

For a fork, fetch the canonical repository through an `upstream` remote and start
from `upstream/main`. If `main` is unavailable, ask maintainers in the issue which
base to use.

## Coordinate work

Use issues for problem statements, ownership, acceptance criteria, and design discussion. Link PRs to their issues. Keep scope reviewable and surface dependencies or blocking questions early. Record lasting architecture decisions in ADRs rather than leaving them only in comments.

Reviewers should verify that implemented features and planned capabilities are described separately. Pay special attention to model credentials, logs, command execution, and file access. A change that expands the trust boundary needs explicit design discussion.

## Definition of done

- The agreed behavior is implemented or the documentation objective is fulfilled.
- Relevant checks pass and the PR records what ran and what could not run.
- User-facing and developer documentation match the change.
- Compatibility and security implications are explained.
- Review discussions are resolved and the pull request targets `main`.

Maintainers handle merges, release coordination, and community moderation. No formal election process or service-level response commitment is established. See [Releasing](RELEASING.md) for the release procedure.
