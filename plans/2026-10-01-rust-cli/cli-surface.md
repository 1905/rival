# CLI surface (Go reference, dev)

Generated from `rival <cmd> --help`. Parity target for the Rust CLI.

### rival 
```
Dispatch prompts and reviews to external AI models

Usage:
  rival [flags]
  rival [command]

Available Commands:
  command     Skill-facing command (reads raw args from stdin, parses, executes)
  completion  Generate the autocompletion script for the specified shell
  help        Help about any command
  install     Install skills for Claude Code and Codex
  queue       Inspect the review queue
  run         Run a CLI executor directly (terminal use)
  sessions    List sessions
  tui         Launch the TUI dashboard
  update      Update rival to the latest version via Homebrew
  version     Print rival version
  wait        Block until review session(s) finish; exit code reflects the outcome

Flags:
  -h, --help   help for rival

Use "rival [command] --help" for more information about a command.
```

### rival command
```
Used by Rival skills. Reads raw slash-command arguments from stdin, parses them, executes the selected model, and prints the final output.

Usage:
  rival command [flags]
  rival command [command]

Available Commands:
  antislop    Quality-only slop & over-engineering review (code or plan)
  claude      Skill-facing Claude executor
  codex       Skill-facing Codex executor
  grok        Skill-facing Grok executor
  k3          Run Kimi K3 prompts from stdin
  plan        Review a plan/spec with Codex and/or Claude
  security    Security review with the configured model

Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
  -h, --help     help for command

Use "rival command [command] --help" for more information about a command.
```

### rival command antislop
```
Quality-only slop & over-engineering review (code or plan)

Usage:
  rival command antislop [flags]

Flags:
      --effort string    override reasoning effort for every selected model: low, medium, high, xhigh, ultra
  -h, --help             help for antislop
  -m, --model strings    antislop model(s): codex, claude (comma-separated). Default models are codex and claude (default [codex,claude])
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command claude
```
Skill-facing Claude executor

Usage:
  rival command claude [flags]

Flags:
  -h, --help             help for claude
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command codex
```
Skill-facing Codex executor

Usage:
  rival command codex [flags]

Flags:
  -h, --help             help for codex
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command grok
```
Skill-facing Grok executor

Usage:
  rival command grok [flags]

Flags:
  -h, --help             help for grok
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command k3
```
Run Kimi K3 prompts from stdin

Usage:
  rival command k3 [flags]

Flags:
  -h, --help             help for k3
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command plan
```
Review a plan/spec with Codex and/or Claude

Usage:
  rival command plan [flags]

Flags:
      --effort string    override reasoning effort for every selected model: low, medium, high, xhigh, ultra (default: each model's own)
  -h, --help             help for plan
  -m, --model strings    plan review model(s): codex, claude (comma-separated) (default [codex])
      --no-queue         bypass the review queue
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival command security
```
Security review with the configured model

Usage:
  rival command security [flags]

Flags:
  -h, --help             help for security
      --no-queue         bypass the review queue
      --which            print the resolved model and exit
      --workdir string   working directory (default ".")

Global Flags:
      --detach   run detached in a new process session; prints 'rival: detached pid=N' and exits
```

### rival install
```
Install skills for Claude Code and Codex

Usage:
  rival install [flags]

Flags:
      --force           overwrite without prompting
  -h, --help            help for install
      --target string   skill host: auto, claude, codex, all (default "auto")
```

### rival queue
```
Inspect the review queue

Usage:
  rival queue [flags]
  rival queue [command]

Available Commands:
  clear       Remove dead queue tickets (--force removes all)

Flags:
  -h, --help   help for queue

Use "rival queue [command] --help" for more information about a command.
```

### rival queue clear
```
Remove dead queue tickets (--force removes all)

Usage:
  rival queue clear [flags]

Flags:
      --force   remove ALL tickets, not just dead ones
  -h, --help    help for clear
```

### rival run
```
Execute a model runner with explicit flags and stream output to stdout.

Usage:
  rival run [flags]
  rival run [command]

Available Commands:
  claude      Run Claude
  grok        Run Grok
  k3          Run Kimi K3 (via opencode)

Flags:
  -h, --help   help for run

Use "rival run [command] --help" for more information about a command.
```

### rival run claude
```
Run Claude

Usage:
  rival run claude [flags]

Flags:
      --effort string    reasoning effort override (low, medium, high, xhigh)
  -h, --help             help for claude
      --no-queue         bypass the review queue
      --prompt-stdin     read prompt from stdin
      --review string    review scope (enables review mode)
      --workdir string   working directory (default ".")
```

### rival run grok
```
Run Grok

Usage:
  rival run grok [flags]

Flags:
      --effort string    reasoning effort override: low, medium, high (ultra clamps to high)
  -h, --help             help for grok
      --no-queue         bypass the review queue
      --prompt-stdin     read prompt from stdin
      --review string    review scope (enables review mode)
      --workdir string   working directory (default ".")
```

### rival run k3
```
Run Kimi K3 (via opencode)

Usage:
  rival run k3 [flags]

Flags:
  -h, --help             help for k3
      --no-queue         bypass the review queue
      --prompt-stdin     read prompt from stdin
      --review string    review scope (enables review mode)
      --workdir string   working directory (default ".")
```

### rival sessions
```
List sessions

Usage:
  rival sessions [flags]

Flags:
      --active       show only running sessions
  -h, --help         help for sessions
      --recent int   show N most recent sessions
```

### rival tui
```
Launch the TUI dashboard

Usage:
  rival tui [flags]

Flags:
  -h, --help   help for tui
```

### rival update
```
Update rival to the latest version via Homebrew

Usage:
  rival update [flags]

Flags:
  -h, --help   help for update
```

### rival version
```
Print rival version

Usage:
  rival version [flags]

Flags:
  -h, --help   help for version
```

### rival wait
```
Wait for one or more rival review sessions to reach a terminal state.

Two modes:

  rival wait --log <stderr-file>   (used by skills)
      Parse the detached rival PID and session IDs from a run's stderr file,
      poll the rival process for liveness, then summarize the sessions when it
      exits. Detects a crashed rival (process dead, sessions not finalized).

  rival wait <session-id>...       (terminal-status only)
      Poll the named sessions' JSON until all reach a terminal state.
      Note: a session is marked terminal moments before its output is flushed
      to the launching command's stdout; prefer --log when that matters.

Exit codes: 0 all completed · 2 some failed · 3 rival crashed · 4 timed out.

Usage:
  rival wait [session-id...] [flags]

Flags:
  -h, --help               help for wait
      --log string         stderr file of a detached run to parse pid + session IDs from
      --poll duration      poll interval (default 2s)
      --timeout duration   give up waiting after this long (default 1h35m0s)
```
