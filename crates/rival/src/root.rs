//! The root command: startup order, the pre-run hook, dispatch and exit
//! codes. Go: `main.go`, `cmd/root.go`.

use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use rival_core::cancel::{CancelFunc, Context};
use rival_core::config::Config;
use rival_core::mergerequest::{self, Snapshot};
use rival_core::paths::{self, Paths};
use rival_core::telemetry::Telemetry;
use rival_core::{logging, queue, session, update};

use crate::detach::{self, DetachOutcome};
use crate::model_command::run_model_command;
use crate::model_run::{RunOptions, run_model_run};
use crate::model_specs::{claude_spec, codex_spec, grok_spec, k3_spec};
use crate::signals::{self, NotifyGuard};
use crate::tree::{self, CommandId, Defaults, Invocation, Parsed};
use crate::{command_plan, command_security, install, queue_sessions, update_cmd};
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
    /// Go `os.Stdin.Stat()` failed (fd 0 closed or invalid). Only
    /// `command security` tells this apart from a non-terminal stdin: it
    /// skips the read instead of failing it.
    fn stat_failed(&self) -> bool {
        false
    }
    /// Go `io.ReadAll(os.Stdin)`. The error is Go's `*PathError` text.
    fn read_all(&mut self) -> Result<Vec<u8>, String>;
    /// Go `bufio.NewReader(cmd.InOrStdin())` for line prompts. Callers keep
    /// one reader for the whole command, so buffered answers are not lost.
    fn reader(&mut self) -> Box<dyn BufRead + '_>;
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

    fn stat_failed(&self) -> bool {
        startup_fds::closed_at_start(0) || !stdin_stat_ok()
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        let text =
            |e: &io::Error| format!("read /dev/stdin: {}", rival_core::gostd::os_error_text(e));
        if startup_fds::closed_at_start(0) {
            return Err(text(&io::Error::from_raw_os_error(libc::EBADF)));
        }
        // Windows: std reads EOF from a missing handle (`handle_ebadf`); Go
        // does not. An INVALID_HANDLE_VALUE makes Go's `os.Stdin` nil, whose
        // Read is the bare `os.ErrInvalid`. A NULL handle is still a File,
        // and its ReadFile fails with ERROR_INVALID_HANDLE.
        #[cfg(windows)]
        match std_handle(windows_sys::Win32::System::Console::STD_INPUT_HANDLE) {
            StdHandle::Invalid => return Err(NilFile.to_string()),
            StdHandle::Null => return Err(text(&invalid_handle_error())),
            StdHandle::Valid(_) => {}
        }
        let mut data = Vec::new();
        io::stdin()
            .lock()
            .read_to_end(&mut data)
            .map_err(|e| text(&e))?;
        Ok(data)
    }

    /// A fd 0 closed at startup reads EOF here (Rust reopened it on
    /// /dev/null); Go's read fails. Both give a prompt an empty answer.
    fn reader(&mut self) -> Box<dyn BufRead + '_> {
        Box::new(io::stdin().lock())
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

#[cfg(unix)]
fn stdin_stat_ok() -> bool {
    // SAFETY: fstat writes into a zeroed stat buffer we own.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::fstat(0, &mut st) == 0 }
}

/// Go `os.Stdin.Stat()` on Windows reports `ModeCharDevice` for a
/// `FILE_TYPE_CHAR` handle: a console, and also the `NUL` device.
#[cfg(windows)]
fn stdin_is_char_device() -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_TYPE_CHAR;
    stdin_file_type() == Some(FILE_TYPE_CHAR)
}

/// Go `os.Stdin.Stat()` succeeding on Windows: a usable standard input
/// handle whose file type can be read. An INVALID_HANDLE_VALUE (Go's
/// `os.Stdin` is nil: `ErrInvalid`) or a NULL handle (`GetFileType` fails)
/// fails, as does an unknown type with an error.
#[cfg(windows)]
fn stdin_stat_ok() -> bool {
    stdin_file_type().is_some()
}

/// A Windows standard handle as Go's `syscall.getStdHandle` keeps it. Go's
/// `os.NewFile` rejects only INVALID_HANDLE_VALUE (a nil `*File`); a NULL
/// handle becomes a `File` whose calls fail with ERROR_INVALID_HANDLE.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StdHandle {
    Valid(windows_sys::Win32::Foundation::HANDLE),
    Null,
    Invalid,
}

#[cfg(windows)]
pub(crate) fn std_handle(which: windows_sys::Win32::System::Console::STD_HANDLE) -> StdHandle {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::GetStdHandle;
    // SAFETY: a plain query.
    let handle = unsafe { GetStdHandle(which) };
    if handle == INVALID_HANDLE_VALUE {
        StdHandle::Invalid
    } else if handle.is_null() {
        StdHandle::Null
    } else {
        StdHandle::Valid(handle)
    }
}

/// ERROR_INVALID_HANDLE: what Go's ReadFile/WriteFile/GetFileType on a NULL
/// standard handle fail with.
#[cfg(windows)]
fn invalid_handle_error() -> io::Error {
    io::Error::from_raw_os_error(windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE as i32)
}

/// `GetFileType` of the standard input handle, or `None` when the handle is
/// missing or the call fails.
#[cfg(windows)]
pub(crate) fn stdin_file_type() -> Option<u32> {
    use windows_sys::Win32::Foundation::{GetLastError, NO_ERROR};
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_UNKNOWN, GetFileType};
    use windows_sys::Win32::System::Console::STD_INPUT_HANDLE;
    let StdHandle::Valid(handle) = std_handle(STD_INPUT_HANDLE) else {
        return None;
    };
    // SAFETY: plain queries on a standard input handle.
    unsafe {
        let kind = GetFileType(handle);
        if kind == FILE_TYPE_UNKNOWN && GetLastError() != NO_ERROR {
            return None;
        }
        Some(kind)
    }
}

/// Go's `os.ErrInvalid` from a nil `*os.File` (`checkValid`): its text has
/// no op or path. Windows `os.Stdin`/`os.Stdout` are nil when the standard
/// handle is INVALID_HANDLE_VALUE.
#[derive(Debug)]
pub struct NilFile;

impl std::fmt::Display for NilFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid argument")
    }
}

impl std::error::Error for NilFile {}

/// Whether `e` is [`NilFile`]: Go prints it bare, without `write <path>:`.
pub fn is_nil_file(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<NilFile>())
}

/// Unbuffered stdout, like Go's `os.Stdout`: every write reaches fd 1 at
/// once, so it interleaves with stderr in call order. When fd 1 was closed
/// at startup, writes fail with EBADF as Go's do (Rust reopened it on
/// /dev/null, where they would succeed).
///
/// Windows: std turns a write to a missing handle into a silent success
/// (`handle_ebadf`). Go fails it: INVALID_HANDLE_VALUE with [`NilFile`], a
/// NULL handle with ERROR_INVALID_HANDLE. The handle is checked before each
/// write. A valid handle that fails later still goes through std.
pub struct ProcessStdout;

impl Write for ProcessStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if startup_fds::closed_at_start(1) {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        #[cfg(windows)]
        match std_handle(windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE) {
            StdHandle::Invalid => return Err(io::Error::new(io::ErrorKind::InvalidInput, NilFile)),
            StdHandle::Null => return Err(invalid_handle_error()),
            StdHandle::Valid(_) => {}
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
    /// Opens an owned handle to the real stdout for `rival run`'s live
    /// mirror (see [`crate::mirror`]). `None` mirrors to `stdout` directly,
    /// as tests do.
    pub live_stdout: Option<&'a LiveStdout>,
    pub prepare_mr: &'a PrepareMr,
    /// Whether SIGINT/SIGTERM cancel a run (production). Tests use a plain
    /// cancellable context instead of process-wide handlers.
    pub signals: bool,
}

/// Makes an owned stdout writer for a thread that can outlive a borrow.
pub type LiveStdout = dyn Fn() -> Box<dyn Write + Send> + Sync;

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

/// Go `update.Check(Version)`, printing its notice to the writer.
pub type UpdateHook = Arc<dyn Fn(&Config, &mut dyn Write) + Send + Sync>;

/// Runs the dashboard. The error is the text after "tui: ".
pub type TuiHook = Box<dyn Fn(&Config) -> Result<(), String>>;

/// The root's side effects, injectable for tests.
pub struct RootHooks {
    /// Go `reap()`: fail orphaned sessions, then drop dead queue tickets.
    pub reap: ConfigHook,
    /// Go `update.Check(Version)`.
    pub update_check: UpdateHook,
    /// Go `detachIfRequested(true)`.
    pub detach: Box<dyn Fn() -> DetachOutcome>,
    /// Go `tea.NewProgram(dashboard.New()).Run()`.
    pub tui: TuiHook,
}

impl RootHooks {
    pub fn production() -> Self {
        RootHooks {
            reap: Arc::new(reap),
            update_check: Arc::new(|cfg, out| {
                update::check_production(rival_core::VERSION, cfg, out);
            }),
            detach: Box::new(|| detach::detach_if_requested(true)),
            tui: Box::new(crate::tui::runtime::run),
        }
    }
}

/// Go `reap`. Sessions first: queue ticket liveness reads session state.
pub fn reap(cfg: &Config) {
    session::reaper::reap_orphans(cfg.paths());
    queue::Manager::new(cfg.paths(), cfg).reap_dead();
}

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
    // Go: telemetry.Init after the logger, then a deferred Flush that only
    // a normal return from Execute reaches (every error path calls os.Exit).
    let telemetry = Telemetry::init(rival_core::VERSION, |key| cfg.getenv(key));

    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut stdin = ProcessStdin;
    let mut stdout = ProcessStdout;
    let mut stderr = io::stderr();
    let prepare: &PrepareMr = &mergerequest::prepare;
    let live: &LiveStdout = &|| Box::new(ProcessStdout);
    let mut env = CmdEnv {
        cfg: &cfg,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        live_stdout: Some(live),
        prepare_mr: prepare,
        signals: true,
    };
    let exit = execute_inner(
        &mut env,
        &RootHooks::production(),
        &defaults,
        &args,
        UPDATE_CHECK_WAIT,
    );
    // Go `defer telemetry.RecoverPanic()` in Execute: a no-op (see
    // Telemetry::recover_panic).
    telemetry.recover_panic();
    if exit.returned {
        telemetry.flush();
    }
    exit.code
}

/// How Go's `Execute` ended.
pub(crate) struct Exit {
    pub code: i32,
    /// `Execute` returned normally (no `os.Exit`), so `main`'s deferred
    /// telemetry flush runs.
    pub returned: bool,
}

/// Background work the root joins before it exits.
#[derive(Default)]
struct Background {
    reap: Option<JoinHandle<()>>,
    /// The check's end. It carries the notice the check buffered instead of
    /// printing (the TUI's); other commands print at once and send nothing.
    update: Option<mpsc::Receiver<Vec<u8>>>,
}

impl Background {
    /// Go `waitForReap`: a background reap always finishes, so the exit
    /// never cuts a session save short.
    fn wait_for_reap(&mut self) {
        if let Some(handle) = self.reap.take() {
            let _ = handle.join();
        }
    }

    /// Go `waitForUpdateCheck`: at most [`UPDATE_CHECK_WAIT`]. Returns the
    /// buffered notice, if the check finished and buffered one.
    fn wait_for_update_check(&mut self, limit: Duration) -> Vec<u8> {
        self.update
            .take()
            .and_then(|done| done.recv_timeout(limit).ok())
            .unwrap_or_default()
    }
}

/// What the pre-run hook decided.
enum PreRun {
    Continue,
    /// Exit now with this code (Go `os.Exit` inside the hook).
    Exit(i32),
    Fail(CmdError),
}

/// [`execute_inner`]'s exit code, for tests.
#[cfg(test)]
pub(crate) fn execute_with_wait(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    defaults: &Defaults,
    args: &[String],
    update_wait: Duration,
) -> i32 {
    execute_inner(env, hooks, defaults, args, update_wait).code
}

/// Go `cmd.Execute` over `args` (no program name).
fn execute_inner(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    defaults: &Defaults,
    args: &[String],
    update_wait: Duration,
) -> Exit {
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
            PreRun::Exit(code) => {
                return Exit {
                    code,
                    returned: false,
                };
            }
            PreRun::Fail(err) => Err(err),
            PreRun::Continue => dispatch(env, hooks, &mut root, &inv),
        },
    };
    // Both exit paths below end the process, so the joins happen first.
    bg.wait_for_reap();
    // The TUI's notice was held back while it owned the screen; the
    // terminal is restored by now.
    let notice = bg.wait_for_update_check(update_wait);
    let _ = env.stderr.write_all(&notice);
    match result {
        Ok(()) => Exit {
            code: 0,
            returned: true,
        },
        Err(err) => {
            let _ = writeln!(env.stderr, "{}", err.message);
            Exit {
                code: err.code,
                returned: false,
            }
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
    let tui = inv.id == CommandId::Tui;
    if tui {
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
        // Go prints the notice on stderr whenever the check ends. Under the
        // TUI that would draw over the screen, so the notice is buffered
        // and the root prints it after the terminal is restored.
        let mut held = Vec::new();
        if tui {
            check(&cfg, &mut held);
        } else {
            check(&cfg, &mut io::stderr());
        }
        let _ = done.send(held);
    });
    bg.update = Some(wait);
    PreRun::Continue
}

fn dispatch(
    env: &mut CmdEnv<'_>,
    hooks: &RootHooks,
    root: &mut clap::Command,
    inv: &Invocation,
) -> Result<(), CmdError> {
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
        CommandId::CommandPlan => command_plan::command_plan_action(env, inv),
        CommandId::CommandSecurity => command_security::command_security_action(env, inv),
        CommandId::Install => install::install_action(env, inv),
        CommandId::Queue => queue_sessions::queue_list_action(env),
        CommandId::QueueClear => queue_sessions::queue_clear_action(env, inv),
        CommandId::Sessions => queue_sessions::sessions_action(env, inv),
        CommandId::Update => update_cmd::update_action(env),
        // Go `cmd/tui.go`: `fmt.Errorf("tui: %w", err)`.
        CommandId::Tui => (hooks.tui)(env.cfg).map_err(|e| CmdError::plain(format!("tui: {e}"))),
        CommandId::RunClaude => model_run(env, inv, claude_spec),
        CommandId::RunGrok => model_run(env, inv, grok_spec),
        CommandId::RunK3 => model_run(env, inv, k3_spec),
        CommandId::Wait => {
            let opts = wait::WaitOptions {
                log: inv.string("log").into(),
                timeout: inv.duration("timeout"),
                poll: inv.duration("poll"),
                auto_fix_policy: env.cfg.auto_fix_policy(),
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
