# Codex skill integration — 2026-09-06

## Sources and decisions

- [OpenAI: Build skills](https://learn.chatgpt.com/docs/build-skills): the current
  user skill location is `~/.agents/skills`; Codex supports a `SKILL.md` with
  name and description. Avoid installing a second copy in `~/.codex/skills`,
  because duplicate names are not merged.
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference):
  `--tools` limits built-in tool availability; `--allowedTools` only grants
  permission. MCP tools require separate restrictions. `--safe-mode` suppresses
  customizations while preserving authentication, unlike `--bare`, whose local
  help explicitly excludes subscription/keychain authentication.
- Local Claude Code 2.1.263 help and a live Opus 5.5 run verified the selected
  flags. Native review restrictions use Read/Glob/Grep, dontAsk, no MCP servers,
  safe mode, empty settings sources, and disabled hooks. Raw prompts retain
  their previous full-auto behavior. Docker reviews also mount the repo `:ro`.

The original assumption that copying the Claude skill was enough was wrong:
Claude skills require Bash's `run_in_background` and a completion notification.
Codex instructions instead use a detached Rival process and a resumable wait,
keeping the turn active until the output has been delivered. Both hosts use
Rival's command parsers and model defaults; Codex variants inherit each embedded
skill's release version and share one host execution template.

Auto installation preserves Claude installation and adds Codex when its CLI,
configuration directory, or macOS app exists. `--target` overrides detection.
A custom CODEX_HOME is a detection signal, not a second skill destination.
The updater must execute the new Homebrew binary after upgrading: the running
old process still holds the old embedded skills. An already-current update
also refreshes skills for hosts installed since the last release.

## Verification

- Target selection, both-host installs, overwrite protection, buffered update
  confirmations, same-version refresh, unrelated-skill preservation, and
  invoking the new Homebrew binary are covered by regression tests.
- Native review/plan transport tests check restricted tools, absence
  of permission bypass, removal of nested-session and API-key environment
  variables, and preservation of raw mode. Docker transport checks the read-only
  mount and tool restrictions.
- Codex CLI 0.153.4 app-server `skills/list` discovered all 11 installed Rival
  skills, enabled, with no Rival parsing errors. The skill-creator validator
  accepted the installed Claude skill.
- A detached live Opus 5.5 review through Rival completed in 15 seconds using
  subscription authentication. It found the planted unconditional authorization
  bypass in `access.py` and reported only Read, Glob, and Grep tools.
  `rival wait` returned exit 0 and the full output reached this Codex session.
  Git status stayed clean; project SessionStart hook and MCP-server marker
  files were absent.

## Limits

The native restriction is enforced by the Claude CLI tool interface, not an
OS sandbox around the CLI process. Managed policy remains the administrator's
trust boundary. Reviews cannot execute tests or git themselves; Rival resolves
changed-file scope first. The live proof covers the native macOS transport;
Docker argument/mount behavior is unit-tested, not a live Docker/model run.
Older Claude versions that reject these flags fail rather than silently
retrying with unrestricted tools; update Claude Code in that case.
