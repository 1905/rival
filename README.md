# Rival

<img src="assets/tui.png" width="100%" alt="Rival terminal UI showing sessions and review output">

Rival sends your code to another AI model for an independent review. It runs in the background while you keep working.

Browse sessions and findings in the terminal UI. Review changed code with Codex or Claude, or run a dedicated security review.

## Install

On macOS or Linux:

```bash
brew install 1905/tap/rival
rival install
```

Windows binaries and manual downloads are available on the [releases page](https://github.com/1905/rival/releases).

## The /rival skill

`rival install` provides one skill, `/rival`, for Claude Code and for Codex.

| Command | What it does |
|---|---|
| `/rival review` | Review the changed code. |
| `/rival review src/api/` | Review a specific scope. |
| `/rival -re high review` | Review with a different reasoning effort. |
| `/rival 'explain the auth flow'` | Run any prompt. |
| `/rival security` | Look for exploitable security vulnerabilities. |

By default, each host gets a review from the other vendor: `/rival` in Claude
Code runs Sol 6.1, and `/rival` in Codex runs Opus 5.5. To pick the models,
open the config window (`rival config`, Models), or run
`rival config set skill.claude <model>` and `rival config set skill.codex <model>`.
The models are `codex`, `sol`, `claude`, `fable`, `k3` and `grok`. The security
review uses the model in `security.reviewer`.

Codex (GPT-6 Astra) and Sol 6.1 use the Codex Ultrafast tier by default.
Ultrafast needs a Codex account on Pro $500, or an Enterprise/Edu workspace
with Ultrafast on. Without it, Codex uses the standard tier and Rival prints a
notice. To turn the tier off, run `rival config set ultrafast.codex false` or
`rival config set ultrafast.sol false`.

Version 6.0.0 replaces the per-model skills (`/rival-codex`, `/rival-sol`,
`/rival-claude`, `/rival-fable`, `/rival-k3`, `/rival-grok` and
`/rival-security`); `rival install` and `rival update` remove them. Plan
reviews remain available through `rival command plan`. See the
[skill migration details](docs/command-reference.md#skills).

## View the results

Run `rival tui` to browse reviews and their findings.

## Reference

Full installation steps, commands, configuration, and platform requirements are in the [command reference](docs/command-reference.md).

## License

[MIT](LICENSE)
