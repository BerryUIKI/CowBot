# Contributing to CowBot

Contributions to documentation, bug fixes, tests, and design discussions are welcome. Read the [README](README.md) for current scope and the [Code of Conduct](CODE_OF_CONDUCT.md) for community expectations.

## Before starting

Search existing [issues](https://github.com/BerryUIKI/CowBot/issues) and PRs. For a small correction, open a focused PR directly. For a new capability or architectural change, open an issue explaining the problem, proposed behavior, alternatives, and affected roadmap milestone. Follow the agreed Phase 1 design and discuss changes to scope or architecture before implementing new capabilities.

Do not disclose vulnerabilities in public issues; follow [SECURITY.md](SECURITY.md).

## Make a change

1. Fork the repository if you do not have write access and clone your fork.
2. Fetch upstream and create a short-lived branch from `main`, the trunk. Follow [the collaboration guide](docs/COLLABORATION.md) for the trunk-based conventions.
3. Set up the environment with [the developer guide](docs/DEVELOPMENT.md).
4. Make the smallest coherent change. Update docs and meaningful tests when behavior changes; avoid unrelated refactors or lockfile churn.
5. Run formatting, Clippy, and workspace tests for code changes. For docs, check local links, command examples, and visual assets.
6. Open a PR against `main`, linking the issue and explaining the problem, resulting behavior, validation, and limitations. Use the PR template.
7. Respond to review, resolve discussion, and rerun affected checks after edits. Maintainers decide when to merge.

Write code, comments, documentation, branch names, and commit messages in English. Use concise imperative commits such as `docs: clarify mock provider behavior` or `fix: handle missing tool arguments`. Keep commits small and atomic.

## Review expectations

A review should assess correctness, compatibility, failure paths, security boundaries, and whether tests demonstrate the intended behavior. Clearly distinguish blocking findings from suggestions. Explain why a change is needed and keep feedback about the work. Draft PRs are useful for early feedback; mark unfinished work and unresolved design questions explicitly.

AI-assisted contributions follow the same review and validation requirements. Contributors remain responsible for correctness, licensing, and protecting private data; describe meaningful generated changes when that helps review.

## Licensing

By intentionally submitting a contribution for inclusion, you agree that it is provided under the project's [Apache-2.0 license](LICENSE), unless explicitly stated otherwise in accordance with that license. Only contribute material you have the right to submit. Preserve third-party attribution and identify any added dependency or borrowed asset and its license.

## Finding useful work

Improve a reproducible bug report, clarify a setup step, add a regression test, review draft designs, or propose a small roadmap task. Issue labels and available assignments may vary; discuss ownership in the issue to avoid duplicated work. No response or merge-time guarantee is currently published.
