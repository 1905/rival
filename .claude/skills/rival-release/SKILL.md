---
name: rival-release
version: 3.11.0
description: Release rival — bump skills, commit, tag, push; CI (goreleaser) publishes the GitHub release + brew formula. Then reinstall, verify, notify. Use when user says "release" or "commit push release".
argument-hint: "<version>"
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# Rival Release

Release pipeline for the rival Go binary. **CI is the only publisher** — the
`.github/workflows/release.yml` workflow runs goreleaser on every `v*` tag push
and publishes the GitHub release, build artifacts, **and** the Homebrew formula
(it has the `HOMEBREW_TAP_TOKEN` secret). Do **NOT** run goreleaser locally — a
second goreleaser run on the same tag collides with the CI-published assets
(`422 already_exists`) and turns CI red. The skill's job is to prepare the tag,
then wait for CI and verify.

## Steps

1. **Bump skill versions** — run `./scripts/bump-skill-versions.sh <version>` to
   update all embedded SKILL.md files in `rival/internal/skills/`.
2. **Build + test** — `cd rival && make test` (lint + race tests + build). Must be
   green before tagging.
3. **Commit** all pending changes (skills, code, README, docs).
4. **Tag** `v<version>` on the release commit (`git tag v<version>`).
5. **Push** `master` + the tag to origin (`git push origin master && git push origin v<version>`).
   This push triggers the Release workflow.
6. **Watch CI** — `gh run watch` (or `gh run list --workflow=Release`). The
   goreleaser job builds 4 platforms, publishes the GitHub release + assets, and
   pushes the updated formula to `1905/homebrew-tap` (`rival.rb` at root).
   - If CI fails, inspect with `gh run view --log-failed`, fix, re-tag if needed.
7. **Verify release published** — `gh release view v<version>` should list the 4
   tarballs + checksums. Confirm `1905/homebrew-tap` got a "Brew formula update
   for rival version v<version>" commit.
8. **Brew reinstall** — `brew update && brew uninstall rival && brew install 1905/tap/rival`.
9. **Install skills** — `rival install --force`.
10. **Verify** — `rival version` should show the new version.
11. **Notify** — send a Telegram notification via `/notify` with version + changelog summary.

## Notes
- goreleaser config (`rival/.goreleaser.yaml`) uses `brews:` (deprecated but the
  correct artifact for this tap — a Formula at root, not a Cask). Do not switch
  to `homebrew_casks:` without migrating the whole tap.
- The brew formula is now updated **by CI**, not by hand. No more manual
  `rival.rb` edits, no more "403 is expected" — that was the old local-goreleaser
  flow.
