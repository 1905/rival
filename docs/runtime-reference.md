# Runtime and model reference

This document describes Rival's command surface, models, authentication
inputs, and effort resolution. Slash-command skills delegate to these same
commands and runtimes. The README "Reference (for agents)" section is the
complete command reference; this page adds runtime detail.

## Code review

Code review is single-model. Pick the model with the command:

```bash
printf '%s\n' 'review src/api/' | rival command codex  --workdir .
printf '%s\n' 'review src/api/' | rival command claude --workdir .
printf '%s\n' 'review src/api/' | rival command k3     --workdir .
printf '%s\n' 'review src/api/' | rival command grok   --workdir .
```

- `review` with no scope reviews the files git reports as changed.
- `-re <level>` before `review` overrides the effort, for example
  `-re high review src/api/`. Kimi K3 ignores it.
- An unavailable runtime fails its own run. No other model is substituted.

## Native commands

`rival run` takes explicit flags. Use `--prompt-stdin` for an arbitrary direct
prompt and `--review` for the single-model review template. It exists for
`claude`, `grok` and `k3`; Codex uses `rival command codex`.

```bash
printf '%s\n' 'inspect this project' |
  rival run k3 --prompt-stdin --workdir .
rival run k3 --review src/api/ --workdir .

printf '%s\n' 'explain the auth flow' |
  rival run claude --prompt-stdin --workdir .
rival run claude --review src/api/ --workdir .

printf '%s\n' 'explain the auth flow' |
  rival run grok --prompt-stdin --workdir .
rival run grok --review src/api/ --workdir .
```

### Grok invocation shape

Unlike the other runtimes, the grok CLI does not read its prompt from stdin, so
Rival writes the composed prompt (system prompt, workdir preamble, then the
user prompt) to a temporary file and passes the path. That also keeps the
prompt out of the process table. The argv is:

```
grok --prompt-file <tmpfile> \
     -m grok-4.6 \
     --effort <low|medium|high> \
     --output-format plain \
     --no-auto-update \
     --yolo \
     [--cwd <workdir>] \
     [--sandbox read-only]
```

- `--cwd` is appended only when a workdir is set.
- `--sandbox read-only` is appended only for review-mode runs. The flag is
  derived from the session mode, so a session recorded as `review` can never run
  writable.
- `--effort` carries the clamped level, never the raw request.
- The temporary prompt file is removed when the run returns.

Sandbox caveats: grok's built-in profiles fail open when the host offers no
kernel sandbox, so the flag is a request Rival cannot verify was enforced. Even
when enforced, the read-only profile grants writes to a fixed allowlist
including `~/.grok` and the system temp directories, so a workdir under `/tmp`
or `/private/tmp` stays writable. Child-process network access is not blocked on
macOS. On macOS the applied profile, including the `enforced` flag and the
allowlist, is appended to `~/.grok/sandbox-events.jsonl`.

### Plan review

Native plan review accepts one file path on stdin. An omitted effort resolves
separately for each selected model:

```bash
printf '%s\n' 'docs/plan.md' |
  rival command plan --model codex,claude --workdir .
printf '%s\n' 'docs/plan.md' |
  rival command plan --model claude --effort high --workdir .
```

Operational views are `rival tui`, `rival sessions`, and Rival.app
(`brew install --cask 1905/tap/rival-app`).

## Authentication

Rival launches installed provider CLIs; it does not replace their accounts.

| Model | Runtime and required authentication |
|---|---|
| Codex | Codex CLI. Run `codex login` for browser-based ChatGPT authentication (preferred), or pipe an OpenAI API key to `codex login --with-api-key`. |
| Kimi K3 | OpenCode plus `MOONSHOT_API_KEY`. Export it or place it in a gitignored project `.env`; Rival searches upward from the workdir. |
| Claude, native | Claude Code CLI. Subscription login is the default. To opt into API billing, set both `RIVAL_CLAUDE_AUTH=api` and a funded `ANTHROPIC_API_KEY`. |
| Claude, Docker fallback | `RIVAL_CLAUDE_TOKEN` containing the OAuth access token extracted by the flow in [Claude in Docker](claude-docker-setup.md). |
| Grok | Grok CLI. Run `grok login` for browser OAuth against grok.com. The preflight requires `grok` on `PATH` and `~/.grok/auth.json` to exist. `XAI_API_KEY` is deliberately unsupported. |
| Grok via OpenRouter (security only) | OpenCode plus `OPENROUTER_API_KEY`, from the environment or the nearest `.env` above the workdir. |

For API-key-based Codex authentication, let Codex store the credential and then
remove it from the immediate shell:

```bash
export OPENAI_API_KEY='your-key'
printenv OPENAI_API_KEY | codex login --with-api-key
unset OPENAI_API_KEY
codex login status
```

For native Claude runs, Rival strips inherited Anthropic key variables in the
default subscription mode so an unrelated shell variable cannot silently
switch billing to API credits. Docker is selected only when the `claude`
executable is not on `PATH`.

Rival blocks the `GROK_` and `XAI_` environment prefixes from child processes,
so a reviewed repository's `.env` cannot repoint the grok runtime through proxy
or base URLs, `GROK_HOME`, or auth helpers, and cannot inject an API key to
bypass the logged-in account. The preflight resolves `auth.json` from the real
home directory for the same reason: honoring `GROK_HOME` would check a location
the run can never use.

Never commit provider keys or OAuth tokens. A project `.env` used for K3 must be
listed in `.gitignore`.

## Per-model effort defaults

Configure stable model labels in `~/.rival/config.yaml`:

```yaml
efforts:
  codex: xhigh
  kimi-k3: max
  claude: medium
  grok: high
```

Effort precedence is:

1. an explicit invocation value (`--effort` or skill `-re`);
2. the matching entry in `~/.rival/config.yaml`;
3. the model's built-in default.

Non-K3 configuration values may be `low`, `medium`, `high`, `xhigh`, or
`ultra`. Individual command help may expose a smaller relevant subset. Kimi K3
must be `max`; an invocation-level effort is normalized to `max`.

grok-4.6 exposes only `low`, `medium`, and `high`. Rival clamps the wider ladder
onto that menu rather than failing a run over a level the model does not expose:
`xhigh` and `ultra` become `high`. An unrecognized value is still an error, so a
typo cannot silently downgrade a run. The clamp is applied before the session is
created, so `rival sessions` and the dashboards report the level actually sent.

The built-in defaults are Codex `xhigh`, Claude `medium`, Kimi K3 `max`, and
Grok `high` — which is also grok-4.6's own default. Codex and Claude keep these
defaults on plan reviews. An explicit `-re` or a configured effort still wins
everywhere. Plan review accepts `codex` and `claude`.

Invalid model labels or effort values in `~/.rival/config.yaml` stop the command
before sessions or queue entries are created. An old `efforts.sol` entry is
ignored.
