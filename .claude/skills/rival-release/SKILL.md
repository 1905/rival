---
name: rival-release
version: 5.1.0
description: Release rival — bump skills, run the local gate, tag, push, then publish the GitHub release + brew formula from the MacBook Air with scripts/release-local.sh. Then reinstall, verify, notify. Use when user says "release" or "commit push release".
argument-hint: "<version>"
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# Rival Release

Rival has **no hosted CI**: GitHub Actions is off for `1905/rival` and the repo
has no workflow files. You publish the release yourself, from the MacBook Air
(`ssh air`, arm64 macOS, Command Line Tools SDK), with
`scripts/release-local.sh`. One GoReleaser run publishes the GitHub release
(six CLI archives + `checksums.txt`) and pushes `rival.rb` to
`1905/homebrew-tap`. Rival.app is not published. Full guide:
`docs/releasing.md`.

Run the Rust gate on the Dell. Build release archives only on the Air. Never
run `release` twice for one tag: the second run collides with the uploaded
assets. Never move a published tag.

## Steps

1. **Bump skill versions** — `./scripts/bump-skill-versions.sh <version>`
   (updates `crates/rival-core/skills/*/SKILL.md`).
2. **Gate (Dell)** — `make cli-check`. It must end with `CHECK OK` (fmt,
   strict Clippy, workspace tests, startup fds, release-script tests,
   fixtures, name check, e2e scenarios). `executor::claude::tests::proxy_key_never_reaches_the_log_mirror_or_record`
   is a known flaky test; re-run once before you call a failure real.
3. **Changelog** — move `[Unreleased]` to `[v<version>] — <date>` and update
   the "Latest binary release" line.
4. **Commit** as `1905`: `git -c user.email=32327548+1905@users.noreply.github.com commit -am "release: v<version>"`.
   No AI co-author trailers.
5. **Snapshot (Air)** — sync the checkout and build without publishing:
   ```bash
   rsync -a --delete --exclude .venv --exclude target --exclude dist ./ air:tmp-rival-release/src/
   ssh air 'cd ~/tmp-rival-release/src && scripts/release-local.sh check && scripts/release-local.sh snapshot'
   ```
   Six `ok` lines plus `ok homebrew/rival.rb` and `RELEASE_LOCAL_OK snapshot`.
   First time on a host: `scripts/release-local.sh setup`. A full build takes
   about 12 minutes; run it in the background.
6. **Tag and push** — `git tag v<version>`, `git push origin master`,
   `git push origin v<version>`. Nothing runs on the push.
7. **Publish (Air)** — sync again (now with the tag), then pass the token on
   stdin so it is never on a command line:
   ```bash
   rsync -a --delete --exclude .venv --exclude target --exclude dist ./ air:tmp-rival-release/src/
   gh auth token --user 1905 | ssh air 'cd ~/tmp-rival-release/src && scripts/release-local.sh release'
   ```
   Expect `release published` and `pushing repository=1905/homebrew-tap`.
8. **Verify** — `gh release view v<version>` lists the six archives and
   `checksums.txt`; `1905/homebrew-tap` has `Brew formula update for rival
   version v<version>`.
9. **Install** — on the Air: `brew update && brew upgrade rival` (or
   `brew install 1905/tap/rival`), `rival version`. On the Dell (no brew):
   download `rival_linux_amd64.tar.gz` + `checksums.txt` with
   `gh release download`, check the hash, move the old `~/.local/bin/rival` to
   `/wrk/trash/`, install the new one, then `rival install --force`.
   If `rival version` is old, run `which -a rival`: a dev build earlier on
   PATH shadows the brew one.
10. **Notify** — send a Telegram notification via `/notify` with the version
    and a changelog summary.
