//! The tree against `cli-surface.md`, cobra lookup and pflag behavior, and
//! the command metadata: which commands are public, model command flags
//! and removed commands.

use super::*;

const DEFAULTS: Defaults = Defaults {
    wait_timeout: 95 * 60 * 1_000_000_000,
};

fn root() -> Command {
    let mut cmd = build(&DEFAULTS);
    cmd.build();
    cmd
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn parse_ok(args: &[&str]) -> Invocation {
    match parse(&mut root(), &argv(args)) {
        Ok(Parsed::Run(inv)) => inv,
        other => panic!("{args:?}: {other:?}"),
    }
}

fn parse_err(args: &[&str]) -> String {
    match parse(&mut root(), &argv(args)) {
        Err(e) => e,
        other => panic!("{args:?} parsed: {other:?}"),
    }
}

fn help_path(args: &[&str]) -> Vec<String> {
    match parse(&mut root(), &argv(args)) {
        Ok(Parsed::Help(path)) => path,
        other => panic!("{args:?}: {other:?}"),
    }
}

fn node<'a>(root: &'a Command, path: &[&str]) -> &'a Command {
    let mut n = root;
    for name in path {
        n = n
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("no {name} under {}", n.get_name()));
    }
    n
}

fn names(cmd: &Command) -> Vec<&str> {
    cmd.get_subcommands().map(Command::get_name).collect()
}

/// (long, short, default) of a command's visible flags, without -h/--help
/// and without inherited globals.
fn flags(cmd: &Command) -> Vec<(String, Option<char>, String)> {
    cmd.get_arguments()
        .filter(|a| !a.is_global_set() && a.get_long().is_some() && a.get_long() != Some("help"))
        .map(|a| {
            let default = a
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",");
            (a.get_long().unwrap().to_string(), a.get_short(), default)
        })
        .collect()
}

fn f(long: &str, default: &str) -> (String, Option<char>, String) {
    (long.to_string(), None, default.to_string())
}

// ---- cli-surface.md ----

#[test]
fn root_commands_match_cli_surface_including_help_and_completion() {
    let r = root();
    assert_eq!(
        names(&r),
        [
            "command",
            "config",
            "install",
            "queue",
            "run",
            "sessions",
            "tui",
            "update",
            "version",
            "wait",
            "help",
            "completion"
        ]
    );
    assert!(flags(&r).is_empty());
    let completion = node(&r, &["completion"]);
    assert_eq!(names(completion), ["bash", "zsh", "fish", "powershell"]);
    for shell in names(completion) {
        assert_eq!(
            flags(node(&r, &["completion", shell])),
            [f("no-descriptions", "false")]
        );
    }
}

#[test]
fn command_subtree_matches_cli_surface() {
    let r = root();
    let command = node(&r, &["command"]);
    assert_eq!(
        names(command),
        [
            "claude", "codex", "fable", "grok", "k3", "plan", "security", "sol"
        ]
    );
    let detach = command
        .get_arguments()
        .find(|a| a.get_long() == Some("detach"))
        .unwrap();
    assert!(detach.is_global_set());
    assert_eq!(
        detach.get_help().unwrap().to_string(),
        "run detached in a new process session; prints 'rival: detached pid=N' and exits"
    );
    for model in ["claude", "codex", "fable", "grok", "k3", "sol"] {
        assert_eq!(
            flags(node(&r, &["command", model])),
            [f("workdir", "."), f("no-queue", "false")],
            "{model}"
        );
    }
    let m = |default: &str| ("model".to_string(), Some('m'), default.to_string());
    assert_eq!(
        flags(node(&r, &["command", "plan"])),
        [
            f("workdir", "."),
            f("no-queue", "false"),
            m("codex"),
            f("effort", "")
        ]
    );
    assert_eq!(
        flags(node(&r, &["command", "security"])),
        [
            f("workdir", "."),
            f("no-queue", "false"),
            f("which", "false")
        ]
    );
}

#[test]
fn other_commands_match_cli_surface() {
    let r = root();
    assert_eq!(
        flags(node(&r, &["install"])),
        [f("force", "false"), f("target", "auto")]
    );
    assert_eq!(names(node(&r, &["queue"])), ["clear"]);
    assert_eq!(flags(node(&r, &["queue", "clear"])), [f("force", "false")]);
    assert_eq!(names(node(&r, &["run"])), ["claude", "fable", "grok", "k3"]);
    let run_flags = [
        f("effort", ""),
        f("workdir", "."),
        f("prompt-stdin", "false"),
        f("review", ""),
        f("no-queue", "false"),
    ];
    assert_eq!(flags(node(&r, &["run", "claude"])), run_flags);
    assert_eq!(flags(node(&r, &["run", "fable"])), run_flags);
    assert_eq!(flags(node(&r, &["run", "grok"])), run_flags);
    assert_eq!(flags(node(&r, &["run", "k3"])), run_flags[1..]);
    assert_eq!(
        flags(node(&r, &["sessions"])),
        [f("active", "false"), f("recent", "0")]
    );
    for leaf in ["tui", "update", "version"] {
        assert!(flags(node(&r, &[leaf])).is_empty(), "{leaf}");
    }
    assert_eq!(
        flags(node(&r, &["wait"])),
        [f("log", ""), f("timeout", "1h35m0s"), f("poll", "2s")]
    );
}

#[test]
fn config_subtree() {
    let r = root();
    let config = node(&r, &["config"]);
    assert_eq!(names(config), ["show", "set", "key", "models"]);
    assert_eq!(names(node(&r, &["config", "key"])), ["set", "clear"]);
    let json = [f("json", "false")];
    assert_eq!(flags(node(&r, &["config", "show"])), json);
    assert_eq!(flags(node(&r, &["config", "set"])), json);
    assert_eq!(flags(node(&r, &["config", "models"])), json);
    assert_eq!(parse_ok(&["config"]).id, CommandId::Config);
    let inv = parse_ok(&["config", "set", "proxy.url", "http://h"]);
    assert_eq!(
        (inv.id, inv.args.clone()),
        (CommandId::ConfigSet, argv(&["proxy.url", "http://h"]))
    );
    assert!(parse_ok(&["config", "set", "--json"]).bool("json"));
    assert_eq!(parse_ok(&["config", "key"]).id, CommandId::ConfigKey);
    // `key set` keeps stray words for its own error; the others refuse them.
    assert_eq!(parse_ok(&["config", "key", "set", "x"]).args, ["x"]);
    assert_eq!(
        parse_err(&["config", "key", "clear", "x"]),
        "unknown command \"x\" for \"rival config key clear\""
    );
    assert_eq!(
        parse_err(&["config", "bogus"]),
        "unknown command \"bogus\" for \"rival config\""
    );
    for id in [
        CommandId::ConfigSet,
        CommandId::ConfigKeySet,
        CommandId::ConfigKeyClear,
    ] {
        assert!(id.repairs_config(), "{id:?}");
    }
    assert!(!CommandId::ConfigShow.repairs_config());
    assert!(!CommandId::Config.repairs_config());
}

#[test]
fn model_commands_are_public_and_registered() {
    let r = root();
    for (parent, model) in [
        ("command", "claude"),
        ("command", "grok"),
        ("run", "claude"),
        ("run", "grok"),
    ] {
        let n = node(&r, &[parent, model]);
        assert_eq!(n.get_name(), model);
        assert!(!n.is_hide_set(), "{parent} {model} is hidden");
    }
    let help = |cmd: &Command, long: &str| {
        cmd.get_arguments()
            .find(|a| a.get_long() == Some(long))
            .and_then(|a| a.get_help().map(ToString::to_string))
            .unwrap()
    };
    // The flag accepts rival's full ladder, so the help must admit that
    // ultra is taken and clamped rather than rejected.
    assert_eq!(
        help(node(&r, &["run", "grok"]), "effort"),
        "reasoning effort override: low, medium, high (ultra clamps to high)"
    );
    assert_eq!(
        help(node(&r, &["run", "claude"]), "effort"),
        "reasoning effort override (low, medium, high, xhigh)"
    );
    let text = render_help(&mut root(), &["rival", "run"]);
    for name in ["claude", "grok", "k3"] {
        assert!(text.contains(name), "{text}");
    }
}

#[test]
fn model_command_parents_reject_unknown_runner_names() {
    assert_eq!(
        parse_err(&["run", "retired-runner"]),
        "unknown command \"retired-runner\" for \"rival run\""
    );
    assert_eq!(
        parse_err(&["command", "retired-runner"]),
        "unknown command \"retired-runner\" for \"rival command\""
    );
    assert_eq!(parse_ok(&["run"]).id, CommandId::Run);
    assert_eq!(parse_ok(&["command"]).id, CommandId::Command);
}

/// Commands removed on 2026-09-26 must not resolve: `rival review`,
/// `rival command megareview`, the Sol commands and the web dashboard.
#[test]
fn removed_review_commands_do_not_resolve() {
    assert!(parse_err(&["review"]).starts_with("unknown command \"review\" for \"rival\""));
    assert!(parse_err(&["server"]).starts_with("unknown command \"server\" for \"rival\""));
    assert_eq!(
        parse_err(&["command", "megareview"]),
        "unknown command \"megareview\" for \"rival command\""
    );
    // Sol has `command sol` but no `run sol`.
    assert_eq!(
        parse_err(&["run", "sol"]),
        "unknown command \"sol\" for \"rival run\""
    );
    let r = root();
    for (parent, name) in [
        (None, "review"),
        (Some("command"), "megareview"),
        (Some("run"), "sol"),
        (None, "server"),
    ] {
        let p = parent.map_or(&r, |p| node(&r, &[p]));
        assert!(p.find_subcommand(name).is_none(), "{name}");
    }
}

// ---- cobra lookup ----

#[test]
fn unknown_root_command_has_cobra_text_and_suggestions() {
    assert_eq!(
        parse_err(&["bogus"]),
        "unknown command \"bogus\" for \"rival\""
    );
    assert_eq!(
        parse_err(&["comand"]),
        "unknown command \"comand\" for \"rival\"\n\nDid you mean this?\n\tcommand\n"
    );
    // Prefix matches, in AddCommand order; help is never suggested.
    assert_eq!(
        parse_err(&["co"]),
        "unknown command \"co\" for \"rival\"\n\nDid you mean this?\n\tcommand\n\tconfig\n\tcompletion\n"
    );
    assert_eq!(
        parse_err(&["WAIT2"]),
        "unknown command \"WAIT2\" for \"rival\"\n\nDid you mean this?\n\twait\n"
    );
    // The unknown command wins over a bad flag after it.
    assert_eq!(
        parse_err(&["bogus", "--nope"]),
        "unknown command \"bogus\" for \"rival\""
    );
    assert_eq!(parse_err(&["--nope"]), "unknown flag: --nope");
}

#[test]
fn leaves_accept_extra_args_like_cobra() {
    let inv = parse_ok(&["command", "codex", "extra", "words"]);
    assert_eq!(inv.id, CommandId::CommandCodex);
    assert_eq!(inv.args, ["extra", "words"]);
    // `queue` has a subcommand but a parent: unknown words go to its action.
    let inv = parse_ok(&["queue", "nope"]);
    assert_eq!(
        (inv.id, inv.args.clone()),
        (CommandId::Queue, argv(&["nope"]))
    );
    assert_eq!(parse_ok(&["queue", "clear"]).id, CommandId::QueueClear);
    assert_eq!(parse_ok(&["wait", "a", "b"]).args, ["a", "b"]);
    assert_eq!(
        parse_err(&["install", "x"]),
        "unknown command \"x\" for \"rival install\""
    );
    assert_eq!(
        parse_err(&["completion", "bash", "x"]),
        "unknown command \"x\" for \"rival completion bash\""
    );
    // A stray word before a model name: cobra stops at the stray word.
    assert_eq!(
        parse_err(&["command", "bogus", "codex"]),
        "unknown command \"bogus\" for \"rival command\""
    );
}

#[test]
fn detach_is_inherited_and_found_anywhere_after_command() {
    for args in [
        &["command", "--detach", "codex"][..],
        &["command", "codex", "--detach"],
        &["command", "codex", "--detach=true", "--workdir", "x"],
    ] {
        let inv = parse_ok(args);
        assert_eq!(inv.id, CommandId::CommandCodex, "{args:?}");
        assert!(inv.bool("detach"), "{args:?}");
    }
    assert!(!parse_ok(&["command", "codex", "--detach=false"]).bool("detach"));
    assert!(!parse_ok(&["command", "codex"]).bool("detach"));
    assert!(!parse_ok(&["run", "claude"]).bool("detach"));
    assert_eq!(
        parse_err(&["run", "claude", "--detach"]),
        "unknown flag: --detach"
    );
}

// ---- help ----

#[test]
fn help_follows_cobra_lookup() {
    assert_eq!(help_path(&["-h"]), ["rival"]);
    assert_eq!(
        help_path(&["command", "codex", "--help"]),
        ["rival", "command", "codex"]
    );
    assert_eq!(
        help_path(&["command", "codex", "-h", "x"]),
        ["rival", "command", "codex"]
    );
    // cobra's lookup does not know -h, so the help is the command before it.
    assert_eq!(help_path(&["-h", "command"]), ["rival"]);
    assert_eq!(help_path(&["command", "-h", "codex"]), ["rival", "command"]);
    // completion is not runnable: help, even with stray words.
    assert_eq!(help_path(&["completion"]), ["rival", "completion"]);
    assert_eq!(help_path(&["completion", "nope"]), ["rival", "completion"]);
    // --help=false is a plain bool.
    assert_eq!(
        parse_ok(&["version", "--help=false"]).id,
        CommandId::Version
    );
    // A flag error wins over help.
    assert_eq!(
        parse_err(&["command", "codex", "-h", "--bogus"]),
        "unknown flag: --bogus"
    );
    let inv = parse_ok(&["help", "command", "codex"]);
    assert_eq!(
        (inv.id, inv.args.clone()),
        (CommandId::Help, argv(&["command", "codex"]))
    );
}

#[test]
fn help_target_follows_cobra_find() {
    let r = root();
    let t = |args: &[&str]| help_target(&r, &argv(args));
    assert_eq!(t(&[]), Some(argv(&["rival"])));
    assert_eq!(
        t(&["command", "codex"]),
        Some(argv(&["rival", "command", "codex"]))
    );
    // Below the root an unknown word is ignored; at the root it is unknown.
    assert_eq!(t(&["command", "nope"]), Some(argv(&["rival", "command"])));
    assert_eq!(t(&["nope"]), None);
}

#[test]
fn help_renders_for_every_command() {
    fn walk(r: &Command, cmd: &Command, path: &mut Vec<String>) {
        path.push(cmd.get_name().to_string());
        let text = render_help(&mut r.clone(), path);
        let about = cmd.get_about().unwrap().to_string();
        let long = cmd
            .get_long_about()
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            text.contains(&about) || text.contains(long.lines().next().unwrap_or("")),
            "{path:?}:\n{text}"
        );
        for (long, _, _) in flags(cmd) {
            assert!(
                text.contains(&format!("--{long}")),
                "{path:?} lacks --{long}:\n{text}"
            );
        }
        for sub in cmd.get_subcommands() {
            walk(r, sub, path);
        }
        path.pop();
    }
    let r = root();
    walk(&r, &r, &mut Vec::new());
    let text = render_help(&mut root(), &["rival", "command", "codex"]);
    assert!(text.contains("--detach"), "{text}");
    assert!(!text.contains("[args]"), "hidden positional shown:\n{text}");
}

// ---- pflag-compatible values ----

#[test]
fn string_flags_take_the_next_arg_even_with_a_dash() {
    let inv = parse_ok(&["command", "codex", "--workdir", "--no-queue"]);
    assert_eq!(inv.string("workdir"), "--no-queue");
    assert!(!inv.bool("no-queue"));
    let inv = parse_ok(&["command", "codex", "--workdir=a=b", "--workdir", "c"]);
    assert_eq!(inv.string("workdir"), "c", "last wins");
    let inv = parse_ok(&["command", "codex"]);
    assert_eq!(inv.string("workdir"), ".");
    assert!(!inv.changed("workdir"));
    let inv = parse_ok(&["command", "codex", "--workdir="]);
    assert_eq!(inv.string("workdir"), "");
    assert!(inv.changed("workdir"));
    // `--` ends flags.
    let inv = parse_ok(&["command", "codex", "--", "--no-queue"]);
    assert!(!inv.bool("no-queue"));
    assert_eq!(inv.args, ["--no-queue"]);
    // A lone dash is an arg.
    assert_eq!(parse_ok(&["command", "codex", "-"]).args, ["-"]);
}

#[test]
fn review_flag_changed_even_when_empty() {
    let inv = parse_ok(&["run", "claude", "--review", ""]);
    assert!(inv.changed("review"));
    assert_eq!(inv.string("review"), "");
    assert!(!parse_ok(&["run", "claude"]).changed("review"));
    let inv = parse_ok(&["run", "k3", "--review=src/"]);
    assert_eq!(inv.string("review"), "src/");
    assert_eq!(inv.string("effort"), "", "k3 has no --effort");
    assert_eq!(
        parse_err(&["run", "k3", "--effort", "high"]),
        "unknown flag: --effort"
    );
}

#[test]
fn bool_flags_parse_like_strconv() {
    for (v, want) in [
        ("true", true),
        ("1", true),
        ("T", true),
        ("False", false),
        ("0", false),
    ] {
        let inv = parse_ok(&["command", "codex", &format!("--no-queue={v}")]);
        assert_eq!(inv.bool("no-queue"), want, "{v}");
    }
    assert_eq!(
        parse_err(&["command", "codex", "--no-queue=yes"]),
        "invalid argument \"yes\" for \"--no-queue\" flag: strconv.ParseBool: parsing \"yes\": invalid syntax"
    );
    assert_eq!(
        parse_err(&["command", "codex", "--no-queue="]),
        "invalid argument \"\" for \"--no-queue\" flag: strconv.ParseBool: parsing \"\": invalid syntax"
    );
    assert_eq!(
        parse_err(&["version", "--help=x"]),
        "invalid argument \"x\" for \"-h, --help\" flag: strconv.ParseBool: parsing \"x\": invalid syntax"
    );
    // A bool never eats the next word.
    let inv = parse_ok(&["command", "codex", "--no-queue", "false"]);
    assert!(inv.bool("no-queue"));
    assert_eq!(inv.args, ["false"]);
    let inv = parse_ok(&["command", "codex", "--no-queue", "--no-queue=false"]);
    assert!(!inv.bool("no-queue") && inv.changed("no-queue"));
}

#[test]
fn flag_errors_have_pflag_text() {
    assert_eq!(
        parse_err(&["command", "codex", "--bogus"]),
        "unknown flag: --bogus"
    );
    assert_eq!(
        parse_err(&["command", "codex", "--bogus=1"]),
        "unknown flag: --bogus"
    );
    assert_eq!(
        parse_err(&["command", "codex", "-x"]),
        "unknown shorthand flag: 'x' in -x"
    );
    assert_eq!(
        parse_err(&["command", "codex", "-hx"]),
        "unknown shorthand flag: 'x' in -x"
    );
    assert_eq!(
        parse_err(&["command", "codex", "-xh"]),
        "unknown shorthand flag: 'x' in -xh"
    );
    assert_eq!(
        parse_err(&["command", "codex", "---x"]),
        "bad flag syntax: ---x"
    );
    assert_eq!(
        parse_err(&["command", "codex", "--=x"]),
        "bad flag syntax: --=x"
    );
    assert_eq!(
        parse_err(&["command", "codex", "--workdir"]),
        "flag needs an argument: --workdir"
    );
    assert_eq!(
        parse_err(&["command", "plan", "-m"]),
        "flag needs an argument: 'm' in -m"
    );
    assert_eq!(
        parse_err(&["command", "plan", "-hm"]),
        "flag needs an argument: 'm' in -m"
    );
    assert_eq!(
        parse_err(&["command", "codex", "-\u{1}"]),
        "unknown shorthand flag: '\\x01' in -\u{1}"
    );
    // The first error wins.
    assert_eq!(
        parse_err(&["command", "codex", "--bogus", "--workdir"]),
        "unknown flag: --bogus"
    );
}

#[test]
fn duration_flags_parse_signed_values_and_defaults() {
    let inv = parse_ok(&["wait", "--timeout", "1s", "--poll=100ms"]);
    assert_eq!(inv.duration("timeout"), 1_000_000_000);
    assert_eq!(inv.duration("poll"), 100_000_000);
    assert_eq!(
        parse_ok(&["wait", "--timeout=-1s"]).duration("timeout"),
        -1_000_000_000
    );
    assert_eq!(
        parse_ok(&["wait", "--timeout", "-1s"]).duration("timeout"),
        -1_000_000_000
    );
    let inv = parse_ok(&["wait"]);
    assert_eq!(inv.duration("timeout"), DEFAULTS.wait_timeout);
    assert_eq!(inv.duration("poll"), 2_000_000_000);
    assert_eq!(
        parse_err(&["wait", "--timeout", "abc"]),
        "invalid argument \"abc\" for \"--timeout\" flag: time: invalid duration \"abc\""
    );
    assert_eq!(
        parse_err(&["wait", "--poll"]),
        "flag needs an argument: --poll"
    );
}

#[test]
fn model_values_accumulate_like_pflag_string_slices() {
    let inv = parse_ok(&["command", "plan"]);
    assert_eq!(inv.strings("model"), ["codex"]);
    assert!(!inv.changed("model"));
    let inv = parse_ok(&["command", "plan", "-m", "claude"]);
    assert_eq!(
        inv.strings("model"),
        ["claude"],
        "the first value replaces the default"
    );
    let inv = parse_ok(&[
        "command",
        "plan",
        "-mclaude",
        "--model=codex,claude",
        "-m=x",
    ]);
    assert_eq!(inv.strings("model"), ["claude", "codex", "claude", "x"]);
    assert!(inv.changed("model"));
    assert_eq!(
        parse_ok(&["command", "plan", "--model", ""]).strings("model"),
        Vec::<String>::new()
    );
    assert_eq!(
        parse_err(&["command", "plan", "-m", "a\"b"]),
        "invalid argument \"a\\\"b\" for \"-m, --model\" flag: parse error on line 1, column 2: bare \" in non-quoted-field"
    );
}

#[test]
fn ints_parse_like_strconv_base_zero() {
    for (v, want) in [
        ("5", 5),
        ("+4", 4),
        ("-3", -3),
        ("0x1f", 31),
        ("0o17", 15),
        ("017", 15),
        ("0b101", 5),
        ("1_000", 1000),
        ("0", 0),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        assert_eq!(parse_int(v), Ok(want), "{v}");
    }
    for v in ["", "x", "08", "0x", "1__0", "_1", "1_", "--1", "1.5"] {
        assert_eq!(
            parse_int(v),
            Err(format!("strconv.ParseInt: parsing {:?}: invalid syntax", v)),
            "{v}"
        );
    }
    for v in [
        "9223372036854775808",
        "-9223372036854775809",
        "99999999999999999999999",
    ] {
        assert_eq!(
            parse_int(v),
            Err(format!(
                "strconv.ParseInt: parsing {:?}: value out of range",
                v
            )),
            "{v}"
        );
    }
    assert_eq!(
        parse_ok(&["sessions", "--recent", "0x10"]).int("recent"),
        16
    );
    assert_eq!(
        parse_err(&["sessions", "--recent", "ten"]),
        "invalid argument \"ten\" for \"--recent\" flag: strconv.ParseInt: parsing \"ten\": invalid syntax"
    );
}

#[test]
fn help_topic_quote_is_debug_quoted() {
    assert_eq!(quote_topic(&argv(&["foo"])), "[`foo`]");
    assert_eq!(quote_topic(&argv(&["a", "b c"])), "[`a` `b c`]");
    assert_eq!(quote_topic(&argv(&["a`b"])), "[\"a`b\"]");
    assert_eq!(quote_topic(&[]), "[]");
}

#[test]
fn completion_scripts_cover_the_tree() {
    for shell in [
        clap_complete::Shell::Bash,
        clap_complete::Shell::Zsh,
        clap_complete::Shell::Fish,
        clap_complete::Shell::PowerShell,
    ] {
        let mut cmd = root();
        let mut out = Vec::new();
        clap_complete::generate(shell, &mut cmd, "rival", &mut out);
        let script = String::from_utf8(out).unwrap();
        for word in [
            "command",
            "codex",
            "workdir",
            "detach",
            "wait",
            "timeout",
            "completion",
            "config",
            "models",
        ] {
            assert!(script.contains(word), "{shell}: script lacks {word}");
        }
    }
}

/// `opus` is another name for `claude` under `command` and `run`; the
/// invocation keeps the canonical path.
#[test]
fn opus_is_an_alias_of_claude() {
    let inv = parse_ok(&["command", "opus"]);
    assert_eq!(inv.id, CommandId::CommandClaude);
    assert_eq!(inv.path, ["rival", "command", "claude"]);
    let inv = parse_ok(&["run", "opus", "--prompt-stdin"]);
    assert_eq!(inv.id, CommandId::RunClaude);
    assert_eq!(parse_ok(&["command", "fable"]).id, CommandId::CommandFable);
    assert_eq!(parse_ok(&["command", "sol"]).id, CommandId::CommandSol);
    assert_eq!(parse_ok(&["run", "fable"]).id, CommandId::RunFable);
    assert_eq!(
        parse_err(&["run", "opuss"]).lines().next().unwrap(),
        "unknown command \"opuss\" for \"rival run\""
    );
}
