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
| Codex, Sol | Codex CLI. Run `codex login` for browser-based ChatGPT authentication (preferred), or pipe an OpenAI API key to `codex login --with-api-key`. |
| Kimi K3 | OpenCode plus `MOONSHOT_API_KEY`. Export it or place it in a gitignored project `.env`; Rival searches upward from the workdir. |
| Claude (Opus), Fable, native | Claude Code CLI. Subscription login is the default. To opt into API billing, set both `RIVAL_CLAUDE_AUTH=api` and a funded `ANTHROPIC_API_KEY`. |
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

### Proxy route

When `proxy.claude.enabled` or `proxy.codex.enabled` is set (and `RIVAL_PROXY` is not `off`), that provider's runs go to `proxy.url` with the key from `RIVAL_PROXY_KEY` or the key file. The provider's own login is not used.

Claude (native):

```
claude -p --model <prefix>/<model> ...          # same flags as direct
env:  ANTHROPIC_BASE_URL=<url>  ANTHROPIC_API_KEY=<key>
drop: CLAUDECODE ANTHROPIC_AUTH_TOKEN ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL
      CLAUDE_CODE_USE_BEDROCK CLAUDE_CODE_USE_VERTEX (and inherited BASE_URL/API_KEY)
```

Claude (Docker): the same variables, passed by name with `-e`. A loopback host becomes `host.docker.internal` with `--add-host=host.docker.internal:host-gateway`.

Codex:

```
codex exec -C <workdir> -m <wire id>
  -c model_provider="rival_proxy"
  -c model_providers.rival_proxy={ name = "rival proxy", base_url = "<url>/v1", env_key = "RIVAL_PROXY_KEY", wire_api = "responses" }
  -c model_reasoning_effort=<effort> --sandbox read-only --ephemeral --skip-git-repo-check --color never -
env:  RIVAL_PROXY_KEY=<key>
```

Preflight (`GET <url>/v1/models`, 5 s, once per process):

| Result | Error |
|---|---|
| connection refused or timeout | `proxy unreachable at <url>: <reason>` |
| 401 or 403 | `proxy rejected the key (<status>) — run rival config key set` |
| wire id not listed | `proxy does not serve <wire id>; it serves <model> as: <ids> — set proxy.<provider>.model_prefix` |
| no models of the provider | `proxy has no <provider> account — log in on the proxy` |

A 429 at run time ("cooling down", "monthly spend limit", `rate_limit_error`) gets a hint with the other prefixes that serve the model. Rival never switches prefix or falls back to a direct run by itself.

`RIVAL_PROXY_KEY` is removed from every child environment and added back only for Codex on the proxy route. The key is scrubbed from provider output, session files, Rival's log lines and printed errors.

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

## Known limits

- Retired-skill cleanup in `rival install`: when one removal fails, the
  install prints a failure line. The skills that it removed before the error
  are not added to the `removed` count.
- The update check compares versions as zero-padded strings, not as numbers.
  Each of the three parts is padded to 3 digits. A part with more than 3 digits
  (for example `1.1000.0`) compares incorrectly. A numeric compare would fix
  this, but it would show update notices to `dev` builds.
