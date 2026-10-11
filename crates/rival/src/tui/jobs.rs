//! Blocking work the model hands to the runtime. The model never reads a
//! file, loads a session, signals a process or launches a program inside
//! `update` or `draw`: it returns a [`Job`] in [`super::model::Cmd::Job`],
//! the runtime (Task 4.5) runs it on a bounded worker with [`JobEnv::run`],
//! and routes the [`JobOutput`]: a result message back to the model, an
//! opened log copy to its [`LogViews`]. Each result carries what it was for,
//! and the model drops one that no longer matches the screen.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Child;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use rival_core::executor::process;
use rival_core::logfmt;
use rival_core::paths::Paths;
use rival_core::session::Session;

use super::config_check::{
    self, CheckFn, CheckRequest, ModelsFn, ProbeRequest, SaveRequest, run_probe, run_save,
};
use super::kill::{ProcessOps, StopRequest, stop_sessions};
use super::logview::{LogRequest, ReadTail, create_group_log_view, create_log_view, load_log};
use super::model::Msg;
use super::result_view::{ResultRequest, load_result};

/// The full prompts of one run's members. The list holds summaries, which
/// drop the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptsRequest {
    pub item_key: String,
    /// The members whose prompt the summary did not carry.
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptsResult {
    pub item_key: String,
    /// Prompt by session id. A session that cannot be read is left out.
    pub prompts: HashMap<String, String>,
}

/// "o": the raw log (or every member's log, for a group) in a temp file,
/// opened with the system viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenLogRequest {
    pub sessions: Vec<Arc<Session>>,
    pub group: bool,
}

/// One piece of blocking work.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Log(LogRequest),
    /// The Result tab's parse of a finished member's log tail.
    Result(ResultRequest),
    Prompts(PromptsRequest),
    Stop(StopRequest),
    OpenLog(OpenLogRequest),
    /// The config window's `/v1/models` call (the online dot, the prefix
    /// pickers).
    Probe(Box<ProbeRequest>),
    /// The config window's model check; each row is its own message.
    Check(Box<CheckRequest>),
    /// The config window's save.
    Save(SaveRequest),
}

/// What a finished job hands back.
#[derive(Debug)]
pub enum JobOutput {
    /// A result for the model.
    Msg(Msg),
    /// A log copy handed to the viewer, for the runtime's [`LogViews`].
    Opened(OpenedLog),
    /// Nothing to deliver (a log that could not be opened).
    Nothing,
}

#[cfg(test)]
impl JobOutput {
    /// The model's message, if the job produced one.
    pub fn into_msg(self) -> Option<Msg> {
        match self {
            JobOutput::Msg(msg) => Some(msg),
            _ => None,
        }
    }
}

/// Starts the system viewer on a file. `Ok(None)` means there is no
/// launcher process to reap. Tests inject a recorder; the default is
/// [`launch_viewer`].
pub type Launch = fn(&Path) -> io::Result<Option<Child>>;

/// The opener program: `open` on macOS and `xdg-open` on the other unix
/// hosts. Where it is not installed the launch fails and the copy is removed
/// again, as for any failed launch. Windows has no opener
/// program: [`launch_viewer`] asks the shell (`ShellExecuteW`) instead.
#[cfg(target_os = "macos")]
pub const VIEWER: &str = "open";
#[cfg(all(unix, not(target_os = "macos")))]
pub const VIEWER: &str = "xdg-open";

/// A command whose stdin, stdout and stderr are the null device: it can
/// neither read the TUI's keys nor print over the screen. Unix only: the Windows viewer starts no command.
#[cfg(unix)]
pub fn quiet_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// The viewer launch for `path`.
#[cfg(unix)]
pub fn viewer_command(path: &Path) -> Command {
    let mut cmd = quiet_command(VIEWER);
    cmd.arg(path);
    cmd
}

#[cfg(unix)]
pub fn launch_viewer(path: &Path) -> io::Result<Option<Child>> {
    process::spawn(&mut viewer_command(path)).map(Some)
}

/// Windows: the shell's "open" verb on the copy, through `ShellExecuteW`.
/// The path is passed as the file argument, never through a command line,
/// so no shell parses it. There is no launcher process to reap (`None`),
/// and whatever app the shell starts is never waited for or ended.
#[cfg(windows)]
pub fn launch_viewer(path: &Path) -> io::Result<Option<Child>> {
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    shell_open(path, |verb, file| {
        // Microsoft asks for COM on the calling thread before ShellExecute.
        // SAFETY: plain COM setup on this job thread, undone below.
        let com = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let code = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb,
                file,
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        if com >= 0 {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        code
    })
}

/// [`launch_viewer`] with the `ShellExecuteW` call injected: `exec` gets
/// the NUL-terminated verb and file and returns its code (above 32 on
/// success).
#[cfg(windows)]
pub fn shell_open(
    path: &Path,
    exec: impl FnOnce(*const u16, *const u16) -> isize,
) -> io::Result<Option<Child>> {
    use std::os::windows::ffi::OsStrExt;
    let mut file: Vec<u16> = path.as_os_str().encode_wide().collect();
    if file.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    file.push(0);
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let code = exec(verb.as_ptr(), file.as_ptr());
    if code > 32 {
        Ok(None)
    } else {
        Err(io::Error::other(format!(
            "ShellExecute failed with code {code}"
        )))
    }
}

/// How long an opened log copy lives while the TUI runs.
pub const LOG_VIEW_TTL: Duration = Duration::from_secs(10 * 60);

/// A log copy "o" handed to the viewer, and the launcher to reap. It owns
/// both: dropping it applies the exit policy of [`OpenedLog::release`], so
/// an outcome the runtime never adopts (say, during shutdown) loses neither
/// the file nor its cleanup rule.
#[derive(Debug)]
pub struct OpenedLog {
    pub path: PathBuf,
    pub opened_at: Instant,
    launcher: Option<Child>,
    /// The copy was removed, or deliberately left for a running launcher.
    settled: bool,
}

impl OpenedLog {
    pub fn new(path: PathBuf, launcher: Option<Child>, opened_at: Instant) -> OpenedLog {
        OpenedLog {
            path,
            opened_at,
            launcher,
            settled: false,
        }
    }

    /// Whether a launcher process is still held (not yet seen to exit).
    #[cfg(test)]
    pub fn has_launcher(&self) -> bool {
        self.launcher.is_some()
    }

    /// Reaps the launcher once it has exited. A `try_wait` error is not an
    /// exit: the launcher stays held.
    fn reap(&mut self) {
        if let Some(child) = &mut self.launcher
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.launcher = None;
        }
    }

    fn remove(&mut self) {
        if !self.settled {
            let _ = fs::remove_file(&self.path);
            self.settled = true;
        }
    }

    /// The exit policy at `now`. An exited launcher is reaped; a running one
    /// is neither waited for nor killed, and is reparented when the TUI
    /// exits. A copy younger than [`LOG_VIEW_TTL`] stays in the temp dir even
    /// when its launcher is done or there was none: plain `open` returns once
    /// LaunchServices has the request, before the app reads the path. Only an
    /// expired copy is removed.
    fn release(&mut self, now: Instant) {
        if self.settled {
            return;
        }
        self.reap();
        if now.saturating_duration_since(self.opened_at) >= LOG_VIEW_TTL {
            self.remove();
        } else {
            self.settled = true;
        }
    }
}

impl Drop for OpenedLog {
    fn drop(&mut self) {
        self.release(Instant::now());
    }
}

/// Owns the log copies "o" made, with no thread or timer of its own. The
/// runtime keeps one for the TUI's lifetime and calls [`LogViews::sweep`]
/// from its tick: it reaps finished launchers and removes copies older than
/// [`LOG_VIEW_TTL`]. On exit [`LogViews::close`] (also run on drop) applies
/// the exit policy of `OpenedLog::release` to every copy still held. Known
/// limit: no process outlives the TUI to finish the TTL, so every copy
/// younger than the TTL at exit stays in the temp dir for the OS to clean.
#[derive(Debug, Default)]
pub struct LogViews {
    views: Vec<OpenedLog>,
}

impl LogViews {
    pub fn adopt(&mut self, opened: OpenedLog) {
        self.views.push(opened);
    }

    /// How many copies or launchers are still held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.views.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// Reaps finished launchers and removes copies older than the TTL. An
    /// entry goes once its copy is removed and its launcher reaped.
    pub fn sweep(&mut self, now: Instant) {
        self.views.retain_mut(|v| {
            v.reap();
            if now.saturating_duration_since(v.opened_at) >= LOG_VIEW_TTL {
                v.remove();
            }
            !(v.settled && v.launcher.is_none())
        });
    }

    /// On exit: releases every copy still held.
    pub fn close(&mut self) {
        self.close_at(Instant::now());
    }

    fn close_at(&mut self, now: Instant) {
        for mut v in self.views.drain(..) {
            v.release(now);
        }
    }
}

impl Drop for LogViews {
    fn drop(&mut self) {
        self.close();
    }
}

/// What jobs need from the outside world. The runtime builds one with
/// [`JobEnv::system`]; tests point it at a temp home, fake processes and a
/// recording launcher.
#[derive(Debug, Clone)]
pub struct JobEnv {
    pub paths: Paths,
    pub procs: ProcessOps,
    pub read_tail: ReadTail,
    pub launch: Launch,
    /// Where "o" writes its log copies.
    pub temp_dir: PathBuf,
    /// The proxy's model list; tests fake it.
    pub models: ModelsFn,
    /// The model check; tests fake it.
    pub check: CheckFn,
}

impl JobEnv {
    pub fn system(paths: Paths) -> JobEnv {
        JobEnv {
            paths,
            procs: ProcessOps::SYSTEM,
            read_tail: logfmt::read_tail,
            launch: launch_viewer,
            temp_dir: std::env::temp_dir(),
            models: rival_core::proxy::models,
            check: crate::check::run_check,
        }
    }

    /// Runs `job` to completion, dropping any results along the way.
    #[cfg(test)]
    pub fn run(&self, job: Job) -> JobOutput {
        self.run_with(job, &|_| {})
    }

    /// Runs `job` to completion. A job with results along the way (the
    /// check's rows) hands each to `emit` before it returns.
    pub fn run_with(&self, job: Job, emit: &(dyn Fn(Msg) + Sync)) -> JobOutput {
        match job {
            Job::Probe(req) => JobOutput::Msg(Msg::ProxyModels(run_probe(*req, self.models))),
            Job::Check(req) => JobOutput::Msg(config_check::run_check(*req, self.check, emit)),
            Job::Save(req) => JobOutput::Msg(Msg::ConfigSaved(Box::new(run_save(req)))),
            Job::Log(req) => JobOutput::Msg(Msg::Log(load_log(req, self.read_tail))),
            Job::Result(req) => JobOutput::Msg(Msg::Result(load_result(req, self.read_tail))),
            Job::Prompts(req) => JobOutput::Msg(Msg::Prompts(self.load_prompts(req))),
            Job::Stop(req) => {
                JobOutput::Msg(Msg::Stopped(stop_sessions(&self.paths, &self.procs, req)))
            }
            Job::OpenLog(req) => match self.open_log(&req) {
                Some(opened) => JobOutput::Opened(opened),
                None => JobOutput::Nothing,
            },
        }
    }

    /// Loads the prompts of the members the summaries left without one.
    fn load_prompts(&self, req: PromptsRequest) -> PromptsResult {
        let prompts = req
            .ids
            .into_iter()
            .filter_map(|id| {
                let full = Session::load(&self.paths, &id).ok()?;
                (!full.prompt.is_empty()).then_some((id, full.prompt))
            })
            .collect();
        PromptsResult {
            item_key: req.item_key,
            prompts,
        }
    }

    /// Opens a run's log, or a group's logs, in the viewer. A copy the viewer
    /// cannot be started for is removed again; errors are dropped.
    fn open_log(&self, req: &OpenLogRequest) -> Option<OpenedLog> {
        let view = if req.group {
            create_group_log_view(&self.temp_dir, &req.sessions)
        } else {
            match req.sessions.first() {
                Some(s) if !s.log_file.is_empty() => create_log_view(&self.temp_dir, s),
                _ => return None,
            }
        };
        let path = view.ok()?;
        match (self.launch)(&path) {
            Err(_) => {
                let _ = fs::remove_file(&path);
                None
            }
            Ok(launcher) => Some(OpenedLog::new(path, launcher, Instant::now())),
        }
    }
}

#[cfg(test)]
mod tests;
