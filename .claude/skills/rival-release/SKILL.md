---
name: rival-release
version: 3.11.0
description: Release rival — bump skills, commit, tag, push; CI (goreleaser) publishes the GitHub release + brew formula. Then reinstall, verify, notify. Use when user says "release" or "commit push release".
argument-hint: "<version>"
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# Rival Release

Release pipeline for the rival Rust CLI and Rival.app. **CI is the only
publisher** — the `.github/workflows/release.yml` workflow runs goreleaser on
every `v*` tag push and publishes the GitHub release, the six CLI archives,
**and** the Homebrew formula (it has the `HOMEBREW_TAP_TOKEN` secret), then the
`app` job publishes Rival.app and its cask. Do **NOT** run `goreleaser release`
locally — a second goreleaser run on the same tag collides with the
CI-published assets (`422 already_exists`) and turns CI red. The skill's job is
to prepare the tag, then wait for CI and verify. Full guide: `docs/releasing.md`.

## Steps

1. **Bump skill versions** — run `./scripts/bump-skill-versions.sh <version>` to
   update the embedded SKILL.md files (`crates/rival-core/skills/`).
2. **Build + test** — from the repository root:
   create/activate a local Python venv and install `scripts/requirements-test.txt`.
   `make cli-test && make cli-release-check` (Rust workspace tests, release
   script tests, `goreleaser check`). Must be green before tagging.
   Require the candidate commit's full three-OS `CI` workflow to pass too,
   including strict Clippy, CLI scenarios and Swift session decoding.
3. **Snapshot gate** — required for the first Rust release and whenever
   `.goreleaser.yaml`, `release.yml`, `.github/actions/cli-release-tools/`,
   the toolchain or dependencies changed. A manual run publishes nothing:
   `gh workflow run release.yml --ref <branch>`, then `gh run watch <run-id>`.
   `snapshot` and all six `snapshot-native` jobs must be green. Download the
   artifact with
   `gh run download <run-id> -n rival-snapshot -D /tmp/rival-snapshot-<version>`
   and read `homebrew/rival.rb` (four darwin/linux URLs, `bin.install "rival"`).
4. **Commit** all pending changes (skills, code, README, docs). Confirm
   `git config user.email` is `32327548+1905@users.noreply.github.com` first.
5. **Tag** `v<version>` on the release commit (`git tag v<version>`).
6. **Push** `master` + the tag over SSH with the explicit `1905` key
   (`/Users/kass/ssh/github-kass`; the HTTPS token lacks the `workflow` scope):
   ```bash
   export GIT_SSH_COMMAND="ssh -o BatchMode=yes -o IdentitiesOnly=yes -i /Users/kass/ssh/github-kass"
   git push git@github.com:1905/rival.git master
   git push git@github.com:1905/rival.git v<version>
   ```
   This push triggers the Release workflow.
7. **Watch CI** — `gh run watch` (or `gh run list --workflow=Release`). The
   `release` job (macos-15) builds six targets — darwin/linux with
   cargo-zigbuild, windows with cargo-xwin — publishes the GitHub release +
   assets, and pushes the updated formula to `1905/homebrew-tap` (`rival.rb` at
   root). The `app` job then uploads Rival.app and updates `Casks/rival-app.rb`.
   - If CI fails, inspect with `gh run view --log-failed`, fix, re-tag if needed.
8. **Verify release published** — `gh release view v<version>` should list
   `rival_{darwin,linux}_{amd64,arm64}.tar.gz`,
   `rival_windows_{amd64,arm64}.zip`, `checksums.txt`, `Rival-app.zip` and the
   DMG. Confirm `1905/homebrew-tap` got a "Brew formula update for rival
   version v<version>" commit and a "Cask update for rival-app version
   v<version>" commit.
9. **Brew reinstall** — `brew update && brew uninstall rival && brew install 1905/tap/rival`.
10. **Install skills** — `rival install --force`.
11. **Verify** — `rival version` should show the new version (`which -a rival`
    if it looks old: a dev build can shadow brew's binary).
12. **Notify** — send a Telegram notification via `/notify` with version + changelog summary.

## Notes
- goreleaser config (`.goreleaser.yaml` at the repository root) uses `brews:`
  (deprecated but the correct artifact for this tap — a Formula at root, not a
  Cask). Do not switch to `homebrew_casks:` without migrating the whole tap.
- Pinned release tools (Rust, Zig, cargo-zigbuild, cargo-xwin, GoReleaser) are
  in `.github/actions/cli-release-tools/action.yml`.
- The brew formula is updated **by CI**, not by hand. No manual `rival.rb`
  edits.
