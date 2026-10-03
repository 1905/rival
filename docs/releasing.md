# Releasing Rival

Rival's GitHub Actions release workflow (`.github/workflows/release.yml`) is
the only publisher. A pushed `v*` tag runs two jobs:

- `release` (runs on `macos-15`): GoReleaser (`.goreleaser.yaml` at the
  repository root) builds the Rust CLI for six targets, creates the GitHub
  release, uploads `rival_{darwin,linux}_{amd64,arm64}.tar.gz`,
  `rival_windows_{amd64,arm64}.zip` and `checksums.txt`, and updates `rival.rb`
  in `1905/homebrew-tap`. cargo-zigbuild builds darwin and linux; cargo-xwin
  builds Windows. The pinned tools are in
  `.github/actions/cli-release-tools/action.yml`.
- `app` (needs `release`, runs on `macos-15`): runs `swift test` in `app/`,
  builds a universal, ad-hoc signed `Rival.app` with
  `app/scripts/bundle.py --version <tag without v>`, uploads
  `app/dist/Rival-app.zip` to the same release, and renders
  `Casks/rival-app.rb` into `1905/homebrew-tap` with
  `app/scripts/render_cask.py`. Homebrew loads casks only from `Casks/`.
  It skips the tap commit when the cask did not change.

Do not run `goreleaser release` locally for the same tag. A second publisher
collides with the CI-created assets and can leave an otherwise valid release
workflow marked failed.

## Unpublished snapshot

A manual run of the same workflow publishes nothing. It has no inputs, only
read permission and no tap token:

```bash
gh workflow run release.yml --ref <branch>
```

- `snapshot`: `goreleaser release --snapshot --clean --parallelism 1` builds
  the six archives and renders `dist/homebrew/rival.rb`.
  `scripts/check_release_archives.py` checks checksums, binary format and CPU,
  bundled files and the formula, and runs the binary built for the build
  host's OS and CPU. The archives, `checksums.txt`, `metadata.json`,
  `artifacts.json` and `homebrew/rival.rb` are uploaded as the run artifact
  `rival-snapshot`.
- `snapshot-native`: one job per archive on a runner of that OS and CPU runs
  the packaged `rival version` with a private home.

The `release` and `app` jobs run only for a pushed `v*` tag.

## Release checklist

Set the version without the `v` prefix:

```bash
VERSION=3.23.0
```

1. Update all embedded skill versions:

   ```bash
   ./scripts/bump-skill-versions.sh "$VERSION"
   ```

2. Run the release gate before creating a tag:

   ```bash
   make cli-test
   make cli-release-check
   ```

   `make cli-test` runs the Rust workspace tests and the release-script tests.
   `make cli-release-check` validates `.goreleaser.yaml`. The release workflow
   itself builds artifacts but does not duplicate this test gate.

   The candidate commit must also pass the full `CI` workflow on macOS,
   Linux and Windows. This includes strict Clippy, CLI scenarios and Swift
   session decoding. Do not tag a commit whose required checks are pending.

   Run the [unpublished snapshot](#unpublished-snapshot) on the release branch
   before the first Rust release, and whenever `.goreleaser.yaml`,
   `release.yml`, `.github/actions/cli-release-tools/`, the toolchain or the
   dependencies changed. `snapshot` and all six `snapshot-native` jobs must
   pass. Download the artifact and read the rendered formula:

   ```bash
   gh run download <run-id> -n rival-snapshot -D "/tmp/rival-snapshot-${VERSION}"
   ```

3. Review and commit the complete release state, then create a lightweight tag
   on that commit:

   ```bash
   git status --short
   git diff --check
   git add -A
   git diff --cached --check
   git diff --cached --stat
   git commit -m "release: v${VERSION}"
   git tag "v${VERSION}"
   ```

   Do not proceed with unrelated worktree changes. `git add -A` is appropriate
   only after the status review confirms that the complete tree is intended for
   this release.

4. Push the release commit and tag as `1905`. Commits must use the `1905`
   GitHub noreply address (`git config user.email`). An HTTPS token without
   the `workflow` scope cannot push `.github/workflows/` changes, so push over
   SSH with the maintainer's key for the `1905` account selected explicitly:

   ```bash
   export GIT_SSH_COMMAND="ssh -o BatchMode=yes -o IdentitiesOnly=yes -i <path-to-1905-key>"
   git push git@github.com:1905/rival.git master
   git push git@github.com:1905/rival.git "v${VERSION}"
   ```

5. Watch the `Release` workflow for that tag:

   ```bash
   gh run list --workflow Release --limit 5
   gh run watch <run-id>
   ```

   Select the run whose event/tag corresponds to `v${VERSION}`.

6. Verify the published release:

   ```bash
   gh release view "v${VERSION}"
   ```

   It must contain archives for Darwin, Linux and Windows on both amd64 and
   arm64, `checksums.txt`, and `Rival-app.zip`. Also confirm that `1905/homebrew-tap`
   has commits for the same tag that update both `rival.rb` and
   `Casks/rival-app.rb` (the app commit message is
   `Cask update for rival-app version v${VERSION}`).

7. Verify the user installation after the tap update:

   ```bash
   brew update
   brew uninstall rival
   brew install 1905/tap/rival
   rival install --force
   rival version
   ```

   If Rival was not already installed, omit the uninstall command. The reported
   version must match `v${VERSION}`. Restart or reload Claude Code after
   refreshing the embedded skills.

8. Verify the app.

   Before tagging, check the app locally from the release commit, in the
   repository root (not `rival/`):

   ```bash
   make test       # swift test in app/
   make install    # native release build into /Applications/Rival.app
   ```

   `make install` moves an existing `/Applications/Rival.app` to `/tmp/trash`
   and opens the new build. Confirm the menu bar icon appears and the window
   lists your runs.

   After the `app` job finishes, verify the cask:

   ```bash
   brew update
   brew install --cask 1905/tap/rival-app    # or: brew upgrade --cask rival-app
   ```

   The app is ad-hoc signed, not notarized. The cask removes the quarantine
   flag in `postflight`. If Homebrew refuses the tap, run
   `brew trust --tap 1905/tap` and retry.
