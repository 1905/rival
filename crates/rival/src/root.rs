//! The root command: startup order, the pre-run hook, dispatch and exit
//! codes. Go: `main.go`, `cmd/root.go`.

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use rival_core::cancel::{CancelFunc, Context};
use rival_core::config::Config;
use rival_core::mergerequest::{self, Snapshot};
use rival_core::paths::{self, Paths};
use rival_core::{logging, queue, session};

use crate::detach::{self, DetachOutcome};
use crate::model_command::run_model_command;
use crate::model_run::{RunOptions, run_model_run};
use crate::model_specs::{claude_spec, codex_spec, grok_spec, k3_spec};
use crate::signals::{self, NotifyGuard};
use crate::tree::{self, CommandId, Defaults, Invocation, Parsed};
use crate::{startup_fds, wait};

#[cfg(test)]
mod tests;

const BANNER: &str = "
         _             __
   _____(_)   ______ _/ /
  / ___/ / | / / __ `/ /
 / /  / /| |/ / /_/ / /
/_/  /_/ |___/\\__,_/_/
";

/// How long the root waits for the background update check after the
/// command finished. It matches the check's own HTTP timeout.
pub const UPDATE_CHECK_WAIT: Duration = Duration::from_secs(2);

/// A command failure. Go: a plain `error` (exit 1) or `*ExitCodeError`.
/// The root prints `message` on stderr and exits with `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    pub code: i32,
    pub message: String,
}

impl CmdError {
    /// A plain Go `error`: exit code 1.
    pub fn plain(message: impl Into<String>) -> Self {
        CmdError {
            code: 1,
            message: message.into(),
        }
    }

    /// Go `&ExitCodeError{Code: code, Err: err}`.
    pub fn exit(code: i32, message: impl Into<String>) -> Self {
        CmdError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CmdError {}

/// Go `os.Stdin` as the commands use it.
pub trait StdinSource {
    /// Go `os.Stdin.Stat()` succeeded and reports a character device (a
    /// terminal, or `/dev/null`).
    fn is_char_device(&self) -> bool;
    /// Go `io.ReadAll(os.Stdin)`. The error is Go's `*PathError` text.
    fn read_all(&mut self) -> Result<Vec<u8>, String>;
}

/// The real fd 0.
pub struct ProcessStdin;

impl StdinSource for ProcessStdin {
    fn is_char_device(&self) -> bool {
        // A descriptor closed at startup fails Go's Stat; Rust reopened it
        // on /dev/null, which would read as a character device.
        if startup_fds::closed_at_start(0) {
            return false;
        }
        stdin_is_char_device()
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        let text =
            |e: &io::Error| format!("read /dev/stdin: {}", rival_core::gostd::os_error_text(e));
        if startup_fds::closed_at_start(0) {
            return Err(text(&io::Error::from_raw_os_error(libc::EBADF)));
        }
        let mut data = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut data)
            .map_err(|e| text(&e))?;
        Ok(data)
    }
}

#[cfg(unix)]
fn stdin_is_char_device() -> bool {
    // SAFETY: fstat writes into a zeroed stat buffer we own.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut st) } != 0 {
        return false;
    }
    st.st_mode & libc::S_IFMT == libc::S_IFCHR
}

#[cfg(not(unix))]
fn stdin_is_char_device() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

/// Unbuffered stdout, like Go's `os.Stdout`: every write reaches fd 1 at
/// once, so it interleaves with stderr in call order. When fd 1 was closed
/// at startup, writes fail with EBADF as Go's do (Rust reopened it on
/// /dev/null, where they would succeed).
pub struct ProcessStdout;

impl Write for ProcessStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if startup_fds::closed_at_start(1) {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        let mut out = io::stdout().lock();
        let n = out.write(buf)?;
        out.flush()?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

/// Go `mergerequest.Prepare` or a test fake (Go's `prepareMR` package var).
pub type PrepareMr = dyn Fn(&Context, &Config, &str, &str) -> Result<Option<Snapshot>, String>;

/// What a command action reads and writes. Go reaches these through
/// globals; here they are injected so tests never touch the process.
pub struct CmdEnv<'a> {
    pub cfg: &'a Config,
    pub stdin: &'a mut dyn StdinSource,
    pub stdout: &'a mut (dyn Write + Send),
    pub stderr: &'a mut (dyn Write + Send),
    pub prepare_mr: &'a PrepareMr,
    /// Whether SIGINT/SIGTERM cancel a run (production). Tests use a plain
    /// cancellable context instead of process-wide handlers.
    pub signals: bool,
}

/// Go `signal.NotifyContext(...)` plus its deferred `stop()`.
pub enum SignalScope {
    Notify(#[allow(dead_code, reason = "held for its Drop")] NotifyGuard),
    Plain(CancelFunc),
}

impl Drop for SignalScope {
    fn drop(&mut self) {
        if let SignalScope::Plain(cancel) = self {
            cancel.cancel();
        }
    }
}

impl CmdEnv<'_> {
    /// The run's root context: cancelled by SIGINT/SIGTERM in production,
    /// and when the returned scope drops. Fails only when the signal
    /// handlers cannot be installed.
    pub fn signal_context(&self) -> Result<(Context, SignalScope), CmdError> {
        if self.signals {
            let (ctx, guard) = signals::notify_context(&Context::background())
                .map_err(|e| CmdError::plain(format!("install signal handlers: {e}")))?;
            Ok((ctx, SignalScope::Notify(guard)))
        } else {
            let (ctx, cancel) = Context::background().with_cancel();
            Ok((ctx, SignalScope::Plain(cancel)))
        }
    }
}

pub type ConfigHook = Arc<dyn Fn(&Config) + Send + Sync>;

/// The root's side effects, injectable for tests.
pub struct RootHooks {
    /// Go `reap()`: fail orphaned sessions, then drop dead queue tickets.
    pub reap: ConfigHook,
    /// Go `update.Check(Version)`.
    pub update_check: ConfigHook,
    /// Go `detachIfRequested(true)`.
    pub detach: Box<dyn Fn() -> DetachOutcome>,
}

impl RootHooks {
    pub fn production() -> Self {
        RootHooks {
            reap: Arc::new(reap),
            update_check: Arc::new(pending_update_check),
            detach: Box::new(|| detach::detach_if_requested(true)),
        }
    }
}

/// Go `reap`. Sessions first: queue ticket liveness reads session state.
pub fn reap(cfg: &Config) {
    session::reaper::reap_orphans(cfg.paths());
    queue::Manager::new(cfg.paths(), cfg).reap_dead();
}

/// Extension point for Task 3.4 (`internal/update` port). Until then no
/// release check runs; the root still starts and bounds it like Go.
fn pending_update_check(_cfg: &Config) {}

/// Go `main` + `cmd.Execute`. Returns the process exit code.
pub fn main_entry() -> i32 {
    // Go package init runs before main: config.yaml is read with the
    // pre-.env HOME, and `wait --timeout` gets its default from the
    // pre-.env RIVAL_QUEUE_TIMEOUT / RIVAL_RUN_TIMEOUT.
    let initial = Config::load(&Paths::from_env());
    let defaults = Defaults {
        wait_timeout: initial.max_run_wait(),
    };
    // SAFETY: no other thread exists yet and nothing reads the environment
    // concurrently.
    unsafe { paths::load_dotenv() };
    logging::init();
    // Runtime getenv calls see .env additions; the user config stays the
    // one loaded above.
    let cfg = initial.reload_env(Paths::from_env());

    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut stdin = ProcessStdin;
    let mut stdout = ProcessStdout;
    let mut stderr = io::stderr();
    let prepare: &PrepareMr = &mergerequest::prepare;
    let mut env = CmdEnv {
        cfg: &cfg,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        prepare_mr: prepare,
        signals: true,
    };
    execute(&mut env, &RootHooks::production(), &defaults, &args)
}

/// Background work the root joins before it exits.
#[derive(Default)]
struct Background {
    reap: Option<JoinHandle<()>>,
    update: Option<mpsc::Receiver<()>>,
}

impl Background {
    /// Go `waitForReap`: a background reap always finishes, so the exit
    /// never cuts a session save short.
    fn wait_for_reap(&mut self) {
        if let Some(handle) = self.reap.take() {
            let _ = handle.join();
        }
    }

    /// Go `waitForUpdateCheck`: at most [`UPDATE_CHECK_WAIT`].
    fn wait_for_update_check(&mut self, limit: Duration) {
        if let Some(done) = self.update.take() {
            let _ = done.recv_timeout(limit);
        }
    }
}

/// What the pre-run hook decided.
enum PreRun {
    Continue,
    /// Exit now with this code (Go `os.Exit` inside the hook).
    Exit(i32),
    Fail(CmdError),
}

/// Go `cmd.Execute` over `args` (no program name). Returns the exit code.
pub fn execute(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    defaults: &Defaults,
    args: &[String],
) -> i32 {
    execute_with_wait(env, hooks, defaults, args, UPDATE_CHECK_WAIT)
}

pub(crate) fn execute_with_wait(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    defaults: &Defaults,
    args: &[String],
    update_wait: Duration,
) -> i32 {
    let mut root = tree::build(defaults);
    let mut bg = Background::default();
    let result = match tree::parse(&mut root, args) {
        Err(message) => Err(CmdError::plain(message)),
        Ok(Parsed::Help(path)) => {
            let _ = env
                .stdout
                .write_all(tree::render_help(&mut root, &path).as_bytes());
            Ok(())
        }
        Ok(Parsed::Run(inv)) => match pre_run(env, hooks, &inv, &mut bg) {
            PreRun::Exit(code) => return code,
            PreRun::Fail(err) => Err(err),
            PreRun::Continue => dispatch(env, &mut root, &inv),
        },
    };
    // Both exit paths below end the process, so the joins happen first.
    bg.wait_for_reap();
    bg.wait_for_update_check(update_wait);
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(env.stderr, "{}", err.message);
            err.code
        }
    }
}

/// Go `PersistentPreRunE`, in its order: config error, detach, reap, update
/// check.
fn pre_run(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    inv: &Invocation,
    bg: &mut Background,
) -> PreRun {
    if let Some(err) = env.cfg.user_config_error() {
        return PreRun::Fail(CmdError::plain(err.to_string()));
    }
    // Detach before any side effects: the re-exec'd child redoes this hook.
    // Only the `command` subtree has the flag; elsewhere it reads false.
    if inv.bool("detach")
        && let DetachOutcome::Exit(code) = (hooks.detach)()
    {
        return PreRun::Exit(code);
    }
    if inv.id == CommandId::Tui {
        // The TUI owns the terminal: a log line on stderr (the background
        // reaper logs each orphan it fails) would draw over the screen.
        logging::set_enabled(false);
        let cfg = env.cfg.clone();
        let reap = Arc::clone(&hooks.reap);
        bg.reap = Some(std::thread::spawn(move || reap(&cfg)));
    } else {
        (hooks.reap)(env.cfg);
    }
    let (done, wait) = mpsc::channel();
    let cfg = env.cfg.clone();
    let check = Arc::clone(&hooks.update_check);
    std::thread::spawn(move || {
        check(&cfg);
        let _ = done.send(());
    });
    bg.update = Some(wait);
    PreRun::Continue
}

/// Which later task ports a command that is not wired yet.
fn pending_task(id: CommandId) -> Option<&'static str> {
    match id {
        CommandId::CommandPlan | CommandId::CommandAntislop | CommandId::CommandSecurity => {
            Some("Task 3.2")
        }
        CommandId::Install => Some("Task 3.3"),
        CommandId::Queue | CommandId::QueueClear | CommandId::Sessions | CommandId::Update => {
            Some("Task 3.4")
        }
        CommandId::Tui => Some("Task 4.5"),
        _ => None,
    }
}

fn dispatch(
    env: &mut CmdEnv<'_>,
    root: &mut clap::Command,
    inv: &Invocation,
) -> Result<(), CmdError> {
    if let Some(task) = pending_task(inv.id) {
        return Err(CmdError::plain(format!(
            "{}: not available in this build yet ({task} of the Rust port)",
            inv.command_path()
        )));
    }
    match inv.id {
        CommandId::Root => {
            let _ = write!(
                env.stdout,
                "{BANNER}  {} — multi-model AI reviews from your terminal\n\n{}",
                rival_core::VERSION,
                tree::render_usage(root)
            );
            Ok(())
        }
        CommandId::Version => {
            let _ = writeln!(env.stdout, "{BANNER}  {}", rival_core::VERSION);
            Ok(())
        }
        CommandId::Command | CommandId::Run => {
            let _ = env
                .stdout
                .write_all(tree::render_help(root, &inv.path).as_bytes());
            Ok(())
        }
        CommandId::CommandCodex => model_command(env, inv, codex_spec),
        CommandId::CommandClaude => model_command(env, inv, claude_spec),
        CommandId::CommandGrok => model_command(env, inv, grok_spec),
        CommandId::CommandK3 => model_command(env, inv, k3_spec),
        CommandId::RunClaude => model_run(env, inv, claude_spec),
        CommandId::RunGrok => model_run(env, inv, grok_spec),
        CommandId::RunK3 => model_run(env, inv, k3_spec),
        CommandId::Wait => {
            let opts = wait::WaitOptions {
                log: inv.string("log").into(),
                timeout: inv.duration("timeout"),
                poll: inv.duration("poll"),
            };
            let (ctx, _scope) = env.signal_context()?;
            wait::wait_action(&opts, &inv.args, env.cfg.paths(), &ctx, env.stdout)
                .map_err(|e| CmdError::exit(e.code, e.message))
        }
        CommandId::Help => {
            help_topic(env, root, &inv.args);
            Ok(())
        }
        CommandId::CompletionBash => completion(env, root, inv, clap_complete::Shell::Bash),
        CommandId::CompletionZsh => completion(env, root, inv, clap_complete::Shell::Zsh),
        CommandId::CompletionFish => completion(env, root, inv, clap_complete::Shell::Fish),
        CommandId::CompletionPowershell => {
            completion(env, root, inv, clap_complete::Shell::PowerShell)
        }
        _ => Err(CmdError::plain(format!(
            "{}: no action wired",
            inv.command_path()
        ))),
    }
}

fn model_command(
    env: &mut CmdEnv<'_>,
    inv: &Invocation,
    spec: fn() -> crate::model_specs::ModelSpec,
) -> Result<(), CmdError> {
    run_model_command(env, &spec(), &inv.string("workdir"), inv.bool("no-queue"))
}

fn model_run(
    env: &mut CmdEnv<'_>,
    inv: &Invocation,
    spec: fn() -> crate::model_specs::ModelSpec,
) -> Result<(), CmdError> {
    run_model_run(
        env,
        &spec(),
        RunOptions {
            workdir: inv.string("workdir"),
            no_queue: inv.bool("no-queue"),
            // K3 has no --effort flag: it reads as "".
            effort: inv.string("effort"),
            review_scope: inv.string("review"),
            is_review: inv.changed("review"),
            prompt_stdin: inv.bool("prompt-stdin"),
        },
    )
}

/// cobra's `help [command]`: the command's help on stdout, or an unknown
/// topic line plus the root usage on stderr.
fn help_topic(env: &mut CmdEnv<'_>, root: &mut clap::Command, args: &[String]) {
    match tree::help_target(root, args) {
        Some(path) => {
            let _ = env
                .stdout
                .write_all(tree::render_help(root, &path).as_bytes());
        }
        None => {
            let _ = writeln!(env.stderr, "Unknown help topic {}", tree::quote_topic(args));
            let _ = env.stderr.write_all(tree::render_usage(root).as_bytes());
        }
    }
}

/// cobra's `completion <shell> [--no-descriptions]`. The script is clap's,
/// not cobra's; both complete the same commands and flags.
fn completion(
    env: &mut CmdEnv<'_>,
    root: &clap::Command,
    inv: &Invocation,
    shell: clap_complete::Shell,
) -> Result<(), CmdError> {
    let mut cmd = if inv.bool("no-descriptions") {
        tree::without_descriptions(root.clone())
    } else {
        root.clone()
    };
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, "rival", &mut buf);
    let _ = env.stdout.write_all(&buf);
    Ok(())
}
