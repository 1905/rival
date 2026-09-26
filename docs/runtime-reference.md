# Runtime and model reference

This document describes Rival's current unreleased command surface, model
roster, authentication inputs, and effort resolution. Slash-command skills
delegate to these same commands and runtimes.

## Code-review roster

`rival review [scope]` runs a multi-model review and merges successful results
through a consilium judge.

- The default roster is **Sol + Kimi K3**.
- `--model` replaces the complete roster for that invocation; it does not add
  to the defaults.
- Accepted selectors are `sol`, `kimi-k3` (`k3`), and `grok`. Selectors may be
  comma-separated or repeated.
- **Grok is opt-in**: it never joins the default roster and participates only
  when `--model` names it.
- Kimi K3 always runs at its provider's only supported reasoning level, `max`.
- Every reviewer uses the `bug_hunter` role. The successful judge priority is
  Sol, then Kimi K3. For an explicit `--model` list the requested order controls
  judge priority, so `--model grok,sol` judges with grok and `--model sol,grok`
  judges with Sol.
- An unavailable selected runtime is reported as skipped. The review can
  continue when at least one selected model succeeds.

Examples:

```bash
rival review
rival review src/api/
rival review --model sol src/api/
rival review --model k3 src/api/
rival review --model grok src/api/
rival review --model grok,sol src/api/
```

Claude is available for standalone code review and plan review through the
Claude Code runtime; it is not a member of the default code-review roster.

## Native commands

Use `--prompt-stdin` for an arbitrary direct prompt and `--review` for the
single-model review template:

```bash
printf '%s\n' 'explain the auth flow' |
  rival run sol --prompt-stdin --workdir .
rival run sol --review src/api/ --workdir .

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

Claude's skill-facing command uses the same runtime but reads its argument
grammar from stdin:

```bash
printf '%s\n' 'review src/api/' |
  rival command claude --workdir .
printf '%s\n' '-re high review src/api/' |
  rival command claude --workdir .
```

Grok has the same skill-facing command shape:

```bash
printf '%s\n' 'review src/api/' |
  rival command grok --workdir .
printf '%s\n' '-re high review src/api/' |
  rival command grok --workdir .
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

Native plan review accepts one file path on stdin. An omitted effort resolves
separately for each selected model:

```bash
printf '%s\n' 'docs/plan.md' |
  rival command plan --model codex,claude --workdir .
printf '%s\n' 'docs/plan.md' |
  rival command plan --model claude --effort high --workdir .
```

Operational views are available through `rival tui`, `rival sessions`, and
`rival server --port 3333`. The web server listens on `127.0.0.1` and tries up
to ten subsequent ports when the requested port is occupied.

## Authentication

Rival launches installed provider CLIs; it does not replace their accounts.

| Model | Runtime and required authentication |
|---|---|
| Sol | Codex CLI. Run `codex login` for browser-based ChatGPT authentication (preferred), or pipe an OpenAI API key to `codex login --with-api-key`. `codex login status` must succeed. |
| Kimi K3 | OpenCode plus `MOONSHOT_API_KEY`. Export it or place it in a gitignored project `.env`; Rival searches upward from the workdir. |
| Claude, native | Claude Code CLI. Subscription login is the default. To opt into API billing, set both `RIVAL_CLAUDE_AUTH=api` and a funded `ANTHROPIC_API_KEY`. |
| Claude, Docker fallback | `RIVAL_CLAUDE_TOKEN` containing the OAuth access token extracted by the flow in [Claude in Docker](claude-docker-setup.md). |
| Grok | Grok CLI. Run `grok login` for browser OAuth against grok.com. The preflight requires `grok` on `PATH` and `~/.grok/auth.json` to exist. `XAI_API_KEY` is deliberately unsupported. |

For API-key-based Sol authentication, let Codex store the credential and then
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
  sol: high
  kimi-k3: max
  claude: medium
  grok: high
```

Effort precedence is:

1. an explicit invocation value (`--effort` or skill `-re`);
2. the matching entry in `~/.rival/config.yaml`;
3. that command's built-in fallback.

Non-K3 configuration values may be `low`, `medium`, `high`, `xhigh`, or
`ultra`. Individual command help may expose a smaller relevant subset. Kimi K3
must be `max`; an invocation-level effort is normalized to `max`.

Grok-4.5 exposes only `low`, `medium`, and `high`. Rival clamps the wider ladder
onto that menu rather than failing a run over a level the model does not expose:
`xhigh`, `ultra`, and `max` become `high`; `minimal` and `none` become `low`. An
unrecognized value is still an error, so a typo cannot silently downgrade a run.
The clamp is applied before the session is created, so `rival sessions` and the
dashboards report the level actually sent.

The general built-in defaults are Sol `high`, Codex `xhigh`, Claude `medium`,
Kimi K3 `max`, and Grok `high` — which is also grok-4.6's own built-in default.
Codex (`xhigh`) and Claude (`medium`) pin their effort on plan reviews too; an
explicit `-re` or a configured effort still wins. Plan review accepts `codex`
and `claude`.

Invalid model labels or effort values in `~/.rival/config.yaml` stop the command
before sessions or queue entries are created.
