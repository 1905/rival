# Parity scenario runner

Runs the Rust `rival` debug binary against scenarios whose expected output is
written from the Go source and Go tests. Scenarios use fake reviewer CLIs and a
local HTTP server. No Go binary runs.

```sh
cargo build -p rival
python3 parity/run.py --bin target/debug/rival                       # every scenario
python3 parity/run.py --bin target/debug/rival --scenario 'queue*'   # glob on file stems, repeatable
python3 -m unittest discover -s parity -p 'test_*.py'               # the runner's own tests
```

- Exit codes: `0` all passed, `1` a scenario failed, `2` bad arguments, `3` a safety guard fired (the run stops).
- SIGTERM or SIGINT: the runner kills the running scenario's processes, reports it, and exits `128+signal` (`143`, `130`).
- Requirements: Python 3.9+ standard library only, on macOS or Linux.
- Scenario roots are never deleted. The last line prints their parent directory.
- Use the debug binary. Only debug builds honour `RIVAL_UPDATE_API`. A release build would query GitHub.

## Isolation

Each scenario gets its own temp root:

| Path | Use |
|---|---|
| `home/` | `HOME`, `USERPROFILE`; `RIVAL_HOME=home/.rival` |
| `work/` | default working directory (no stray `.env` is loaded) |
| `tmp/` | `TMPDIR` |
| `bin/` | one launcher per present fake |
| `sysbin/` | symlinks to the allowed system tools (`git`, plus `system_tools`) |
| `.parity/` | scenario copy, fake scripts and call logs, step stdout/stderr files |

- `PATH` is `bin:sysbin` only. The runner checks that each present fake resolves to `bin/` and each absent fake resolves to nothing. A real `codex` or `docker` on the host can never answer.
- The environment is built, not inherited. Only `USER` and `LOGNAME` come from the caller.
- Fixed values: `TZ=UTC`, `CI=1`, `RIVAL_NO_TELEMETRY=1`, `RIVAL_NO_UPDATE_CHECK=1`, `GIT_CONFIG_NOSYSTEM=1`.
- The telemetry opt-out key comes from `internal/telemetry/telemetry.go`: `DO_NOT_TRACK`, `RIVAL_NO_TELEMETRY` or `CI`.
- `RIVAL_UPDATE_API` always points at the runner's server on `127.0.0.1`. The Rust client requests `GET $RIVAL_UPDATE_API/repos/1905/rival/releases/latest`.
- Each scenario runs its own server. Every request is recorded. An unknown path gets a 404 and fails the scenario unless it is expected.
- A scenario `env` may set or unset (`null`) any variable except `PATH`, `HOME`, `USERPROFILE`, `RIVAL_HOME`, `TMPDIR`, `RIVAL_NO_TELEMETRY`, `RIVAL_UPDATE_API`, `GIT_CONFIG_NOSYSTEM`, `RIVAL_PARITY_TASK` and `FAKE_*`.
- Process ownership: every process started for a scenario carries `RIVAL_PARITY_TASK=<token>` in its environment.
- The runner finds owned processes through `/proc/*/environ` on Linux or `ps -E` on macOS. It never matches by name.
- The runner also tracks every step process from the moment it spawns. Cleanup kills the group of any tracked step still running, even one that dropped the token.
- A process still alive at the end fails the scenario and is killed (TERM, then KILL). Step timeouts kill the step's process group.
- Cleanup runs at the end, on a setup or step error, and on SIGTERM/SIGINT. A signal that arrives during a spawn or during cleanup waits until that finishes.

## Fakes

`fakes/fakecli.py` implements `codex claude grok opencode glab docker brew`. Each scenario generates `bin/<name>` with an absolute `#!<python> -IB` shebang. Each launcher calls `fakecli.main(<name>)`. A fake reads `FAKE_<NAME>_SCRIPT`, which the runner points at `.parity/fakes/<name>.json`:

```json
{"responses": [
  {"argv": ["login", "status"], "times": 1, "stdout": "Logged in\n", "exit": 0},
  {"argv_prefix": ["exec"], "expect": {"stdin": {"$regex": "(?s).*diff.*"}, "cwd": "<ROOT>/work",
                                       "env": {"ANTHROPIC_API_KEY": null}},
   "touch": "<ROOT>/sync/started", "wait_for": "<ROOT>/sync/go", "wait_timeout": 30, "delay": 0.1,
   "stdout": "...", "stderr": "You've hit your usage limit.\n", "exit": 1}
]}
```

- The fake reads stdin to EOF. It then picks the first response whose `argv` (exact) or `argv_prefix` matches and whose `times` budget is not spent.
- It logs `{pid, argv, cwd, env, stdin, response, violations}` to `.parity/fakes/<name>.calls.jsonl`, then plays `touch`, `wait_for`, `delay`, `stdout`, `stderr` and `exit`.
- argv items, `expect.stdin`, `expect.cwd` and `expect.env` values are exact strings or `{"$regex": ...}` (full match). An env value of `null` means the variable must be unset.
- `times: N` means the response must be used exactly N times. These cases fail the scenario: an unscripted fake, an argv with no response, a violated `expect`, a `wait_for` timeout.
- An absent fake (`fakes.absent`) has no launcher, so `LookPath` fails. Use it for the missing-runtime preflight and the Claude Docker fallback.
- Brew guard, step 1: before any binary runs, the runner checks every brew response that matches `--prefix rival`. Its stdout must be inside the task root, and `<prefix>/bin/rival` must exist (add a `copy_bin` fixture). An unsafe prefix stops the run with exit 3.
- Brew guard, step 2: at runtime, the fake refuses any `--prefix` answer outside the root. It exits 98 with no stdout and logs `unsafe`, which also stops the run.

## Scenario files

`scenarios/*.yaml` use the JSON subset of YAML, so the runner needs only the standard library. Unknown keys are errors. The sketch below shows every key; it is not a runnable scenario.

```json
{
  "name": "queue-empty",
  "description": "where the expectation comes from in the Go source",
  "timeout": 300,
  "fakes": {"absent": ["docker"], "scripts": {"codex": {"responses": []}}},
  "system_tools": ["ps"],
  "env": {"CI": null, "RIVAL_NO_UPDATE_CHECK": null},
  "fixtures": [
    {"write": "home/.rival/config.yaml", "text": "...", "mode": "0600"},
    {"write": "home/.rival/sessions/x.json", "json": {}},
    {"mkdir": "sync"},
    {"copy_bin": "brew/opt/rival/bin/rival"},
    {"git": "work/repo", "args": ["init", "-q"]}
  ],
  "update_server": {"routes": {"/repos/1905/rival/releases/latest": {"status": 200, "json": {"tag_name": "v9.9.9"}}}},
  "normalise": [{"regex": "  (\\d+)  ", "kind": "pid"}, {"regex": "(\\d+s) ", "kind": "duration"},
                {"regex": "v(\\d+\\.\\d+\\.\\d+)", "kind": "replace", "replace": "<V>"}],
  "steps": [
    {"run": ["queue"], "cwd": "work", "stdin": "", "env": {}, "timeout": 30,
     "expect": {"exit_code": 0, "stdout": "Queue is empty.\n", "stderr_lines": [], "log_events": []}},
    {"start": "a", "run": ["codex", "review", "--detach"]},
    {"run": ["codex", "review", "--detach"], "stdin_file": "in/prompt.txt", "unlink_stdin": true,
     "stdout_file": "out/review.log", "stderr_file": "out/errors.log",
     "expect": {"exit_code": 0, "stdout": "", "stderr_lines": ["rival: detached pid=<PID1>"]}},
    {"wait_file": "sync/started", "timeout": 10},
    {"touch": "sync/go"},
    {"wait_idle": true, "timeout": 30},
    {"wait": "a", "expect": {"exit_code": 0, "stdout": "",
                             "stderr_lines": ["rival: detached pid=<PID2>"],
                             "log_events": [{"level": "info", "message": "reaping orphaned session",
                                             "fields": {"session": "<UUID1>", "pid": "<PID3>"}}]}}
  ],
  "expect": {
    "home_files": [".rival/queue/.lock"],
    "files": [{"path": "<HOME>/.rival/sessions/<UUID1>.json", "json": {"status": "completed", "exit_code": 0}},
              {"path": "<HOME>/.rival/.update-check", "absent": true}],
    "calls": {"codex": 2},
    "http_requests": [{"method": "GET", "path": "/repos/1905/rival/releases/latest"}]
  }
}
```

- Inputs: `<ROOT>`, `<HOME>` and `<BIN>` expand to real paths in run args, `stdin`, `env`, fixtures and fake scripts. Inside `$regex` they expand escaped.
- Fixture and sync paths are relative to the root and may not leave it.
- `system_tools` lists extra host tools. `git` is always linked; listing it, or any tool twice, is an error.
- `git` fixtures run the system git with `protocol.allow=never`, a fixed identity and a fixed date. `clone`, `fetch`, `pull`, `push`, `ls-remote`, `submodule`, `archive` and `bundle` are rejected.
- Steps:
  - `run`: runs in the foreground.
  - `start`: runs in the background under a handle; a later `wait` joins it.
  - `wait_file`: blocks until a file exists.
  - `touch`: creates a file.
  - `wait_idle`: blocks until no task process is alive (tracked steps and token carriers). Use it to sync on detached children.
- Redirects of `run` and `start` (all paths relative to the root, never inside `.parity/`):
  - `stdin`: literal text. `stdin_file`: an existing regular file, for example a `write` fixture. Set at most one.
  - `unlink_stdin: true` (needs `stdin_file`): the runner unlinks that file once the parent has exited (`run`: right after it exits; `start`: at its `wait`). A detached child must keep reading through its inherited descriptor.
  - `stdout_file`, `stderr_file`: arbitrary new file names. The path must not exist yet, and no two steps may share one. Default: `.parity/steps/NN.stdout` and `.stderr`.
- Step stdout and stderr go to files, so a detached child that inherits them appends to the same step output.
- `expect` in a step: `exit_code` and `stdout` are required. `stderr_lines` and `log_events` default to empty, which means none are allowed.
- The exit code is checked when the parent exits. Stdout, stderr lines and log events are checked only at the end, after every task process has exited or been killed. Output a detached child writes late is still checked, named redirect files included.
- Stderr is split per line: a line that parses as a JSON object is a log event, and every other line (blank lines too) is a plain line.
- Plain lines must equal `stderr_lines` in order. Examples: `rival: detached pid=<PID1>`, errors printed by `cmd/root.go` `Execute`, and the `Update available:` notice of `internal/update/check.go` with its blank line before and after.
- Log events must have zerolog's shape (`level`, `app: "rival"`, RFC 3339 `time`). They match `log_events` as a multiset.
- Each expected event names `level` and `message`, plus required `fields`. Extra fields in an event are allowed. An extra event fails the scenario.
- Final `expect`:
  - `home_files` (required): the exact file list under `HOME`.
  - `files`: `json` (subset match), `text` (exact) or `absent` for a normalised path.
  - `calls`: exact call counts per fake. A fake not listed must not be called.
  - `http_requests`: the update server requests as a multiset. Empty by default.
- Expected values compare by type. `{"$regex": ...}` is the only wildcard, and it must be written out.

## Normalisation (`normalise.py`)

- Temp paths become `<HOME>`, `<ROOT>` and `<BIN>`.
- UUIDs become `<UUID1>`, `<UUID2>`... in first-seen order: step outputs in the order their parents were joined (a `run` when it exits, a `start` at its `wait`), then files under `HOME` by modification time. One value always gets one marker.
- PIDs become `<PID1>`... only for JSON keys `pid`/`owner_pid` and text `pid=N`. `pid_start`/`owner_pid_start` become `<PIDSTART1>`...
- RFC 3339 times become `<TIME>` with no index. Go writes them at second precision, so whether two are equal depends on timing. The JSON key `duration` becomes `<DURATION>`.
- Other numbers stay exact: exit codes, ratings, line numbers, byte and line counts.
- Scenario `normalise` rules add patterns for anything else, for example a PID column or a wait duration. Each rule replaces its single regex group.
