# Releasing Rival

Rival has no hosted CI. GitHub Actions is turned off for `1905/rival`, and the
repository has no workflow files. Checks and releases run locally:

- `scripts/check.sh` (`make cli-check`) is the pre-merge gate. It runs on
  Linux or macOS.
- `scripts/release-local.sh` (`make cli-release-check`, `make cli-snapshot`,
  `make cli-publish`) builds and publishes a release. It runs on an arm64
  macOS host, because the darwin targets need the Apple SDK. The maintainer
  uses the MacBook Air (`ssh air`).

A release publishes, from one GoReleaser run (`.goreleaser.yaml` at the
repository root):

- the GitHub release with `rival_{darwin,linux}_{amd64,arm64}.tar.gz`,
  `rival_windows_{amd64,arm64}.zip` and `checksums.txt`;
- `rival.rb` at the root of `1905/homebrew-tap` (commit message
  `Brew formula update for rival version v<version>`).

cargo-zigbuild builds darwin and linux; cargo-xwin builds Windows. The pinned
tool versions are at the top of `scripts/release-local.sh`.

## One-time host setup

On the macOS host, from a checkout of the repository:

```bash
scripts/release-local.sh setup
```

It installs Zig, GoReleaser, cargo-zigbuild and cargo-xwin under
`~/tmp-rival-release/tools` (override with `RIVAL_RELEASE_TOOLS`), the pinned
Rust toolchain with the six targets, and Homebrew `llvm` and `lld`. The
Command Line Tools SDK is enough; Xcode is not needed.

## Release checklist

Set the version without the `v` prefix:

```bash
VERSION=5.2.0
```

1. Update all embedded skill versions:

   ```bash
   ./scripts/bump-skill-versions.sh "$VERSION"
   ```

2. Run the pre-merge gate on the release commit:

   ```bash
   make cli-check
   ```

   It must end with `CHECK OK`. It runs fmt, strict Clippy, the workspace
   tests, the startup fd check, the release-script tests (PyYAML in `.venv`),
   the fixture check, the name check and the e2e scenarios.

3. Run the unpublished snapshot on the macOS host, from a checkout of the
   release commit:

   ```bash
   make cli-release-check
   make cli-snapshot
   ```

   `cli-snapshot` builds the six archives and `dist/homebrew/rival.rb`, then
   `scripts/check_release_archives.py` checks checksums, binary format and
   CPU, bundled files and the formula, and runs the binary built for the
   host. It publishes nothing.

4. Update `CHANGELOG.md`, review and commit the complete release state, then
   create a lightweight tag on that commit:

   ```bash
   git status --short
   git diff --check
   git commit -am "release: v${VERSION}"
   git tag "v${VERSION}"
   ```

   Commits must use the `1905` GitHub noreply address
   (`32327548+1905@users.noreply.github.com`).

5. Push the release commit and the tag:

   ```bash
   git push origin master
   git push origin "v${VERSION}"
   ```

   Nothing runs on the push.

6. Publish from a clean checkout of the tag on the macOS host:

   ```bash
   git checkout "v${VERSION}"
   GITHUB_TOKEN=<1905 token> make cli-publish
   ```

   The script refuses a checkout with changes or a `HEAD` that is not at a
   `v*` tag. Instead of `GITHUB_TOKEN`, the token can come as one line on
   stdin (for example `gh auth token --user 1905 | ssh air '…'`), so it is
   not on a command line. `HOMEBREW_TAP_TOKEN` defaults to the same token.
   Never publish one tag twice: a second run collides with the uploaded
   assets.

7. Verify the published release:

   ```bash
   gh release view "v${VERSION}"
   ```

   It must list the six archives and `checksums.txt`. `1905/homebrew-tap`
   must have the `Brew formula update for rival version v${VERSION}` commit.

8. Verify the user installation after the tap update:

   ```bash
   brew update
   brew upgrade rival      # or: brew install 1905/tap/rival
   rival install --force
   which -a rival
   rival version
   ```

   The reported version must match `v${VERSION}`. If it does not, an older
   `rival` earlier on `PATH` (for example `~/.local/bin/rival`) shadows the
   Homebrew one. If Homebrew refuses the tap, run `brew trust 1905/tap`.
   Restart or reload Claude Code after refreshing the embedded skills.

## History

v5.1.0 (2026-10-12) was the first release published from the MacBook Air,
while GitHub Actions was blocked by billing. Hosted CI was removed after it.
v5.0.0 and earlier were published by the removed `release` workflow.
Rival.app (the Swift macOS viewer) was removed after v5.2.0; its last
published build is the `rival-app` cask from v5.0.0.
