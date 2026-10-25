# Rival command reference

[Back to the README](../README.md)

Everything below is checked against the `rival` binary and its source. Commands, flags, defaults and exit codes are exact.

### Contents

1. [Install and setup](#install-and-setup)
2. [Skills](#skills)
3. [Native commands](#native-commands)
4. [Detached flow and `rival wait`](#detached-flow-and-rival-wait)
5. [Models and effort](#models-and-effort)
6. [Code review: JSON contract and console output](#code-review-json-contract-and-console-output)
7. [Plan: contract and rating](#plan-contract-and-rating)
8. [Security review](#security-review)
9. [GitLab MR review](#gitlab-mr-review)
10. [Configuration](#configuration)
11. [Proxy](#proxy)
12. [Queue and timeouts](#queue-and-timeouts)
13. [Sessions directory](#sessions-directory)
14. [TUI](#tui)
15. [Claude authentication and sandboxing](#claude-authentication-and-sandboxing)
16. [Removed in this release](#removed-in-this-release)
17. [Uninstall](#uninstall)

### Install and setup

Choose your system below. The CLI runs on macOS, Linux and Windows.

**Release availability:** v5.0.0 and later contain the Rust CLI for macOS, Linux and Windows. v4.2.0 and earlier support macOS and Linux only.

#### macOS

With [Homebrew](https://docs.brew.sh/Installation), on Apple Silicon or Intel:

```bash
brew install 1905/tap/rival
rival version
rival install
```

To install without Homebrew:

1. Download the matching archive and `checksums.txt` from the same [release](https://github.com/1905/rival/releases).
2. Open Terminal in the download folder. Select your archive:

| Mac | Archive |
|---|---|
| Apple Silicon (M1 or later) | `rival_darwin_arm64.tar.gz` |
| Intel | `rival_darwin_amd64.tar.gz` |

```bash
archive=rival_darwin_arm64.tar.gz    # use rival_darwin_amd64.tar.gz on Intel
shasum -a 256 "$archive"
```

Compare the hash with the matching line in `checksums.txt`. If it matches, install:

```bash
rival_extract=$(mktemp -d)
tar -xzf "$archive" -C "$rival_extract"
mkdir -p "$HOME/.local/bin"
install -m 755 "$rival_extract/rival" "$HOME/.local/bin/rival"
export PATH="$HOME/.local/bin:$PATH"
rival version
rival install
```

Add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zprofile` to keep the command available in new terminals. If you use Bash, add it to `~/.bash_profile` instead.

#### Linux

With [Homebrew on Linux](https://docs.brew.sh/Homebrew-on-Linux):

```bash
brew install 1905/tap/rival
rival version
rival install
```

To install without Homebrew, download your archive and `checksums.txt` from the same [release](https://github.com/1905/rival/releases). Run `uname -m` to identify your CPU:

| `uname -m` | Archive |
|---|---|
| `x86_64` (Intel or AMD) | `rival_linux_amd64.tar.gz` |
| `aarch64` or `arm64` | `rival_linux_arm64.tar.gz` |

The Rust Linux archives require glibc. They do not target Alpine or other musl-based systems.

In the download folder:

```bash
archive=rival_linux_amd64.tar.gz    # use rival_linux_arm64.tar.gz on ARM64
sha256sum "$archive"
```

Compare the hash with the matching line in `checksums.txt`. If it matches, install:

```bash
rival_extract=$(mktemp -d)
tar -xzf "$archive" -C "$rival_extract"
mkdir -p "$HOME/.local/bin"
install -m 755 "$rival_extract/rival" "$HOME/.local/bin/rival"
export PATH="$HOME/.local/bin:$PATH"
rival version
rival install
```

Add `export PATH="$HOME/.local/bin:$PATH"` to your shell startup file (`~/.bashrc` for Bash, `~/.zshrc` for Zsh).

#### Windows

Download your ZIP and `checksums.txt` from the same [release](https://github.com/1905/rival/releases). In Settings → System → About, check **System type**:

| System type | Archive |
|---|---|
| x64-based processor (Intel or AMD) | `rival_windows_amd64.zip` |
| ARM-based processor | `rival_windows_arm64.zip` |

Open PowerShell in the download folder:

```powershell
$archive = '.\rival_windows_amd64.zip'   # use rival_windows_arm64.zip on ARM64
Get-FileHash $archive -Algorithm SHA256
```

Compare the hash with the matching line in `checksums.txt`. If it matches, extract the files and add the folder to your user PATH:

```powershell
$rivalBin = Join-Path $env:LOCALAPPDATA 'Programs\Rival'
Expand-Archive -LiteralPath $archive -DestinationPath $rivalBin -Force
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $rivalBin) {
    [Environment]::SetEnvironmentVariable('Path', "$rivalBin;$userPath", 'User')
}
$env:Path = "$rivalBin;$env:Path"
rival version
rival install
```

No administrator account is needed. New terminals inherit the saved PATH. Rival stores its state in `%USERPROFILE%\.rival`.

The binary is not code-signed. SmartScreen or antivirus software can warn on the first run. If Windows blocks the verified download, run `Unblock-File (Join-Path $rivalBin 'rival.exe')` and try again.

#### Build from source

Install [Rust through rustup](https://rust-lang.org/tools/install/) and Git. The repository selects Rust 1.98. Follow rustup's platform instructions for the native linker and C build tools. On Windows, use the MSVC toolchain and Visual Studio's Desktop development with C++ tools. Windows ARM64 also needs the ARM64 C++ build tools and Clang.

These commands work in a Unix shell or PowerShell:

```text
git clone https://github.com/1905/rival.git
cd rival
cargo install --locked --path crates/rival
rival version
rival install
```

Cargo installs to `~/.cargo/bin` on macOS/Linux or `%USERPROFILE%\.cargo\bin` on Windows. If `rival` is not found, open a new terminal or add that directory to PATH.

A plain Cargo build reports version `dev`. On macOS/Linux, `make cli-install` sets the version from `git describe`. `make install` installs the separate Mac app.

#### Updates and skills

For a Homebrew installation, run `rival update`. It checks the latest release, upgrades through Homebrew when needed, and refreshes the skills.

For a manual archive installation, repeat your OS's download, checksum and extraction steps with the new release. Then run `rival install` again. For a source installation, update the checkout and repeat `cargo install --locked --path crates/rival`. Binary upgrades through `rival update` require Homebrew.

Each Rust release archive contains `rival` (`rival.exe` on Windows), `LICENSE` and `README.md`. Keep the license file with redistributed copies.

```bash
rival install                   # Claude Code, plus Codex when detected
rival install --target codex    # explicit Codex install
rival install --target all      # both sets of skills
rival install --force           # overwrite without prompting
```

| Flag | Default | Effect |
|---|---|---|
| `--target` | `auto` | `auto`, `claude`, `codex` or `all`. |
| `--force` | off | Overwrite installed skills without prompting. |

- Claude Code skills go to `~/.claude/skills`. Codex skills go to `~/.agents/skills`. On Windows, `~` means `%USERPROFILE%`.
- `--target auto` always installs for Claude Code. It adds Codex when one of these exists: `codex` on `PATH`, `$CODEX_HOME`, `~/.codex`, or `Codex.app` in `~/Applications` or `/Applications`.
- `rival install` removes retired skills: the 5.x per-model skills `rival-codex`, `rival-sol`, `rival-claude`, `rival-fable`, `rival-k3`, `rival-grok` and `rival-security` (replaced by `rival` in 6.0), `rival-review`, `rival-plan-sol`, `rival-astra`, `rival-plan-astra`, `rival-plan-fable`, `rival-astra-ultrafast`, `rival-sol-ultrafast` and older names. The 5.1.0 Ultrafast skills are replaced by the `ultrafast` switch in 5.2.

The retired `rival-plan`, `rival-plan-codex` and `rival-plan-claude` skills are
no longer installed. On `rival install` or `rival update`, existing copies are
moved to numbered backups under `~/.claude/rival-retired-skills` or
`~/.agents/rival-retired-skills`, outside the active skills directory. Local
edits and extra files are preserved; earlier backups are never overwritten.
Plan reviews remain available through `rival command plan` with the document
path on stdin.

#### Provider runtimes

Install and authenticate the runtime for each model you use. Rival does not include these programs.

| Model | Runtime and authentication |
|---|---|
| Codex (`gpt-6-astra`), Sol (`gpt-6.1-sol`) | [Codex CLI](https://github.com/openai/codex): `npm install -g @openai/codex && codex login`. Through the [proxy](#proxy), no `codex login` is needed. |
| Claude (`claude-opus-5-5`), Fable (`claude-fable-5-1`) | [Claude Code](https://code.claude.com/docs/en/overview) CLI, authenticated with `claude auth login`. On Windows, use native Claude Code; Docker fallback has a known drive-path defect. On macOS/Linux, if `claude` is not on `PATH`, Rival uses the `rival-claude` Docker image with `RIVAL_CLAUDE_TOKEN` (see [docs/claude-docker-setup.md](claude-docker-setup.md)). |
| Grok (`grok-4.6`) | [Grok CLI](https://docs.x.ai/) with `grok login`. `XAI_API_KEY` is not supported. |
| China (`china.model`, default `moonshot/kimi-k3.1-pro`); also the security review | [OpenCode](https://opencode.ai/docs) v2 with the provider set up in opencode (`opencode auth login` or its config). Rival sends no key. |

### Updating

`rival update` upgrades the Homebrew installation and checks that its executable
reports the requested release version. Before installing skills, it also checks
whether `PATH` selects a different executable. A mismatch stops the update with
the two paths and leaves skills untouched.

If `rival version` still reports `dev` while Homebrew says the release is
installed, compare `command -v rival` with `$(brew --prefix rival)/bin/rival`.
Run the Homebrew executable directly or put its bin directory ahead of the
other installation on `PATH`. The updater does not delete development binaries
or change shell configuration.

Matching skill versions are left unchanged. Missing skills are installed, and
changed versions use the normal confirmation prompt. Use `rival install --force`
only when you intend to overwrite your installed skill contents.

### Skills

`rival install` installs one skill, `rival`, for each host. In Claude Code it is the slash command `/rival`; in Codex it is `$rival`. The skill launches a detached run (see [Detached flow](#detached-flow-and-rival-wait)). It names no model: the model comes from the config.

| Syntax | Runs |
|---|---|
| `/rival [-re low\|medium\|high\|xhigh\|ultra] review [scope]` | A code review with the model of `skill.<host>`. |
| `/rival [-re level] <prompt>` | Any prompt with the model of `skill.<host>`. |
| `/rival security [scope]` | The security review with the `/china` model (`china.model`). |
| `/rival` | Prints the usage. |

| Host | Config key | Default model | Launches |
|---|---|---|---|
| Claude Code | `skill.claude` | `sol` (Sol 6.1) | `rival command skill --host claude` |
| Codex | `skill.codex` | `claude` (Opus 5.5) | `rival command skill --host codex` |

- By default each host gets a review from the other vendor.
- Pick a model in the config window (`rival config`, Models: `/RIVAL RUNS`) or with `rival config set skill.claude <model>`. The values are `codex`, `sol`, `claude` (or `opus`), `fable` and `grok`.
- An omitted `-re` uses `efforts.<model>` of the resolved model. Grok clamps levels above high.
- The review header names the model that ran (`Model: sol (gpt-6.1-sol)`).
- If the runtime of the configured model is missing, the preflight error is followed by `set another model: rival config set skill.<host> <model> (…)`.
- 6.0 removes the per-model skills `/rival-codex`, `/rival-sol`, `/rival-claude`, `/rival-fable`, `/rival-k3`, `/rival-grok` and `/rival-security`. `rival command codex|sol|claude|fable|k3|grok|security|plan` stay for the CLI.

#### `/china`

`rival install` also installs `china` (`/china` in Claude Code, `$china` in Codex). It launches `rival command china` detached. "review with china" in words also triggers it.

| Syntax | Runs |
|---|---|
| `/china [-re low\|medium\|high\|xhigh\|ultra] review [scope]` | A code review with `china.model`. |
| `/china [-re level] <prompt>` | Any prompt with `china.model`. |
| `/china` | Prints the usage. |

- `china.model` is an opencode `provider/model`, split at the first `/` (the model may hold more slashes). Default: `moonshot/kimi-k3.1-pro`. Set it with `rival config set china.model <provider/model>`, or in the config window (`rival config`, Models: `/CHINA RUNS`).
- The config window's `provider` picker offers `moonshot` (Moonshot) and `openrouter` (OpenRouter). Its `model` picker lists the models of that provider from `opencode models --standalone`, Chinese labs first (Moonshot, DeepSeek, Qwen, Z.ai, MiniMax, StepFun, Tencent, Baidu, ByteDance, Xiaomi, Meituan, inclusionAI). A provider change keeps the model name when the new provider lists it (`moonshot/kimi-k3.1-pro` → `openrouter/moonshotai/kimi-k3.1-pro`), else takes the first model. `e` types a model: its exact name, a part that matches one model, or any full `provider/model` (a model opencode does not list saves with a warning).
- Models that the user's opencode config disables are not listed.
- The runtime is opencode v2 or later: `opencode run --standalone -m <china.model>`, prompt on stdin, the workdir as cwd. The preflight reads `opencode --version`.
- Auth is opencode's own (`opencode auth login` or the opencode config). Rival sends no key. Rival strips every `MOONSHOT_*`, `KIMI_*` and `OPENCODE_*` variable from the child, so an opencode config that reads `{env:MOONSHOT_API_KEY}` fails: use `opencode auth login` or `{file:…}`.
- `OPENCODE_CONFIG_CONTENT` declares the model under its provider with `options.reasoningEffort`, plus the permission profile: read-only for reviews, full auto for raw prompts. `OPENCODE_DISABLE_PROJECT_CONFIG=1` keeps the reviewed repository's opencode config out. Raw prompts also drop the credential variables that `rival command k3` drops. So a provider whose opencode config reads its key from one of them (`{env:OPENAI_API_KEY}`) authenticates in reviews but not in raw prompts: store the key with `opencode auth login` instead. China runs use opencode's own session DB, not a private one: opencode v2 keeps `opencode auth login` credentials in it. Every run uses its own opencode agent, `rival-china`, with the same profile, so permissions on the user's own agents (`build`) do not apply.
- `-re` maps onto Moonshot's levels: `low` → low, `medium`/`high` → high, `xhigh`/`ultra` → max. Omitted: max. A provider that does not accept these values rejects the run.
- The review header names the model by the text after the last `/` (`kimi-k3.1-pro`).

- `review` with no scope reviews the changed files found by git. A scope may be a path or plain language, for example `review the auth middleware`.
- Git scope detection: tracked changes against `HEAD` plus untracked files first. If the tree is clean, the files of the last commit. If there are none, the entire project.
- The scope is a focus hint. Reviewers can read the whole repository.
- A `prompt` (any text that does not start with `review`) runs the model on that prompt. Raw prompts run in full-auto mode in the workdir and can edit files.

### Native commands

Top-level commands (`rival --help`):

| Command | Purpose |
|---|---|
| `rival command <x>` | Skill-facing commands. They read raw skill arguments from stdin. `rival command skill --host claude\|codex` is the one the `/rival` skill runs. |
| `rival run <x>` | Terminal runners with explicit flags. |
| `rival wait` | Blocks until detached runs finish. |
| `rival tui` | Full-screen session monitor. |
| `rival config` | Config window. Subcommands: `show`, `set`, `key set`/`key clear`, `models`, `check`. See [Proxy](#proxy). |
| `rival sessions [--active] [--recent N]` | Prints sessions as a text table. |
| `rival queue` / `rival queue clear [--force]` | Shows queue tickets. `clear` removes dead tickets; `--force` also removes waiting tickets and keeps live running ones. |
| `rival install`, `rival update`, `rival version`, `rival completion` | Setup and maintenance. |

`rival command` subcommands. Each reads its input from stdin.

- If stdin is a terminal or `/dev/null`, every subcommand prints its usage and starts no model.
- Empty piped input prints usage for `skill`, `codex`, `claude`, `grok`, `china` and `plan`. For `security`, empty piped input reviews the changed files.

| Subcommand | Flags (defaults) |
|---|---|
| `codex`, `sol`, `claude` (alias `opus`), `fable`, `grok`, `china` | `--workdir` (`.`), `--no-queue` |
| `plan` | `--workdir` (`.`), `--no-queue`, `-m/--model` (`plan.models`, else `codex`; accepts `codex`, `sol`, `claude`/`opus`, `fable`, comma-separated), `--effort` |
| `security` | `--workdir` (`.`), `--no-queue`, `--which` |
| `skill` | `--host` (required: `claude` or `codex`), `--workdir` (`.`), `--no-queue`. Input that starts with `security` runs `security` with the rest as the scope; any other input runs `codex`, `sol`, `claude`, `fable` or `grok` as `skill.<host>` selects. |

- `--detach` is a flag on `rival command` and applies to every subcommand.
- The Codex service tier is a config key, not a flag: see `speed` under Configuration.
- `plan --help` shows `--effort` with default `high`. That value is not used unless you pass the flag. Without it, Codex runs at xhigh and Claude at medium.
- `plan` input: one path, optionally `-re <level> <path>`. Use `-- <path>` for a path that starts with `-`.

`rival run` subcommands: `claude` (alias `opus`), `fable`, `grok`. There is no `rival run codex` or `rival run sol`; use `rival command codex` or `rival command sol`.

| Flag | Meaning |
|---|---|
| `--prompt-stdin` | Read the prompt from stdin. |
| `--review <scope>` | Review mode. `--review` wins when both are given. |
| `--effort` | `claude`: low, medium, high, xhigh. `grok`: low, medium, high; higher values clamp to high. |
| `--workdir` (`.`), `--no-queue` | As above. |

Examples:

```bash
echo 'review src/api/'             | rival command codex --workdir .
echo '-re high review'             | rival command claude --workdir .
echo 'explain the auth flow'       | rival command codex --workdir .
echo 'docs/plan.md'                | rival command plan --workdir .
echo 'review'                      | rival command skill --host claude --workdir .
echo 'explain the auth flow'       | rival run claude --prompt-stdin --workdir .
rival run grok --review src/api/ --workdir .
rival command security --which
```

Exit codes of `rival command` and `rival run`: `0` on success. On a failed run, the model's non-zero exit code, or `1` for a Rival error such as bad input, a failed preflight or an unusable security review.

### Detached flow and `rival wait`

Skills never block the host session. They use this pattern:

```bash
rival command codex --detach --workdir "$PWD" < input.txt > out.txt 2> err.txt
# stderr line: "rival: detached pid=<N>"
rival wait --log err.txt          # run in the background; exit code = outcome
cat out.txt                       # the review
```

- `--detach` re-executes Rival in its own process session (`setsid`) and exits at once. The run survives the teardown of the launching shell.
- The skill writes the input with a file tool, never with `echo` or a heredoc, so no shell expands it.
- In Claude Code the skill arms `rival wait` as a background task and ends its turn. In Codex the skill keeps the turn and polls `rival wait`.
- If the watcher is lost, run `rival wait --log <err file>` again.

`rival wait` has two modes:

- `rival wait --log <stderr-file>`: reads the PID and session IDs from the stderr file and watches the process. It detects a crash. After the summary it prints the `auto-fix:` policy for the skills.
- `rival wait <session-id>...`: polls session JSON only. The session can turn terminal a moment before stdout is flushed, so prefer `--log`.

| Flag | Default |
|---|---|
| `--log` | none |
| `--poll` | `2s` |
| `--timeout` | `1h35m` with default settings: `RIVAL_QUEUE_TIMEOUT` + 2 × `RIVAL_RUN_TIMEOUT` + 5 minutes. |

| `rival wait` exit code | Meaning |
|---|---|
| `0` | All watched sessions completed. |
| `2` | At least one session failed, including a run timeout. |
| `3` | Rival crashed: the process died and left a session unfinished. |
| `4` | `--timeout` elapsed while a run was still active. |
| `64` | Usage error, for example both `--log` and session IDs. |

### Models and effort

| Label | Model id | Runtime | Built-in effort |
|---|---|---|---|
| `codex` | `gpt-6-astra` | Codex CLI | `xhigh` |
| `sol` | `gpt-6.1-sol` | Codex CLI | `xhigh` |
| `claude` (alias `opus`) | `claude-opus-5-5` | Claude Code CLI (native, else Docker) | `medium` |
| `fable` | `claude-fable-5-1` | Claude Code CLI (native, else Docker) | `medium` |
| `grok` | `grok-4.6` | Grok CLI | `high` |
| the model name of `china.model` (`kimi-k3.1-pro`) | `china.model` (`moonshot/kimi-k3.1-pro`) | OpenCode v2 | `max` |

- The effort ladder is `low`, `medium`, `high`, `xhigh`, `ultra`. `xhigh` and `ultra` are different levels.
- Precedence: explicit `-re`/`--effort`, then `efforts.<label>` in `~/.rival/config.yaml`, then the built-in value.
- China maps the ladder onto Moonshot's levels: `low` → low, `medium`/`high` → high, `xhigh`/`ultra` → max. There is no `efforts` key for it.
- Grok accepts `low`, `medium`, `high`. Higher values clamp to `high`, and the session records the clamped value.
- Claude maps `low` and `medium` to the same CLI levels, and `high`, `xhigh` and `ultra` to the Claude CLI level `max`.

### Code review: JSON contract and console output

Every single-model code review (`codex`, `claude`, `grok`, `china` in review mode) asks the model for exactly one JSON object:

```json
{
  "summary": "1-3 sentence reviewer summary",
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "critical|high|medium|low",
      "category": "bug|security|performance|concurrency|architecture|tests|ux",
      "title": "brief title",
      "body": "concrete explanation tied to code",
      "failure_scenario": "input/state that triggers it → the wrong result",
      "suggestion": "concrete fix",
      "confidence": 8
    }
  ]
}
```

- A clean review is `{"summary": "No issues found.", "findings": []}`.
- Every finding must carry a `failure_scenario`: the input or state that triggers it and the wrong result. The prompt tells the model to drop a finding without one.
- Severity rubric: `critical` = data loss, a reachable security hole, or a crash on a normal path. `high` = wrong result or broken flow on a realistic path. `medium` = wrong result on an edge case, or a real hot-path performance problem. `low` = minor defect with a cheap workaround.
- The prompt asks for the review text in ASD-STE100 Simplified Technical English. After a code, security or plan review, Rival checks the text. If the check finds a word or a sentence outside the standard, the same model edits the wording one time, at low effort and read-only. Facts, file references and numbers do not change. The edited review is the result.
- The check is a Rust port of the checker in the antislop-eng skill (version 2.4.0). It uses the full approved-word dictionary and the software glossary of that skill. The prompts name the standard and its rules; they do not contain the word list.

Rival formats the JSON for the console:

```
═══ RIVAL REVIEW ═══

Model: codex (gpt-6-astra)
Scope: src/api/

Summary: <summary>

1. [high] <title> — src/api/handler.go:42
   <body>
   Scenario: <failure_scenario>
   Fix: <suggestion>
   (bug, confidence 8)

Low confidence (1):
- [low] <title> — src/api/util.go:10 (confidence 4)

Findings: 1 total — 0 crit, 1 high, 0 med, 0 low
Log: ~/.rival/sessions/<session-id>.log
```

- Findings are sorted by severity, then by confidence (highest first).
- Findings with confidence below 6 go to the short "Low confidence" block. The `Findings:` tally counts only the main list.
- An empty `findings` array prints `No issues found.`.
- If the output does not parse, Rival prints `═══ RIVAL REVIEW — UNPARSED OUTPUT ═══` with a `Problem:` line and the raw output. The run still exits `0`. Treat that block as "no structured review", never as a clean review.

### Plan: contract and rating

`rival command plan` reviews one markdown plan or spec. It uses this JSON shape:

```json
{
  "summary": "1-3 sentence overall assessment",
  "rating": 7,
  "findings": [
    {
      "file": "section or file",
      "line": 0,
      "severity": "critical|high|medium|low",
      "category": "...",
      "title": "one-line description",
      "body": "what is wrong and why it matters",
      "suggestion": "concrete fix",
      "confidence": 8
    }
  ]
}
```

| Field | Plan |
|---|---|
| `rating` | 1 (unimplementable or dangerously wrong) to 10 (ready to execute) |
| `category` | `bug`, `gap`, `ambiguity`, `scope`, `verification` |
| Console title | `═══ RIVAL PLAN REVIEW ═══` then `File: <path>` |
| Rating line | `Rating: 7/10` |
| No findings | `No bugs or gaps found.` |

- A payload with a `rating` outside 1–10 is rejected as not a real answer.
- Findings print in the same numbered layout as code review, without the `Scenario:` line, followed by the `Findings:` tally.
- With two models, the title names both (`═══ RIVAL PLAN REVIEW (codex + claude) ═══`). Each model gets a `── <label> ──` block.
- A model that cannot run is listed as `Skipped: <label> — <reason>`. It does not fail the run.

### Security review

`/rival security [scope]` runs `rival command security`. It hunts exploitable vulnerabilities in twelve classes: injection, authorization (including IDOR), authentication, crypto, path traversal, SSRF, deserialization, secret exposure, input validation, CSRF, open redirect, and resource exhaustion. It does not report style or ordinary logic bugs.

The model is the `/china` model: `china.model` in `~/.rival/config.yaml` (default `moonshot/kimi-k3.1-pro`), through the same opencode v2 runner (see [`/china`](#china)). The review runs read-only at `max`; the language pass runs at `low`.

- `rival command security --which` prints the model, `china.model` with its source (`default` or `file`), the effort, the auth note and `Ready.` or `Not usable.`. It exits `1` when not usable.
- Auth is opencode's own. Rival reads no key.
- If opencode v2 is missing, the run fails. It never falls back to another model.
- It uses the code-review JSON contract. The console title is `═══ RIVAL SECURITY REVIEW ═══`; an empty list prints `No vulnerabilities found.`.
- Every finding must have a file, a title and a known severity. Otherwise Rival prints `RIVAL SECURITY REVIEW — UNUSABLE OUTPUT` and exits `1`.
- 6.0 and earlier picked the model with `security.reviewer` (`k3` or `grok`). That key is removed; an old value is ignored.

### GitLab MR review

A single-model review accepts a GitLab merge request URL as its whole scope:

```bash
/rival review https://gitlab.example.com/group/project/-/merge_requests/123
echo 'review https://gitlab.example.com/group/project/-/merge_requests/123' | rival command codex --workdir .
rival run claude --review https://gitlab.example.com/group/project/-/merge_requests/123 --workdir .
```

- Supported by `rival command codex|claude|grok|china` in review mode and by `rival run <x> --review`.
- Rejected before any reviewer starts by raw prompts and `security`.
- The scope must be one HTTPS URL of the form `.../<namespace>/<project>/-/merge_requests/<N>`, optionally ending in `/diffs` or `/commits`.
- The workdir must be a git repository with a remote for the same host and project.
- Rival resolves the MR with `glab api` (authenticate with `glab auth login --hostname <host>`). It then fetches the exact base and head commits into a temporary checkout. Your checkout and index are not touched.
- The reviewer also receives the full patch. A patch over 512 KiB is refused.
- Output starts with `GitLab MR: <url>`, `Base: <sha>`, `Head: <sha>`.

### Configuration

`~/.rival/config.yaml` is optional. These are all the keys that exist:

```yaml
efforts:              # per-model default effort
  codex: xhigh        # low | medium | high | xhigh | ultra
  sol: xhigh          # low | medium | high | xhigh | ultra
  claude: medium      # low | medium | high | xhigh | ultra
  fable: medium       # low | medium | high | xhigh | ultra
  grok: high          # low | medium | high | xhigh | ultra (clamped to high)

skill:                # the model the /rival skill runs, per host
  claude: sol         # in Claude Code: codex | sol | claude (or opus) | fable | grok
  codex: claude       # in Codex: the same values

china:                # the model the /china skill runs
  model: moonshot/kimi-k3.1-pro   # an opencode provider/model; also runs the security review

claude:
  subscription: team  # free text shown in the TUI Account field, e.g. team or personal

auto_fix_critical_high: false  # true: skills fix CONFIRMED critical/high findings without asking

speed:                # Codex service tier, per model: normal | fast | ultrafast
  codex: fast         # GPT-6 Astra
  sol: fast           # Sol 6.1

proxy:                # see Proxy
  url: http://127.0.0.1:8317
  key_file: ~/.rival/proxy.key   # default; the key never goes in this file
  claude:
    enabled: false
    model_prefix: ""  # wire id <prefix>/<model>
  codex:
    enabled: false
    model_prefix: ""

plan:
  models: [codex]     # default --model of rival command plan

roles:                # optional prompt overrides
  bug_hunter: "..."   # replaces the code-review instructions
  security: "..."     # replaces the security-review instructions
```

- The values shown for `efforts`, `skill` and `speed` are the built-in defaults.
- An unknown `efforts` key, an invalid effort, an invalid `skill.claude` / `skill.codex`, an invalid `china.model`, or an invalid `speed.codex` / `speed.sol` stops every command before it creates a session.
- The removed keys `security.reviewer` and `efforts.kimi-k3` are ignored when the file has them; `rival config set` rejects them.
- `rival config set` and the config windows rewrite this file. Unknown keys stay. Comments do not: the first rewrite of a hand-written file keeps a copy in `config.yaml.bak`.
- `auto_fix_critical_high` is off by default. Skills always verify every finding. When it is on, they fix CONFIRMED critical and high findings without asking, then build and test. Medium and low findings are only verified, presented and proposed. `rival wait --log` prints the setting as its last line: `auto-fix: off` or `auto-fix: critical+high`.
- `speed.codex` and `speed.sol` pick the Codex service tier of each Codex-runtime model: `normal`, `fast` (the default) or `ultrafast`. Change them in the config window (`rival config`, Models: `Codex speed`, `Sol speed`; `←`/`→` cycles normal → fast → ultrafast) or with `rival config set speed.sol normal`. The speed applies to every call of that model: `/rival` reviews and prompts, `rival command codex` and `rival command sol`, their language-repair pass, `rival command plan` and `rival config check`.
  - `normal` sends the plain argv. `fast` adds `-c service_tier="fast"` and `-c features.fast_mode=true` after `-m <model>` (after the proxy provider pairs on the proxy route). Fast runs at about 2x speed and uses more of the plan's quota; Codex sends it as its `priority` tier. `ultrafast` adds `-c service_tier="ultrafast"` and `-c features.ultrafast_mode=true`. The session records `"service_tier": "fast"` or `"ultrafast"`.
  - Ultrafast needs the Pro $500 plan, or an Enterprise/Edu workspace with Ultrafast on. `rival config set speed.<model> ultrafast` saves and prints that warning on stderr; the config window shows it as a notice.
  - Compatibility: with no `speed.<model>`, a 5.2 `ultrafast.<model>: true` reads as `ultrafast`; `false` or absent reads as `fast`. `rival config set` writes only `speed.*`; `ultrafast.<model>` is not a settable key.
  - When the account's model catalog does not list the tier, Codex prints a warning and sends the request at the standard tier. Rival sets `"service_tier_omitted": true` on the session and prints `rival: ⚠ <tier> not applied — …` after the output (command, plan) on stdout and stderr; for ultrafast on the direct route it adds the plan line. `rival config check` marks the row `[fast]`, `[ultrafast]` or `[<tier> not applied]`; `--json` rows have `speed` (`normal`, `fast`, `ultrafast`, or "" for the other models) and `tier_omitted`. Exit codes do not change.
  - Rival cannot see a tier that a server-side requirement turns off: Codex drops it without a warning. Whether a proxy forwards `service_tier` depends on the proxy.
- A `roles` override replaces the role instructions only. Rival still appends the JSON contract. An empty override is ignored.

Environment variables:

| Variable | Default | Effect |
|---|---|---|
| `RIVAL_MAX_CONCURRENT` | `2` | Runs allowed at once. |
| `RIVAL_QUEUE_TIMEOUT` | `30m` | Longest wait for a queue slot. |
| `RIVAL_RUN_TIMEOUT` | `30m` | Longest run after it gets a slot. `0` disables it. |
| `RIVAL_NO_QUEUE` | unset | Bypass the queue (same as `--no-queue`). |
| `RIVAL_CLAUDE_AUTH` | `subscription` | `subscription`/`sub` or `api`. See [Claude authentication](#claude-authentication-and-sandboxing). |
| `RIVAL_CLAUDE_TOKEN` | unset | OAuth token for the Docker Claude runtime. |
| `RIVAL_NO_UPDATE_CHECK` | unset | Disable the update check (`CI` also disables it). |
| `RIVAL_PROXY` | unset | `off` sends this run direct, whatever `proxy:` says. Process environment only; a repository .env cannot set it. |
| `RIVAL_PROXY_URL` | unset | Wins over `proxy.url`. Process environment only; a repository .env cannot set it. |
| `RIVAL_PROXY_KEY` | unset | Wins over the key file. Never passed to a child process, except as the Codex provider key on the proxy route. Process environment only; a repository .env cannot set it. |
| `RIVAL_HOME` | unset | State directory used instead of `~/.rival`: `config.yaml`, `sessions/`, `queue/` and the update-check cache. Set it in the process environment; repository `.env` files cannot set it. Skills still go to `~/.claude/skills` and `~/.agents/skills`. |

### Proxy

Rival can send Claude runs (Opus, Fable) and Codex runs (Codex, Sol) through one HTTP gateway such as CLIProxyAPI. Each provider has its own switch and model prefix. China and Grok always run direct.

| Item | Claude route | Codex route |
|---|---|---|
| Model sent | `--model <prefix>/<model>`, e.g. `emcd_/claude-opus-5-5` | `-m <prefix>/<model>`, or the bare id when the prefix is empty |
| Endpoint | `ANTHROPIC_BASE_URL=<url>` | an inline `rival_proxy` model provider, `base_url = "<url>/v1"`, `wire_api = "responses"` |
| Key | `ANTHROPIC_API_KEY` in the child environment | `RIVAL_PROXY_KEY` in the child environment (`env_key`) |
| Removed from the child | `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_DEFAULT_*_MODEL`, `CLAUDE_CODE_USE_BEDROCK`/`VERTEX` | — |
| Login needed | none (`RIVAL_CLAUDE_AUTH` is ignored) | none (`codex login status` is skipped) |

- The key comes from `RIVAL_PROXY_KEY`, else `proxy.key_file`, else `~/.rival/proxy.key`. A key file that other users can read is an error. A `key:` in `config.yaml` is an error. The key is never in process arguments, and Rival scrubs it from logs, session files and errors.
- Before a proxied run, Rival reads `<url>/v1/models` once. It stops with the fix when the proxy is down, rejects the key (401/403), or does not serve the exact model name. The error lists the prefixes that do serve it.
- An account at its limit returns 429 at run time. The error names the other prefixes that serve the model. Rival does not switch prefixes by itself.
- Docker Claude reaches a loopback proxy at `host.docker.internal`.
- Sessions record `route` (`proxy`) and `wire_model`.

`rival config` commands:

| Command | Effect |
|---|---|
| `rival config` | Opens the config window (TUI). |
| `rival config show [--json]` | Each value with its source: `default`, `file` or `env`. The key shows as `set (…a91f)` or `missing`. |
| `rival config set KEY VALUE` | Sets one key, e.g. `proxy.claude.model_prefix emcd_`. `set --json` reads a patch from stdin. |
| `rival config key set` / `key clear` | Reads the key from stdin (hidden on a terminal) and writes it with mode 0600 / removes it. |
| `rival config models [--json]` | The proxy's models, grouped by prefix. |
| `rival config check [-m LIST\|all] [--json] [--config-stdin]` | Sends `Reply with exactly: ok` to each model through its real adapter (low effort, read-only, 90 s). Default: codex, sol, claude, fable and china (`china.model`, which also runs the security review). Exit `1` if one fails. `--json` prints one line per model, then a summary. `--config-stdin` checks an unsaved draft. Check runs are not saved as sessions; logs go to `~/.rival/check/`. |

The config window (`rival config`) has Proxy, Models, Review and Check sections, a "Requests go to" list with the exact model name each run sends, and a check panel. Models has the model table (MODEL, RUNTIME, ROUTE, EFFORT; `←`/`→` picks the effort), the `/RIVAL RUNS` pickers `in Claude Code` (`skill.claude`) and `in Codex` (`skill.codex`), which cycle the six models with `←`/`→`, the `/CHINA RUNS` pickers `provider` and `model` (`china.model`, from opencode's model list; `e` types a model), and the two speed pickers (`Codex speed`, `Sol speed`). The window does not set `plan.models`; use `rival config set plan.models` or `rival command plan -m`. Keys: `Tab` section, `↑`/`↓` field, `Space` toggle, `←`/`→` choose, `e` type a value, `c` check the draft, `a` check all models, `x` cancel the check, `s` save, `u` undo, `?` help, `Esc` back (asks to save unsaved changes).

### Queue and timeouts

- Rival has no daemon. A cross-process FIFO queue in `~/.rival/queue/` (ticket files plus `flock`) limits concurrent runs to `RIVAL_MAX_CONCURRENT`.
- A waiting run shows as `queued` in the TUI. A dead slot holder is reaped.
- A queue wait longer than `RIVAL_QUEUE_TIMEOUT`, or a run longer than `RIVAL_RUN_TIMEOUT`, fails the session. `rival wait` then exits `2`.
- Use `--no-queue` or `RIVAL_NO_QUEUE` on NFS home directories, where `flock` is not reliable.

### Sessions directory

Every run writes to `~/.rival/sessions/` (`$RIVAL_HOME/sessions/` when `RIVAL_HOME` is set):

- `<session-id>.json`: the session record. It is written to `<session-id>.json.tmp` first and then renamed.
- `<session-id>.log`: the full model output.
- `<session-id>.log.repair.log`: the output of the wording edit, when one runs.

Main JSON fields: `id`, `group_id` (shared by the models of one plan run), `cli`, `mode`, `model`, `effort`, `review_scope`, `prompt_preview`, `status` (`queued`, `running`, `completed`, `failed`), `start_time`, `end_time`, `exit_code`, `duration`, `work_dir`, `log_file`, `error`, `pid`, `pid_start`.

`rival sessions` prints them as a table. The TUI reads the same directory.

### TUI

`rival tui` shows runs grouped by day (TODAY, YESTERDAY, THIS WEEK, OLDER), 50 runs per page. Runs with the same `group_id` show as one row.

List keys:

| Key | Action |
|---|---|
| `↑`/`k`, `↓`/`j` | Move up and down. |
| `g`/`Home`, `G`/`End` | First and last run. |
| `n`/`PgDn`, `p`/`PgUp` | Next and previous page. |
| `Enter` | Open the run. |
| `/` | Filter. Space-separated terms, all must match, case-insensitive. `Enter` keeps the filter, `Esc` clears it. |
| `Tab`, `Shift+Tab` | Cycle the status tabs: ALL, RUNNING, FAILED, DONE. |
| `?` | More help. |
| `q`, `Ctrl+C` | Quit. |

Detail keys:

| Key | Action |
|---|---|
| `1`, `2`, `3`, `4` | Result, Raw, Prompt, Info tabs. |
| `j`/`k`, `Enter`/`Space` on Result | Select a finding, then expand or collapse it. |
| `[`, `]` | Previous and next member of a group. |
| `f` | Follow the live output. |
| `/`, `n`, `N` | Search the output, next match, previous match. |
| `o` | Open the log file. |
| `x` | Open the stop confirmation. `y` stops the run; `n` or `Esc` cancels. Unix sends SIGTERM. Windows terminates the owner and its provider tree. |
| `Esc` | Back to the list. |

Finished runs open on Result. Live runs open on Raw. Result shows findings by severity or formatted Markdown. If parsing fails, it offers the raw log.

### Claude authentication and sandboxing

Claude runs through the Claude Code CLI and bills your subscription login by default.

| `RIVAL_CLAUDE_AUTH` | Behavior |
|---|---|
| unset, `subscription`, `sub` | Use the CLI login. `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are removed from the child environment. |
| `api` | Bill the API key. `ANTHROPIC_API_KEY` must be set, or the run fails. |
| any other value | Error. |

- Claude code and plan reviews can use only the Read, Glob and Grep tools. Shell, edits, MCP tools, hooks and plugins are disabled. The Docker transport also mounts the repository read-only.
- These are CLI tool restrictions, not an operating-system sandbox.
- Grok review mode passes `--sandbox read-only`. The Grok CLI profiles fail open when the host has no kernel sandbox, so treat that as a request, not a guarantee.
- Rival removes known credential variables from child processes, and blocks the `GROK_` and `XAI_` prefixes.

### Removed in this release

| Removed | Replacement |
|---|---|
| Per-model skills `/rival-codex`, `/rival-sol`, `/rival-claude`, `/rival-fable`, `/rival-k3`, `/rival-grok`, `/rival-security` (6.0) | `/rival` with `skill.claude` / `skill.codex`; `/rival security`. `rival install` removes the old skills. |
| Kimi K3: `rival command k3`, `rival run k3`, `skill.<host>: k3`, `efforts.kimi-k3` | `rival command china` with `china.model` (default `moonshot/kimi-k3.1-pro`) |
| `security.reviewer` (`k3`, `grok`) and the OpenRouter Grok security model | The security review runs on `china.model`. |
| The PLAN column and plan toggles of the config window (6.0) | `rival config set plan.models <models>` |
| Megareview: `rival review`, `rival command megareview`, and the consilium judge | `rival command codex review` (skill: `/rival review`) |
| `/rival-review` skill | `/rival review`. `rival install` removes the old skill. |
| `rival server` (web dashboard) | `rival tui` |
| Rival.app (macOS viewer) and its `rival-app` cask | `rival tui`. Remove an installed copy with `brew uninstall --cask rival-app`. |

Sessions written by older releases still render in the TUI, with their old labels.

### Uninstall

```bash
brew uninstall rival
brew uninstall --cask rival-app   # only if an old Rival.app is installed
```

Then delete the `rival-*` directories in `~/.claude/skills` and `~/.agents/skills`. Session history stays in `~/.rival` until you delete it.
