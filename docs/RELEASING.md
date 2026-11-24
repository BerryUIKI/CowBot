# Release checklist

This is a maintainer procedure, not a claim that automated releases or published packages exist.

1. Confirm the agreed milestone and release scope; verify documentation against implemented behavior. The release is cut from `main`, so nothing is pending on another branch.
2. Bump the version on a short-lived `feature/release-<version>` branch from `main` and land it through the usual pull request. Update the workspace version, the desktop package version, and the Tauri crate and config versions together; the CLI reads `CARGO_PKG_VERSION`. Regenerate the lockfiles only as needed.
3. Write release notes covering changes, compatibility, known limitations, upgrade steps, and — for desktop releases — the update path. Do not mark planned work as shipped. Save the published notes as `docs/releases/<version>.md` and add them to the `docs/releases/README.md` index.
4. Confirm the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repository secrets exist and that an offline backup of both is available. The private key never enters the repository. Without the secret the release build fails; losing the key or password prevents installed clients from receiving automatic updates.
5. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, and `cargo build --release --workspace --locked` on the intended supported platforms. Run desktop frontend and standalone Tauri checks separately when shipping the desktop; root workspace gates exclude it. Record actual results and platform coverage.
6. Verify `cowbot --help`, mock execution, license metadata, and the included license. Inspect any distribution contents for secrets and required third-party notices.
7. Confirm the reviewed version-bump commit is on `main` with a green CI run, then create the `v<version>` tag on that commit and publish release notes when authorized. Nothing is merged back: `main` is the only long-lived branch.
8. For desktop releases, wait for the release workflow to finish, then review the draft release before publishing:
   - The tag matches `desktop/src-tauri/tauri.conf.json`.
   - `latest.json` is attached and lists `windows-x86_64`, `darwin-aarch64`, `darwin-x86_64`, `darwin-universal`, and `linux-x86_64`.
   - `.sig` assets exist for the NSIS setup, the macOS `.app.tar.gz`, and the AppImage, and the referenced artifacts are attached.
   - The release notes explain the update behavior and any manual step (MSI and deb installs, users upgrading from a version without the updater).
9. After publishing, confirm `https://github.com/BerryUIKI/CowBot/releases/latest/download/latest.json` returns the new version. With the previous release installed, confirm Settings → About offers the new version and installs it.
10. If distributing binaries, state the platform/toolchain, include license information, and publish checksums. Publish to a registry only after reviewing crate metadata, package contents, and dependency publishability.
11. Check quick-start links and installation instructions against the actual release artifacts.

Do not silently replace a published tag or artifact. Correct release problems through a documented follow-up release or a short-lived fix branch merged into `main` like any other change. Until a support policy is established, do not promise maintenance of older versions.
