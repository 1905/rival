//! The Windows backend of [`super`]: provider containment with Job Objects,
//! pipes whose IO a cancellation can interrupt, and Go's Windows `LookPath`.
//!
//! Containment has two layers:
//!
//! - The owner Job. Before its first provider spawn, the Rival process that
//!   owns the review puts itself into one unnamed kill-on-close Job
//!   ([`ensure_owner_job`]). Its only handle is non-inheritable and lives in a
//!   static for the rest of the process, so the system closes it exactly when
//!   the owner dies, however it dies, and then kills every process still in
//!   the Job. A process inherits its creator's Job during `CreateProcess`, so a
//!   provider is contained from its first instruction, also while it waits
//!   for its own Job below. The detach parent never runs a provider and so
//!   never creates this Job.
//! - One nested kill-on-close Job per provider. The provider starts suspended
//!   (std `Command` keeps its `.cmd` quoting and stdio handling), is assigned
//!   to its own Job, and only then resumes. A timeout or cancel terminates
//!   that Job alone, so concurrent providers stay independent. Windows 8+
//!   nests Jobs, also under a Job the caller was already in (a CI runner).
//!   Neither Job allows breakaway or sets UI limits.
//!
//! The resume step (Toolhelp thread snapshot, `OpenThread`, `ResumeThread`)
//! is the pattern watchexec's process-wrap uses (MIT OR Apache-2.0); this is
//! an independent implementation, no code was copied.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{self, PipeReader, PipeWriter};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING,
    ERROR_NO_MORE_FILES, ERROR_OPERATION_ABORTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
    ReadFile, WriteFile,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateEventW, GetCurrentProcess, INFINITE, OpenThread, ResumeThread,
    SetEvent, THREAD_SUSPEND_RESUME, WaitForMultipleObjects,
};

use super::{Abort, ExitState, Io, KillOutcome, LookPathError};
use crate::winpath;

/// Takes ownership of a handle a Win32 call returned; NULL and
/// `INVALID_HANDLE_VALUE` are the call's failure, reported from
/// `GetLastError`.
fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle the caller just received and owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// A Win32 `BOOL` result: zero is failure.
fn check(ok: i32) -> io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `<op>: <io error>`, keeping the error kind.
fn context(op: &str, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{op}: {err}"))
}

/// An unnamed Job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and no other
/// limit: no breakaway, no UI restriction. Its handle is not inheritable.
pub fn kill_on_close_job() -> io::Result<OwnedHandle> {
    // SAFETY: NULL attributes (non-inheritable) and NULL name (unnamed).
    let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    // SAFETY: all-zero is a valid value of this plain-data struct.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: info is the struct this information class expects, with its size.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    Ok(job)
}

/// The owner Job's controlling handle. Set once, never taken or dropped:
/// the system closes it when the process ends.
static OWNER_JOB: OnceLock<OwnedHandle> = OnceLock::new();
/// Serializes the first creation, so concurrent first spawns make one Job.
static OWNER_JOB_INIT: Mutex<()> = Mutex::new(());

/// Puts this process into its owner Job, once. Every later call is a no-op.
/// Any failure (create, configure, assign) is returned and no provider may
/// start; a later call tries again. Unlike Cargo's `job::setup`, errors are
/// never ignored.
pub(crate) fn ensure_owner_job() -> io::Result<()> {
    if OWNER_JOB.get().is_some() {
        return Ok(());
    }
    let _init = OWNER_JOB_INIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if OWNER_JOB.get().is_some() {
        return Ok(());
    }
    let job = kill_on_close_job().map_err(|e| context("create owner job", e))?;
    // SAFETY: both handles are valid; GetCurrentProcess is a pseudo handle.
    check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) })
        .map_err(|e| context("assign owner job", e))?;
    // Under the init lock nothing else can have set it.
    let _ = OWNER_JOB.set(job);
    Ok(())
}

/// Whether this process created its owner Job.
pub fn owner_job_active() -> bool {
    OWNER_JOB.get().is_some()
}

/// Whether process `pid` is in this process's owner Job (tests only).
#[cfg(test)]
pub(crate) fn pid_in_owner_job(pid: u32) -> bool {
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    let Some(job) = OWNER_JOB.get() else {
        return false;
    };
    let Some(process) = crate::procinfo::windows::open(pid as i32, 0) else {
        return false;
    };
    let mut result = 0;
    // SAFETY: two valid handles and a writable BOOL.
    let ok = unsafe { IsProcessInJob(process.as_raw_handle(), job.as_raw_handle(), &mut result) };
    ok != 0 && result != 0
}

/// Test seams. Only test builds have them; production code calls nothing.
#[cfg(test)]
pub(crate) mod hooks {
    use std::cell::Cell;
    use std::sync::Mutex;

    use super::Abort;

    /// Runs after the suspended provider exists and before its own Job
    /// assignment, with its PID. Process-wide: only the startup-race helper
    /// process sets it.
    pub(crate) static AFTER_SUSPENDED_SPAWN: Mutex<Option<fn(u32)>> = Mutex::new(None);

    thread_local! {
        /// Runs on the IO thread after the abort check and before
        /// `ReadFile`/`WriteFile`: the window a one-shot cancel would miss.
        pub(crate) static BEFORE_IO: Cell<Option<fn(&Abort)>> = const { Cell::new(None) };
    }
}

/// Go `exec.Cmd.Start` for a provider: spawns `cmd` suspended inside the
/// owner Job, assigns it to its own new kill-on-close Job, then resumes it.
/// A failure after the spawn terminates and reaps the child before the
/// error returns; its Job handle closes with it.
pub(crate) fn spawn_contained(cmd: &mut Command) -> io::Result<(Child, OwnedHandle)> {
    ensure_owner_job()?;
    cmd.creation_flags(provider_creation_flags(has_console()));
    let mut child = super::spawn(cmd)?;
    #[cfg(test)]
    {
        let hook = *hooks::AFTER_SUSPENDED_SPAWN
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(hook) = hook {
            hook(child.id());
        }
    }
    match contain(&child) {
        Ok(job) => Ok((child, job)),
        Err(err) => {
            // The child never ran; it is still ours to end and reap.
            let _ = child.kill();
            let _ = child.wait();
            Err(err)
        }
    }
}

/// Whether this process has a console (`GetConsoleCP` is 0 without one).
pub fn has_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleCP;
    // SAFETY: a plain query.
    unsafe { GetConsoleCP() != 0 }
}

/// The provider's creation flags: `CREATE_SUSPENDED`, plus
/// `CREATE_NO_WINDOW` when the owner has no console. A console program
/// started by a process without a console otherwise gets a new visible
/// console window; a fully redirected detached owner (`DETACHED_PROCESS`)
/// is such a process. With a console the provider shares it, as in Go.
/// Its stdio is the provider pipes either way.
pub fn provider_creation_flags(owner_has_console: bool) -> u32 {
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    if owner_has_console {
        CREATE_SUSPENDED
    } else {
        CREATE_SUSPENDED | CREATE_NO_WINDOW
    }
}

fn contain(child: &Child) -> io::Result<OwnedHandle> {
    let job = kill_on_close_job().map_err(|e| context("create provider job", e))?;
    // SAFETY: both handles are valid and owned by us.
    check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) })
        .map_err(|e| context("assign provider job", e))?;
    resume_threads(child.id()).map_err(|e| context("resume provider", e))?;
    Ok(job)
}

/// Resumes the suspended threads of process `pid` (the initial thread of a
/// `CREATE_SUSPENDED` child). The child is unreaped, so its PID cannot be
/// reused meanwhile. An error if no suspended thread was found.
pub fn resume_threads(pid: u32) -> io::Result<()> {
    // SAFETY: a plain snapshot request.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    // SAFETY: all-zero is a valid value of this plain-data struct.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    // SAFETY: entry is a writable THREADENTRY32 with dwSize set.
    check(unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) })?;
    let mut resumed = false;
    loop {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: a plain open by thread ID.
            let thread =
                owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: thread is a valid handle with THREAD_SUSPEND_RESUME.
            let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
            if previous == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed |= previous > 0;
        }
        // SAFETY: as for Thread32First.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(err);
        }
    }
    if resumed {
        Ok(())
    } else {
        Err(io::Error::other(
            "no suspended thread of the child was found",
        ))
    }
}

/// The started provider with its own Job. The Job handle closes when this
/// drops, after the reap, and then ends anything still left in the Job.
pub(crate) struct ProcessHandle {
    child: Child,
    job: OwnedHandle,
    reaped: bool,
}

impl ProcessHandle {
    pub(crate) fn new(child: Child, job: OwnedHandle) -> ProcessHandle {
        ProcessHandle {
            child,
            job,
            reaped: false,
        }
    }

    pub(crate) fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Ends the provider's whole Job with exit code 1, the code Go's
    /// `Process.Kill` (`TerminateProcess(h, 1)`) leaves on Windows.
    pub(crate) fn kill_group(&mut self) -> KillOutcome {
        if self.reaped {
            return KillOutcome::Done;
        }
        // SAFETY: the Job handle is valid and ours.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } != 0 {
            KillOutcome::Sent
        } else {
            KillOutcome::Failed(io::Error::last_os_error().to_string())
        }
    }

    /// One non-blocking wait. Go's Windows `ExitCode` is the `uint32` exit
    /// code as an `int`, so `0xC0000005` stays positive.
    pub(crate) fn try_reap(&mut self) -> io::Result<Option<ExitState>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        self.reaped = true;
        Ok(Some(ExitState {
            code: status.code().map_or(-1, |c| i64::from(c as u32)),
            success: status.success(),
        }))
    }
}

/// Distinguishes concurrent pipe names within this process.
static PIPE_SERIAL: AtomicUsize = AtomicUsize::new(0);

/// A pipe for a provider stream whose parent end is overlapped, so a
/// cancellation can interrupt its IO ([`read_some`], [`write_some`]). The
/// child end is an ordinary synchronous handle. Both are non-inheritable
/// until std duplicates the child end into the spawn. Only these functions
/// may do IO on the parent end: std's own `Read`/`Write` would issue
/// synchronous calls on an overlapped handle.
///
/// Rust 1.98.1's `io::pipe` is a synchronous `CreatePipe`, and its child
/// stdio pipes use NT anonymous handles; neither can be cancelled this way.
/// This is the earlier std `anon_pipe` scheme (Rust 1.85
/// `sys/pal/windows/pipe.rs`): a first-instance overlapped named pipe plus a
/// client `OpenOptions` open.
pub(crate) fn overlapped_pipe(parent_reads: bool) -> io::Result<(PipeReader, PipeWriter)> {
    let mut tries = 0;
    let (name, ours) = loop {
        tries += 1;
        let name = format!(
            r"\\.\pipe\rival-provider-{}-{}-{}",
            std::process::id(),
            PIPE_SERIAL.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let wide: Vec<u16> = OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let access = if parent_reads {
            PIPE_ACCESS_INBOUND
        } else {
            PIPE_ACCESS_OUTBOUND
        };
        // SAFETY: wide is NUL-terminated; NULL security attributes make the
        // handle non-inheritable.
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                access | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        match owned(handle) {
            Ok(ours) => break (name, ours),
            // A name collision: try another name.
            Err(e) if tries < 10 && e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {}
            Err(e) => return Err(e),
        }
    };
    let theirs = OpenOptions::new()
        .read(!parent_reads)
        .write(parent_reads)
        .share_mode(0)
        .open(&name)?;
    let theirs = OwnedHandle::from(theirs);
    Ok(if parent_reads {
        (PipeReader::from(ours), PipeWriter::from(theirs))
    } else {
        (PipeReader::from(theirs), PipeWriter::from(ours))
    })
}

/// A manual-reset event, initially unset.
pub(crate) fn manual_event() -> io::Result<OwnedHandle> {
    // SAFETY: NULL attributes and name; manual reset, not signalled.
    owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
}

/// Signals `event` (the abort of [`Abort::fire`]).
pub(crate) fn set_event(event: &OwnedHandle) {
    // SAFETY: a valid event handle we own.
    unsafe { SetEvent(event.as_raw_handle()) };
}

/// One overlapped `ReadFile` or `WriteFile` on a parent pipe end, waited
/// for together with the abort event.
///
/// The abort is checked first. An abort that fires after that check, even
/// before the IO call is issued, still ends the wait: the event is level
/// triggered, so the wait sees it at once and cancels the IO
/// (`CancelIoEx`). The call then waits for the IO to finish (completed or
/// cancelled) before it returns, so the kernel never writes into a buffer
/// or OVERLAPPED that is gone. IO that completed despite the cancel keeps
/// its data; the next call sees the abort.
fn overlapped_io(handle: HANDLE, buf: *mut u8, len: usize, write: bool, abort: &Abort) -> Io {
    if abort.is_fired() {
        return Io::Aborted;
    }
    let event = match manual_event() {
        Ok(event) => event,
        Err(e) => return Io::Err(e),
    };
    #[cfg(test)]
    if let Some(hook) = hooks::BEFORE_IO.with(|h| h.get()) {
        hook(abort);
    }
    // SAFETY: all-zero is a valid OVERLAPPED.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event.as_raw_handle();
    let len = u32::try_from(len).unwrap_or(u32::MAX);
    // SAFETY: buf has len bytes and outlives the IO: this function does not
    // return before GetOverlappedResult(wait) reports the IO finished.
    let started = unsafe {
        if write {
            WriteFile(handle, buf, len, std::ptr::null_mut(), &mut ov)
        } else {
            ReadFile(handle, buf, len, std::ptr::null_mut(), &mut ov)
        }
    };
    if started == 0 {
        // SAFETY: reads the calling thread's last error.
        let err = unsafe { GetLastError() };
        if err != ERROR_IO_PENDING {
            return io_error(err, write);
        }
        let handles = [event.as_raw_handle(), abort.event.as_raw_handle()];
        // SAFETY: two valid handles.
        let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if woke != WAIT_OBJECT_0 {
            // The abort (or a failed wait): cancel this IO. If it already
            // completed, the cancel finds nothing and the result stands.
            // SAFETY: ov names our IO on this handle.
            unsafe { CancelIoEx(handle, &ov) };
        }
    }
    let mut done = 0u32;
    // SAFETY: waits until the kernel is done with ov and buf.
    if unsafe { GetOverlappedResult(handle, &ov, &mut done, 1) } != 0 {
        return Io::Done(done as usize);
    }
    // SAFETY: reads the calling thread's last error.
    let err = unsafe { GetLastError() };
    if err == ERROR_OPERATION_ABORTED {
        return Io::Aborted;
    }
    io_error(err, write)
}

/// A failed pipe call. A read at the closed write end is EOF, as in Go.
fn io_error(err: u32, write: bool) -> Io {
    if !write && (err == ERROR_BROKEN_PIPE || err == ERROR_HANDLE_EOF) {
        return Io::Done(0);
    }
    Io::Err(io::Error::from_raw_os_error(err as i32))
}

/// Reads once from an [`overlapped_pipe`] parent end.
pub(crate) fn read_some(r: &PipeReader, buf: &mut [u8], abort: &Abort) -> Io {
    overlapped_io(r.as_raw_handle(), buf.as_mut_ptr(), buf.len(), false, abort)
}

/// Writes once (possibly short) to an [`overlapped_pipe`] parent end. An
/// empty `buf` still makes one write call.
pub(crate) fn write_some(w: &PipeWriter, buf: &[u8], abort: &Abort) -> Io {
    overlapped_io(
        w.as_raw_handle(),
        buf.as_ptr().cast_mut(),
        buf.len(),
        true,
        abort,
    )
}

/// Go `(*os.File).Close` on Windows: `CloseHandle` with its error.
pub(crate) fn close_file(file: std::fs::File) -> io::Result<()> {
    let handle = file.into_raw_handle();
    // SAFETY: we own the handle and close it exactly once.
    check(unsafe { CloseHandle(handle) })
}

/// Go `syscall.FullPath`: `GetFullPathNameW`, which reads this process's
/// current directory (and another drive's own directory for `D:x`). A NUL
/// in `name` is `EINVAL`, as Go's `UTF16PtrFromString`.
pub fn full_path(name: &OsStr) -> io::Result<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

    let mut wide: Vec<u16> = name.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    wide.push(0);
    let mut buf = vec![0u16; 100];
    loop {
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: wide is NUL-terminated; buf has len writable u16s.
        let n =
            unsafe { GetFullPathNameW(wide.as_ptr(), len, buf.as_mut_ptr(), std::ptr::null_mut()) };
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize <= buf.len() {
            // Fits: n excludes the NUL. Too small: n is the size needed.
            buf.truncate(n as usize);
            return Ok(std::ffi::OsString::from_wide(&buf));
        }
        buf.resize(n as usize, 0);
    }
}

/// The program path std must start when the child runs in `dir`: Go's
/// `StartProcess` rule ([`winpath::join_exe_dir_and_fname`]) with the OS
/// [`full_path`]. An empty `dir` keeps `program`. The error is Go's
/// `StartProcess` error (`InvalidInput` reads `invalid argument`).
pub fn program_in_dir(program: &Path, dir: &Path) -> io::Result<PathBuf> {
    if dir.as_os_str().is_empty() {
        return Ok(program.to_path_buf());
    }
    let full = |b: &[u8]| -> io::Result<Vec<u8>> {
        // SAFETY: winpath passes pieces of the two valid encoded paths below,
        // split and joined only at ASCII bytes.
        let name = unsafe { OsStr::from_encoded_bytes_unchecked(b) };
        full_path(name).map(std::ffi::OsString::into_encoded_bytes)
    };
    let joined = winpath::join_exe_dir_and_fname(
        dir.as_os_str().as_encoded_bytes(),
        program.as_os_str().as_encoded_bytes(),
        &full,
    )?;
    // SAFETY: valid encoded bytes from full_path or the input.
    Ok(PathBuf::from(unsafe {
        std::ffi::OsString::from_encoded_bytes_unchecked(joined)
    }))
}

/// Go `exec.ErrNotFound` on Windows.
pub(crate) const ERR_NOT_FOUND: &str = "executable file not found in %PATH%";
/// Go `fs.ErrNotExist`.
const ERR_NOT_EXIST: &str = "file does not exist";
/// Go `fs.ErrPermission`.
const ERR_PERMISSION: &str = "permission denied";

/// The process-level inputs of Go's Windows `LookPath`, besides `PATH`:
/// `PATHEXT` and whether `NoDefaultCurrentDirectoryInExePath` is set (any
/// value). Go reads both from the process environment.
#[derive(Debug, Clone, Default)]
pub struct LookEnv {
    pub path_ext: Option<std::ffi::OsString>,
    pub no_dot: bool,
}

impl LookEnv {
    /// This process's values, as Go's `os.Getenv` reads them.
    pub fn process() -> LookEnv {
        LookEnv {
            path_ext: std::env::var_os("PATHEXT"),
            no_dot: std::env::var_os("NoDefaultCurrentDirectoryInExePath").is_some(),
        }
    }
}

/// Go `exec.pathExt`: lower-cased `PATHEXT` entries with a leading dot, or
/// `.com .exe .bat .cmd` when unset or empty.
pub fn path_ext(value: Option<&OsStr>) -> Vec<String> {
    let value = value.map(|v| v.to_string_lossy()).unwrap_or_default();
    if value.is_empty() {
        return [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec();
    }
    value
        .to_lowercase()
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| {
            if e.starts_with('.') {
                e.to_string()
            } else {
                format!(".{e}")
            }
        })
        .collect()
}

/// Go `exec.chkStat`: exists (following links) and is not a directory.
fn chk_stat(file: &Path) -> Result<(), String> {
    match std::fs::metadata(file) {
        Ok(meta) if meta.is_dir() => Err(ERR_PERMISSION.to_string()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("GetFileAttributesEx {}: {e}", file.display())),
    }
}

/// Go `exec.hasExt`: a `.` after the last separator or colon.
fn has_ext(file: &[u8]) -> bool {
    let Some(dot) = file.iter().rposition(|&c| c == b'.') else {
        return false;
    };
    file.iter()
        .rposition(|&c| c == b':' || c == b'\\' || c == b'/')
        .is_none_or(|sep| sep < dot)
}

fn with_suffix(file: &Path, ext: &str) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

/// Go `exec.findExecutable` (Windows).
fn find_executable(file: &Path, exts: &[String]) -> Result<PathBuf, String> {
    if exts.is_empty() {
        return chk_stat(file).map(|()| file.to_path_buf());
    }
    let bytes = file.as_os_str().as_encoded_bytes();
    if has_ext(bytes) && chk_stat(file).is_ok() {
        return Ok(file.to_path_buf());
    }
    // Keep checking: a name like `foo.bat.exe` still resolves.
    for ext in exts {
        let f = with_suffix(file, ext);
        if chk_stat(&f).is_ok() {
            return Ok(f);
        }
    }
    if has_ext(bytes) {
        Err(ERR_NOT_EXIST.to_string())
    } else {
        Err(ERR_NOT_FOUND.to_string())
    }
}

/// Go `exec.lookPath` (Windows) for one `PATHEXT` list.
pub fn look_path_exts(
    file: &str,
    exts: &[String],
    path_env: Option<&OsStr>,
    no_dot: bool,
    same_file: impl Fn(&Path, &Path) -> bool,
) -> Result<PathBuf, LookPathError> {
    let error = |err: String, dot_path: Option<PathBuf>| LookPathError {
        name: file.to_string(),
        err,
        dot_path,
    };
    if file.contains([':', '\\', '/']) {
        return find_executable(Path::new(file), exts).map_err(|e| error(e, None));
    }
    let mut dot: Option<PathBuf> = None;
    if !no_dot {
        let joined = winpath::join_paths(Path::new("."), Path::new(file));
        if let Ok(f) = find_executable(&joined, exts) {
            dot = Some(f);
        }
    }
    let path_env = path_env.unwrap_or_default();
    for dir in winpath::split_list(path_env.as_encoded_bytes()) {
        if dir.is_empty() {
            // Skipped, as PowerShell does.
            continue;
        }
        // SAFETY: pieces of a valid encoding split at ASCII bytes.
        let dir = unsafe { OsStr::from_encoded_bytes_unchecked(&dir) };
        let Ok(f) = find_executable(&winpath::join_paths(Path::new(dir), Path::new(file)), exts)
        else {
            continue;
        };
        if let Some(dotf) = &dot {
            // go.dev/issue/53536: the same file found through %PATH% wins
            // without an error; another file keeps the implicit ErrDot.
            if !same_file(dotf, &f) {
                return Err(error(super::ERR_DOT.to_string(), dot));
            }
        }
        if !winpath::is_abs_path(&f) {
            if dot.is_none() {
                dot = Some(f);
            }
            continue;
        }
        return Ok(f);
    }
    match dot {
        Some(dotf) => Err(error(super::ERR_DOT.to_string(), Some(dotf))),
        None => Err(error(ERR_NOT_FOUND.to_string(), None)),
    }
}

/// Go `os.SameFile` after two `os.Lstat`s: the same volume and file index,
/// without following a final link. False when either cannot be read.
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        GetFileInformationByHandle,
    };
    let id = |p: &Path| -> Option<(u32, u32, u32)> {
        let file = OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
            .ok()?;
        // SAFETY: all-zero is a valid value of this plain-data struct.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid handle and a writable struct.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return None;
        }
        Some((
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        ))
    };
    matches!((id(a), id(b)), (Some(x), Some(y)) if x == y)
}

/// Go `exec.lookExtensions(path, dir)`: resolves the extension of a name
/// that has a separator, without a `%PATH%` search.
pub fn look_extensions(path: &str, dir: &str, env: &LookEnv) -> Result<PathBuf, LookPathError> {
    if matches!(path, "" | "." | "..") {
        return Err(LookPathError {
            name: path.to_string(),
            err: ERR_NOT_FOUND.to_string(),
            dot_path: None,
        });
    }
    let mut path = path.to_string();
    if winpath::base(path.as_bytes()) == path.as_bytes() {
        path = format!(".\\{path}");
    }
    let exts = path_ext(env.path_ext.as_deref());
    let ext = winpath::ext(path.as_bytes());
    if !ext.is_empty() && exts.iter().any(|e| e.as_bytes().eq_ignore_ascii_case(ext)) {
        // Assume the path was already resolved.
        return Ok(PathBuf::from(path));
    }
    let look = |p: &str| look_path_exts(p, &exts, None, env.no_dot, same_file);
    let bytes = path.as_bytes();
    if dir.is_empty()
        || winpath::volume_name_len(bytes) != 0
        || (bytes.len() > 1 && winpath::is_sep(bytes[0]))
    {
        return look(&path);
    }
    let joined = winpath::join(&[dir.as_bytes(), bytes]);
    // SAFETY: built from two valid strings joined at ASCII bytes.
    let joined = unsafe { String::from_utf8_unchecked(joined) };
    let lp = look(&joined)?;
    let lp = lp.to_string_lossy();
    let ext = lp.strip_prefix(&joined).unwrap_or_default();
    Ok(PathBuf::from(format!("{path}{ext}")))
}
