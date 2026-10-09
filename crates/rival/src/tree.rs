//! The command tree, defined once in clap: names, flags, defaults, help and
//! completion scripts. clap parses the command line.
//!
//! The clap settings below match the cobra/pflag rules rival relies on:
//! value flags take the next word even when it starts with `-`, bools take
//! only `--flag=value`, the last value wins, `--model` values append, and
//! leaf commands accept stray words. A narrow adapter then applies the
//! cobra rules clap has no setting for (help lookup, non-runnable commands,
//! `NoArgs`) and rewrites clap errors into the exact cobra/pflag texts with
//! exit code 1. Help prose and completion scripts may differ from cobra;
//! errors and accepted input may not.

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use rival_core::config::OPUS_ALIAS;
use rival_core::duration;

use crate::csvflag;

#[cfg(test)]
mod tests;

/// Every command in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    Root,
    Command,
    CommandClaude,
    CommandCodex,
    CommandFable,
    CommandGrok,
    CommandK3,
    CommandPlan,
    CommandSecurity,
    CommandSol,
    Config,
    ConfigShow,
    ConfigSet,
    ConfigKey,
    ConfigKeySet,
    ConfigKeyClear,
    ConfigModels,
    Install,
    Queue,
    QueueClear,
    Run,
    RunClaude,
    RunFable,
    RunGrok,
    RunK3,
    Sessions,
    Tui,
    Update,
    Version,
    Wait,
    Help,
    Completion,
    CompletionBash,
    CompletionZsh,
    CompletionFish,
    CompletionPowershell,
}

impl CommandId {
    fn from_path(path: &[&str]) -> Option<CommandId> {
        use CommandId::*;
        Some(match path {
            ["rival"] => Root,
            ["rival", "command"] => Command,
            ["rival", "command", "claude"] => CommandClaude,
            ["rival", "command", "codex"] => CommandCodex,
            ["rival", "command", "fable"] => CommandFable,
            ["rival", "command", "grok"] => CommandGrok,
            ["rival", "command", "k3"] => CommandK3,
            ["rival", "command", "plan"] => CommandPlan,
            ["rival", "command", "security"] => CommandSecurity,
            ["rival", "command", "sol"] => CommandSol,
            ["rival", "config"] => Config,
            ["rival", "config", "show"] => ConfigShow,
            ["rival", "config", "set"] => ConfigSet,
            ["rival", "config", "key"] => ConfigKey,
            ["rival", "config", "key", "set"] => ConfigKeySet,
            ["rival", "config", "key", "clear"] => ConfigKeyClear,
            ["rival", "config", "models"] => ConfigModels,
            ["rival", "install"] => Install,
            ["rival", "queue"] => Queue,
            ["rival", "queue", "clear"] => QueueClear,
            ["rival", "run"] => Run,
            ["rival", "run", "claude"] => RunClaude,
            ["rival", "run", "fable"] => RunFable,
            ["rival", "run", "grok"] => RunGrok,
            ["rival", "run", "k3"] => RunK3,
            ["rival", "sessions"] => Sessions,
            ["rival", "tui"] => Tui,
            ["rival", "update"] => Update,
            ["rival", "version"] => Version,
            ["rival", "wait"] => Wait,
            ["rival", "help"] => Help,
            ["rival", "completion"] => Completion,
            ["rival", "completion", "bash"] => CompletionBash,
            ["rival", "completion", "zsh"] => CompletionZsh,
            ["rival", "completion", "fish"] => CompletionFish,
            ["rival", "completion", "powershell"] => CompletionPowershell,
            _ => return None,
        })
    }

    /// cobra `Args: cobra.NoArgs`: stray words are an unknown command.
    fn no_args(self) -> bool {
        use CommandId::*;
        matches!(
            self,
            Command
                | Run
                | Config
                | ConfigShow
                | ConfigKey
                | ConfigKeyClear
                | ConfigModels
                | Install
                | Completion
                | CompletionBash
                | CompletionZsh
                | CompletionFish
                | CompletionPowershell
        )
    }

    /// cobra `Runnable()`: `completion` has no action and prints help.
    fn runnable(self) -> bool {
        self != CommandId::Completion
    }

    /// Commands that run with an invalid `config.yaml`, so they can fix
    /// it.
    pub fn repairs_config(self) -> bool {
        use CommandId::*;
        matches!(self, ConfigSet | ConfigKeySet | ConfigKeyClear)
    }
}

/// Values computed at startup, before `.env` loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    /// `rival wait --timeout` default (`config.MaxRunWait()`), nanoseconds.
    pub wait_timeout: i64,
}

/// The positional args of a command (`cmd.Flags().Args()`). Hidden: cobra
/// accepts and ignores them on leaf commands.
const ARGS: &str = "args";

fn command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .args_override_self(true)
        .arg(bool_flag("help", format!("help for {name}")).short('h'))
}

/// A command whose stray words are kept for the action or the `NoArgs`
/// check.
fn with_args(cmd: Command) -> Command {
    cmd.arg(
        Arg::new(ARGS)
            .num_args(0..)
            .action(ArgAction::Append)
            .hide(true),
    )
}

/// pflag bool: `--flag`, `--flag=<strconv.ParseBool value>`; never takes
/// the next word.
fn bool_flag(long: &'static str, help: impl Into<String>) -> Arg {
    Arg::new(long)
        .long(long)
        .help(help.into())
        .num_args(0..=1)
        .require_equals(true)
        .default_value("false")
        .default_missing_value("true")
        .value_parser(parse_bool)
        .action(ArgAction::Set)
}

/// pflag string/duration/int: takes the next word, even one starting with
/// `-`; the last occurrence wins.
fn value_flag(
    long: &'static str,
    value_name: &'static str,
    default: String,
    help: &'static str,
) -> Arg {
    let hide = default.is_empty();
    Arg::new(long)
        .long(long)
        .help(help)
        .value_name(value_name)
        .num_args(1)
        .allow_hyphen_values(true)
        .default_value(default)
        .hide_default_value(hide)
        .action(ArgAction::Set)
}

fn string_flag(long: &'static str, default: &str, help: &'static str) -> Arg {
    value_flag(long, "string", default.to_string(), help)
}

fn duration_flag(long: &'static str, default: i64, help: &'static str) -> Arg {
    value_flag(long, "duration", duration::format(default), help).value_parser(duration::parse)
}

/// pflag `StringSliceP`: each value is one CSV record; the first occurrence
/// replaces the default, later ones append.
fn model_flag(default: &'static str, help: &'static str) -> Arg {
    value_flag("model", "strings", default.to_string(), help)
        .short('m')
        .value_parser(csvflag::read_as_csv)
        .action(ArgAction::Append)
}

fn workdir() -> Arg {
    string_flag("workdir", ".", "working directory")
}

fn no_queue() -> Arg {
    bool_flag("no-queue", "bypass the review queue")
}

fn effort(help: &'static str) -> Arg {
    string_flag("effort", "", help)
}

fn model_command(name: &'static str, about: &'static str) -> Command {
    with_args(command(name, about)).args([workdir(), no_queue()])
}

fn run_command(
    name: &'static str,
    about: &'static str,
    effort_help: Option<&'static str>,
) -> Command {
    let mut cmd = with_args(command(name, about));
    if let Some(help) = effort_help {
        cmd = cmd.arg(effort(help));
    }
    cmd.args([
        workdir(),
        bool_flag("prompt-stdin", "read prompt from stdin"),
        string_flag("review", "", "review scope (enables review mode)"),
        no_queue(),
    ])
}

fn shell(name: &'static str) -> Command {
    let about = match name {
        "bash" => "Generate the autocompletion script for bash",
        "zsh" => "Generate the autocompletion script for zsh",
        "fish" => "Generate the autocompletion script for fish",
        _ => "Generate the autocompletion script for powershell",
    };
    with_args(command(name, about)).arg(bool_flag(
        "no-descriptions",
        "disable completion descriptions",
    ))
}

/// The whole tree. Child order is the old cobra registration order (by
/// source file name), then `help` and `completion`. That order decides the
/// suggestion order.
pub fn build(defaults: &Defaults) -> Command {
    let command_cmd = with_args(command(
        "command",
        "Skill-facing command (reads raw args from stdin, parses, executes)",
    ))
    .long_about("Used by Rival skills. Reads raw slash-command arguments from stdin, parses them, executes the selected model, and prints the final output.")
    .arg(
        bool_flag(
            "detach",
            "run detached in a new process session; prints 'rival: detached pid=N' and exits",
        )
        .global(true),
    )
    .subcommands([
        // `opus` is another name for `claude`; clap reports the canonical
        // name, so the command path stays `command claude`.
        model_command("claude", "Skill-facing Claude executor").visible_alias(OPUS_ALIAS),
        model_command("codex", "Skill-facing Codex executor"),
        model_command("fable", "Skill-facing Fable executor"),
        model_command("grok", "Skill-facing Grok executor"),
        model_command("k3", "Run Kimi K3 prompts from stdin"),
        with_args(command("plan", "Review a plan/spec with Codex, Sol, Claude and/or Fable")).args([
            workdir(),
            no_queue(),
            model_flag("codex", "plan review model(s): codex, sol, claude (or opus), fable (comma-separated; default: plan.models, else codex)"),
            effort("override reasoning effort for every selected model: low, medium, high, xhigh, ultra (default: each model's own)"),
        ]),
        with_args(command("security", "Security review with the configured model")).args([
            workdir(),
            no_queue(),
            bool_flag("which", "print the resolved model and exit"),
        ]),
        model_command("sol", "Skill-facing Sol executor"),
    ]);

    // `key set` keeps its stray words: the action refuses them without
    // echoing them, since a stray word may be the key.
    let config_cmd = with_args(command(
        "config",
        "Show and change ~/.rival/config.yaml (no subcommand: open the TUI)",
    ))
    .subcommands([
        with_args(command(
            "show",
            "Show each resolved value and its source (default, file, env)",
        ))
        .arg(bool_flag("json", "print one JSON object")),
        with_args(command(
            "set",
            "Set one key (KEY VALUE), or apply a JSON patch from stdin (--json)",
        ))
        .arg(bool_flag("json", "read a JSON patch from stdin")),
        with_args(command("key", "Store or remove the proxy key")).subcommands([
            with_args(command(
                "set",
                "Store the proxy key read from stdin (mode 0600)",
            )),
            with_args(command("clear", "Remove the proxy key file")),
        ]),
        with_args(command("models", "List the models the proxy serves"))
            .arg(bool_flag("json", "print one JSON object")),
    ]);

    command("rival", "Dispatch prompts and reviews to external AI models")
        .bin_name("rival")
        .subcommands([
            command_cmd,
            config_cmd,
            with_args(command("install", "Install skills for Claude Code and Codex")).args([
                bool_flag("force", "overwrite without prompting"),
                string_flag("target", "auto", "skill host: auto, claude, codex, all"),
            ]),
            with_args(command("queue", "Inspect the review queue")).subcommand(
                with_args(command("clear", "Remove dead queue tickets (--force also removes waiting ones)"))
                    .arg(bool_flag("force", "also remove live waiting tickets; live running tickets stay")),
            ),
            with_args(command("run", "Run a CLI executor directly (terminal use)"))
                .long_about("Execute a model runner with explicit flags and stream output to stdout.")
                .subcommands([
                    run_command(
                        "claude",
                        "Run Claude",
                        Some("reasoning effort override (low, medium, high, xhigh)"),
                    )
                    .visible_alias(OPUS_ALIAS),
                    run_command(
                        "fable",
                        "Run Fable",
                        Some("reasoning effort override (low, medium, high, xhigh)"),
                    ),
                    run_command(
                        "grok",
                        "Run Grok",
                        Some("reasoning effort override: low, medium, high (ultra clamps to high)"),
                    ),
                    run_command("k3", "Run Kimi K3 (via opencode)", None),
                ]),
            with_args(command("sessions", "List sessions")).args([
                bool_flag("active", "show only running sessions"),
                value_flag("recent", "int", "0".into(), "show N most recent sessions")
                    .value_parser(parse_int),
            ]),
            with_args(command("tui", "Launch the TUI dashboard")),
            with_args(command("update", "Update rival to the latest version via Homebrew")),
            with_args(command("version", "Print rival version")),
            command(
                "wait",
                "Block until review session(s) finish; exit code reflects the outcome",
            )
            .long_about(WAIT_LONG)
            .args([
                Arg::new(ARGS)
                    .value_name("session-id")
                    .num_args(0..)
                    .action(ArgAction::Append),
                string_flag(
                    "log",
                    "",
                    "stderr file of a detached run to parse pid + session IDs from",
                ),
                duration_flag("timeout", defaults.wait_timeout, "give up waiting after this long"),
                duration_flag("poll", crate::wait::DEFAULT_POLL, "poll interval"),
            ]),
            with_args(command("help", "Help about any command")).long_about(
                "Help provides help for any command in the application.\nSimply type rival help [path to command] for full details.",
            ),
            with_args(command(
                "completion",
                "Generate the autocompletion script for the specified shell",
            ))
            .long_about("Generate the autocompletion script for rival for the specified shell.\nSee each sub-command's help for details on how to use the generated script.")
            .subcommands(["bash", "zsh", "fish", "powershell"].map(shell)),
        ])
}

const WAIT_LONG: &str = "Wait for one or more rival review sessions to reach a terminal state.

Two modes:

  rival wait --log <stderr-file>   (used by skills)
      Parse the detached rival PID and session IDs from a run's stderr file,
      poll the rival process for liveness, then summarize the sessions when it
      exits. Detects a crashed rival (process dead, sessions not finalized).
      After the summary it prints \"auto-fix: off\" or \"auto-fix: critical+high\"
      (auto_fix_critical_high in ~/.rival/config.yaml).

  rival wait <session-id>...       (terminal-status only)
      Poll the named sessions' JSON until all reach a terminal state.
      Note: a session is marked terminal moments before its output is flushed
      to the launching command's stdout; prefer --log when that matters.

Exit codes: 0 all completed · 2 some failed · 3 rival crashed · 4 timed out.";

/// A command ready to run: the found command, its flags and its positional
/// args.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub id: CommandId,
    /// Names from the root, e.g. `["rival", "command", "codex"]`.
    pub path: Vec<String>,
    matches: ArgMatches,
    pub args: Vec<String>,
}

impl Invocation {
    /// cobra `CommandPath()`.
    pub fn command_path(&self) -> String {
        self.path.join(" ")
    }

    /// `GetBool`: false when the command has no such flag.
    pub fn bool(&self, name: &str) -> bool {
        matches!(self.matches.try_get_one::<bool>(name), Ok(Some(true)))
    }

    /// `GetString`: "" when absent.
    pub fn string(&self, name: &str) -> String {
        match self.matches.try_get_one::<String>(name) {
            Ok(Some(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// `GetDuration` (nanoseconds) or `GetInt`: 0 when absent.
    pub fn duration(&self, name: &str) -> i64 {
        self.matches
            .try_get_one::<i64>(name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(0)
    }

    /// `GetInt`: 0 when absent.
    #[allow(dead_code, reason = "read by the Task 3.4 sessions action")]
    pub fn int(&self, name: &str) -> i64 {
        self.duration(name)
    }

    /// `GetStringSlice`: every occurrence's CSV fields, in order.
    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.matches.try_get_many::<Vec<String>>(name) {
            Ok(Some(values)) => values.flatten().cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// `Flags().Changed(name)`.
    pub fn changed(&self, name: &str) -> bool {
        matches!(self.matches.try_contains_id(name), Ok(true))
            && self.matches.value_source(name) == Some(ValueSource::CommandLine)
    }
}

/// What the command line asks for.
#[derive(Debug, Clone)]
pub enum Parsed {
    Run(Invocation),
    /// Print this command's help on stdout.
    Help(Vec<String>),
}

/// Parses `args` (no program name) against `root`. `Err` holds the exact
/// cobra/pflag error text; every such error exits 1.
pub fn parse(root: &mut Command, args: &[String]) -> Result<Parsed, String> {
    let argv = std::iter::once("rival".to_string()).chain(args.iter().cloned());
    let matches = root
        .try_get_matches_from_mut(argv)
        .map_err(|e| cobra_error(&e, root, args))?;

    let mut path = vec!["rival".to_string()];
    let mut chain = vec![&matches];
    let mut cur = &matches;
    while let Some((name, sub)) = cur.subcommand() {
        path.push(name.to_string());
        chain.push(sub);
        cur = sub;
    }
    // cobra's lookup treats `-h` as an unknown flag that swallows the next
    // word, so the first command whose own -h/--help is set is the one
    // whose help prints.
    for (depth, m) in chain.iter().enumerate() {
        if matches!(m.try_get_one::<bool>("help"), Ok(Some(true))) {
            return Ok(Parsed::Help(path[..=depth].to_vec()));
        }
    }
    let names: Vec<&str> = path.iter().map(String::as_str).collect();
    let id = CommandId::from_path(&names).expect("every tree path has an id");
    if !id.runnable() {
        return Ok(Parsed::Help(path));
    }
    let args: Vec<String> = match cur.try_get_many::<String>(ARGS) {
        Ok(Some(values)) => values.cloned().collect(),
        _ => Vec::new(),
    };
    if id.no_args()
        && let Some(first) = args.first()
    {
        return Err(format!(
            "unknown command {:?} for {:?}",
            first,
            path.join(" ")
        ));
    }
    Ok(Parsed::Run(Invocation {
        id,
        path,
        matches: cur.clone(),
        args,
    }))
}

/// Rewrites a clap error into cobra/pflag's text.
fn cobra_error(err: &clap::Error, root: &Command, raw: &[String]) -> String {
    let ctx = |kind| match err.get(kind) {
        Some(ContextValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    match err.kind() {
        // Only the root has no positional args: its stray word is cobra's
        // legacyArgs error, with suggestions.
        ErrorKind::InvalidSubcommand => {
            let typed = ctx(ContextKind::InvalidSubcommand);
            format!(
                "unknown command {:?} for \"rival\"{}",
                typed,
                suggestions(root, &typed)
            )
        }
        ErrorKind::UnknownArgument => {
            let arg = ctx(ContextKind::InvalidArg);
            if arg == "--" || arg.starts_with("---") {
                // `--=x` or `---x`.
                let token = raw
                    .iter()
                    .find(|t| t.starts_with("--=") || t.starts_with("---"))
                    .cloned()
                    .unwrap_or(arg);
                return format!("bad flag syntax: {token}");
            }
            if let Some(long) = arg.strip_prefix("--") {
                return format!("unknown flag: --{}", long.split('=').next().unwrap_or(long));
            }
            let c = arg.chars().nth(1).unwrap_or('-');
            format!(
                "unknown shorthand flag: {} in -{}",
                quote_char(c),
                short_cluster_rest(raw, c)
            )
        }
        // A value flag at the end of the line.
        ErrorKind::InvalidValue if ctx(ContextKind::InvalidValue).is_empty() => {
            let token = raw.last().cloned().unwrap_or_default();
            if token.starts_with("--") {
                format!("flag needs an argument: {token}")
            } else {
                let c = token.chars().last().unwrap_or('-');
                format!("flag needs an argument: {} in -{c}", quote_char(c))
            }
        }
        ErrorKind::ValueValidation | ErrorKind::InvalidValue => {
            let value = ctx(ContextKind::InvalidValue);
            let flag = flag_error_name(root, &ctx(ContextKind::InvalidArg));
            let cause = std::error::Error::source(err)
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("invalid argument {:?} for {:?} flag: {cause}", value, flag)
        }
        // Not produced by this tree; keep clap's text, still exit 1.
        _ => err
            .render()
            .to_string()
            .trim_start_matches("error: ")
            .trim_end()
            .to_string(),
    }
}

/// pflag's flag name in value errors: `-m, --model` or `--workdir`. `shown`
/// is clap's `--name <value>` display.
fn flag_error_name(root: &Command, shown: &str) -> String {
    let long = shown
        .trim_start_matches('-')
        .split(['[', ' ', '='])
        .next()
        .unwrap_or_default()
        .to_string();
    match find_short(root, &long) {
        Some(c) => format!("-{c}, --{long}"),
        None => format!("--{long}"),
    }
}

fn find_short(cmd: &Command, long: &str) -> Option<char> {
    cmd.get_arguments()
        .find(|a| a.get_long() == Some(long))
        .and_then(Arg::get_short)
        .or_else(|| cmd.get_subcommands().find_map(|s| find_short(s, long)))
}

/// The rest of the short-flag cluster from `c` on (pflag prints the
/// cluster from the failing shorthand: `-hx` fails as `-x`).
fn short_cluster_rest(raw: &[String], c: char) -> String {
    raw.iter()
        .take_while(|t| t.as_str() != "--")
        .filter(|t| t.starts_with('-') && !t.starts_with("--"))
        .find_map(|t| t[1..].find(c).map(|i| t[1 + i..].to_string()))
        .unwrap_or_else(|| c.to_string())
}

/// cobra `findSuggestions` with the default minimum distance 2. `help` is
/// never suggested.
fn suggestions(root: &Command, typed: &str) -> String {
    let typed = typed.to_lowercase();
    let found: Vec<&str> = root
        .get_subcommands()
        .map(Command::get_name)
        .filter(|name| *name != "help")
        .filter(|name| {
            let name = name.to_lowercase();
            levenshtein(&typed, &name) <= 2 || name.starts_with(&typed)
        })
        .collect();
    if found.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nDid you mean this?\n");
    for name in found {
        out.push_str(&format!("\t{name}\n"));
    }
    out
}

/// cobra `ld`: byte-wise Levenshtein distance.
fn levenshtein(s: &str, t: &str) -> usize {
    let (s, t) = (s.as_bytes(), t.as_bytes());
    let mut prev: Vec<usize> = (0..=s.len()).collect();
    for j in 1..=t.len() {
        let mut cur = vec![j; s.len() + 1];
        for i in 1..=s.len() {
            cur[i] = if s[i - 1] == t[j - 1] {
                prev[i - 1]
            } else {
                1 + prev[i].min(cur[i - 1]).min(prev[i - 1])
            };
        }
        prev = cur;
    }
    prev[s.len()]
}

/// A shorthand byte as a quoted character literal.
fn quote_char(c: char) -> String {
    match c {
        '\'' => "'\\''".to_string(),
        '\\' => "'\\\\'".to_string(),
        '\x07' => "'\\a'".to_string(),
        '\x08' => "'\\b'".to_string(),
        '\x0c' => "'\\f'".to_string(),
        '\n' => "'\\n'".to_string(),
        '\r' => "'\\r'".to_string(),
        '\t' => "'\\t'".to_string(),
        '\x0b' => "'\\v'".to_string(),
        c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("'\\x{:02x}'", c as u32),
        c => format!("'{c}'"),
    }
}

/// Parses a bool flag value (`1`, `t`, `true`, …), with a
/// `strconv.ParseBool` error text.
pub fn parse_bool(s: &str) -> Result<bool, String> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(format!(
            "strconv.ParseBool: parsing {:?}: invalid syntax",
            s
        )),
    }
}

/// Parses an i64 with base prefixes and underscores, with a `strconv.ParseInt`
/// error text.
pub fn parse_int(s0: &str) -> Result<i64, String> {
    let syntax = || format!("strconv.ParseInt: parsing {:?}: invalid syntax", s0);
    let range = || format!("strconv.ParseInt: parsing {:?}: value out of range", s0);
    let (neg, s) = match s0.as_bytes().first() {
        None => return Err(syntax()),
        Some(b'+') => (false, &s0[1..]),
        Some(b'-') => (true, &s0[1..]),
        _ => (false, s0),
    };
    // ParseUint(s, 0, 64).
    let b = s.as_bytes();
    if b.is_empty() {
        return Err(syntax());
    }
    let prefix = |p: u8| b.len() >= 3 && b[1].eq_ignore_ascii_case(&p);
    let (base, digits): (u64, &[u8]) = if b[0] != b'0' {
        (10, b)
    } else if prefix(b'b') {
        (2, &b[2..])
    } else if prefix(b'o') {
        (8, &b[2..])
    } else if prefix(b'x') {
        (16, &b[2..])
    } else {
        (8, &b[1..])
    };
    let cutoff = u64::MAX / base + 1;
    let mut n: u64 = 0;
    let mut underscores = false;
    for &c in digits {
        let d = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => c - b'0',
            _ if c.is_ascii_alphabetic() => c.to_ascii_lowercase() - b'a' + 10,
            _ => return Err(syntax()),
        };
        if u64::from(d) >= base {
            return Err(syntax());
        }
        if n >= cutoff {
            return Err(range());
        }
        n *= base;
        n = n.checked_add(u64::from(d)).ok_or_else(range)?;
    }
    if underscores && !underscore_ok(s) {
        return Err(syntax());
    }
    const CUTOFF: u64 = 1 << 63;
    if (!neg && n >= CUTOFF) || (neg && n > CUTOFF) {
        return Err(range());
    }
    Ok(if neg {
        (n as i64).wrapping_neg()
    } else {
        n as i64
    })
}

/// Underscores only between digits, or right
/// after a base prefix.
fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    // ^ start, 0 digit or prefix, _ underscore, ! anything else.
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if b.len() >= 2 && b[0] == b'0' && matches!(b[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = b[1].eq_ignore_ascii_case(&b'x');
    }
    for &c in &b[i..] {
        if c.is_ascii_digit() || (hex && c.is_ascii_hexdigit()) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

/// The command at `path` (names from the root) inside the built tree.
fn subcommand<'a>(root: &'a mut Command, path: &[impl AsRef<str>]) -> &'a mut Command {
    root.build();
    let mut target = root;
    for name in path.iter().skip(1) {
        target = target
            .find_subcommand_mut(name.as_ref())
            .expect("help path comes from the tree");
    }
    target
}

/// The help text for the command at `path`.
pub fn render_help(root: &mut Command, path: &[impl AsRef<str>]) -> String {
    subcommand(root, path).render_long_help().to_string()
}

/// cobra's root usage: the root help without its description.
pub fn render_usage(root: &mut Command) -> String {
    root.build();
    root.clone()
        .about(None::<&str>)
        .long_about(None::<&str>)
        .render_long_help()
        .to_string()
}

/// `cmd` with every command and flag description removed, for
/// `completion <shell> --no-descriptions`.
pub fn without_descriptions(cmd: Command) -> Command {
    cmd.about(None::<&str>)
        .long_about(None::<&str>)
        .mut_args(|a| a.help(None::<&str>).long_help(None::<&str>))
        .mut_subcommands(without_descriptions)
}

/// cobra `help [command]`'s lookup: the deepest command named by `args`,
/// or `None` when the first word names no root command (cobra's "Unknown
/// help topic"). Words after a found command that name nothing are
/// ignored, as cobra's `Find` does below the root.
pub fn help_target(root: &Command, args: &[String]) -> Option<Vec<String>> {
    let mut path = vec!["rival".to_string()];
    let mut cur = root;
    for word in args.iter().filter(|a| !a.starts_with('-')) {
        match cur.find_subcommand(word) {
            Some(sub) => {
                path.push(word.clone());
                cur = sub;
            }
            None if path.len() == 1 => return None,
            None => break,
        }
    }
    Some(path)
}

/// cobra's `%#q` of the help topic args: `[`a` `b`]`.
pub fn quote_topic(args: &[String]) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|a| {
            let backquotable = !a.contains('`')
                && !a.contains('\u{feff}')
                && a.chars().all(|c| c == '\t' || !c.is_control());
            if backquotable {
                format!("`{a}`")
            } else {
                format!("{:?}", a)
            }
        })
        .collect();
    format!("[{}]", parts.join(" "))
}
