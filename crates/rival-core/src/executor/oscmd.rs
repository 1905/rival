//! The Go `os` and `os/exec` calls the provider preflights make outside
//! [`super::run_subprocess`]: one-shot commands (`exec.Command` with
//! `Run`/`CombinedOutput`), `os.TempDir` and `os.CreateTemp`.
//!
//! Everything reads the injected [`Config`]: the binary is looked up in its
//! `$PATH` and the child gets its `environ()`, as Go's `exec.Command` with a
//! nil `Env` passes `os.Environ()`.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

use super::process;
use super::subprocess::{dedup_env, getenv, io_text, spawn_error_text};
use crate::config::Config;

/// The `$PATH` that Go's `exec.LookPath` reads: the first `PATH=` entry of
/// the inherited env.
pub(crate) fn path_env(cfg: &Config) -> Option<&OsStr> {
    getenv(cfg.environ(), "PATH")
}

/// Go `exec.LookPath(name)` against `cfg`'s `$PATH`.
pub(crate) fn look_path(cfg: &Config, name: &str) -> Result<PathBuf, process::LookPathError> {
    process::look_path(name, path_env(cfg))
}

/// Where a one-shot command's stdout and stderr go.
#[derive(Clone, Copy)]
pub(crate) enum Output {
    /// Go's nil `Stdout`/`Stderr`: the null device.
    Discard,
    /// Go `CombinedOutput`: both streams into one buffer.
    Combined,
    /// Go `cmd.Stdout = os.Stderr; cmd.Stderr = os.Stderr`.
    Stderr,
}

/// Go `exec.Command(name, args...)` plus `Run` or `CombinedOutput`. Returns
/// the captured output (empty unless [`Output::Combined`]) and Go's error
/// text: the `LookPath` error, `fork/exec <path>: <errno>`, or the exit
/// status (`exit status 1`). stdin is the null device.
pub(crate) fn run(
    cfg: &Config,
    name: &str,
    args: &[&str],
    out: Output,
) -> (Vec<u8>, Result<(), String>) {
    let path = match look_path(cfg, name) {
        Ok(path) => path,
        Err(e) => return (Vec::new(), Err(e.to_string())),
    };
    let env = match dedup_env(cfg.environ()) {
        Ok(env) => env,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let fork_error =
        |e: &io::Error| format!("fork/exec {}: {}", path.display(), spawn_error_text(e));
    let mut cmd = Command::new(&path);
    if let Err(e) = process::set_exec(&mut cmd, &path, name, args, &env) {
        return (Vec::new(), Err(fork_error(&e)));
    }
    cmd.stdin(Stdio::null());
    let mut reader = None;
    match out {
        Output::Discard => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        Output::Combined => {
            let (r, w) = match process::pipe() {
                Ok(pair) => pair,
                Err(e) => return (Vec::new(), Err(format!("pipe: {}", io_text(&e)))),
            };
            let w2 = match w.try_clone() {
                Ok(w2) => w2,
                Err(e) => return (Vec::new(), Err(format!("pipe: {}", io_text(&e)))),
            };
            cmd.stdout(w).stderr(w2);
            reader = Some(r);
        }
        Output::Stderr => match stderr_stdio() {
            Ok((o, e)) => {
                cmd.stdout(o).stderr(e);
            }
            Err(e) => return (Vec::new(), Err(io_text(&e))),
        },
    }

    let spawned = process::spawn(&mut cmd);
    // Drops our copies of the pipe's write ends so the read sees EOF.
    drop(cmd);
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return (Vec::new(), Err(fork_error(&e))),
    };
    let mut captured = Vec::new();
    let read_err = reader.and_then(|mut r| r.read_to_end(&mut captured).err());
    let status = child.wait();
    let result = match status {
        Err(e) => Err(format!("wait: {}", io_text(&e))),
        Ok(status) if status.success() => match read_err {
            Some(e) => Err(format!("read |0: {}", io_text(&e))),
            None => Ok(()),
        },
        Ok(status) => Err(exit_status_text(status)),
    };
    (captured, result)
}

/// Go `exec.Command(name, args...)` up to `Start`, for callers that pick the
/// child's stdio themselves. A bare name is looked up in `cfg`'s `$PATH`; a
/// name with a separator is used as-is, as Go skips `LookPath` for it (on
/// Windows it still gets its `PATHEXT` extension, Go's `lookExtensions`).
/// The child gets `cfg`'s env. Returns the command and the program path for
/// [`fork_error`].
pub fn command(cfg: &Config, name: &str, args: &[&str]) -> Result<(Command, PathBuf), String> {
    let path = command_path(cfg, name)?;
    let env = dedup_env(cfg.environ())?;
    let mut cmd = Command::new(&path);
    process::set_exec(&mut cmd, &path, name, args, &env).map_err(|e| fork_error(&path, &e))?;
    Ok((cmd, path))
}

/// The program path of [`command`].
#[cfg(not(windows))]
fn command_path(cfg: &Config, name: &str) -> Result<PathBuf, String> {
    if name.contains('/') {
        return Ok(PathBuf::from(name));
    }
    look_path(cfg, name).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn command_path(cfg: &Config, name: &str) -> Result<PathBuf, String> {
    use super::process::windows::{LookEnv, look_extensions};
    if crate::winpath::base(name.as_bytes()) == name.as_bytes() {
        return look_path(cfg, name).map_err(|e| e.to_string());
    }
    // Go resolves a relative name against cmd.Dir at Start; callers here
    // leave Dir unset, so the current directory applies.
    look_extensions(name, "", &LookEnv::process()).map_err(|e| e.to_string())
}

/// Go's `Start` error: `fork/exec <path>: <errno text>`.
pub fn fork_error(path: &std::path::Path, e: &io::Error) -> String {
    format!("fork/exec {}: {}", path.display(), spawn_error_text(e))
}

/// Two handles on this process's stderr, for a child's stdout and stderr.
#[cfg(unix)]
fn stderr_stdio() -> io::Result<(Stdio, Stdio)> {
    use std::os::fd::AsFd;
    let fd = io::stderr().as_fd().try_clone_to_owned()?;
    let fd2 = fd.try_clone()?;
    Ok((Stdio::from(fd), Stdio::from(fd2)))
}

/// Windows: two duplicates of this process's stderr handle
/// (`DuplicateHandle`), one for each of the child's streams.
#[cfg(windows)]
fn stderr_stdio() -> io::Result<(Stdio, Stdio)> {
    use std::os::windows::io::AsHandle;
    let handle = io::stderr().as_handle().try_clone_to_owned()?;
    let handle2 = handle.try_clone()?;
    Ok((Stdio::from(handle), Stdio::from(handle2)))
}

/// Go `(*exec.ExitError).Error()`: `exit status N`, or `signal: <name>`.
/// Windows prints an exit code of `1<<16` or more in hex (`0xc0000005`).
pub fn exit_status_text(status: ExitStatus) -> String {
    #[cfg(windows)]
    if let Some(code) = status.code() {
        return windows_exit_text(code as u32);
    }
    #[cfg(not(windows))]
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            let mut text = format!("signal: {}", signal_name(sig));
            if status.core_dumped() {
                text.push_str(" (core dumped)");
            }
            return text;
        }
    }
    format!("exit status {status}")
}

/// Go `ProcessState.String` on Windows for exit code `code` (the `uint32`
/// from `GetExitCodeProcess`): decimal below `1<<16`, else `0x` hex.
pub fn windows_exit_text(code: u32) -> String {
    if code >= 1 << 16 {
        format!("exit status {code:#x}")
    } else {
        format!("exit status {code}")
    }
}

/// Go's `syscall.Signal.String()`: the per-OS `signals` table from
/// `zerrors_<os>_<arch>.go` (Go 1.25), else `signal N`.
#[cfg(unix)]
fn signal_name(sig: i32) -> String {
    usize::try_from(sig)
        .ok()
        .and_then(|i| GO_SIGNALS.get(i))
        .filter(|name| !name.is_empty())
        .map_or_else(|| format!("signal {sig}"), |name| name.to_string())
}

/// Go `syscall.signals` on darwin (`zerrors_darwin_arm64.go`).
#[cfg(target_os = "macos")]
const GO_SIGNALS: [&str; 32] = [
    "",
    "hangup",
    "interrupt",
    "quit",
    "illegal instruction",
    "trace/BPT trap",
    "abort trap",
    "EMT trap",
    "floating point exception",
    "killed",
    "bus error",
    "segmentation fault",
    "bad system call",
    "broken pipe",
    "alarm clock",
    "terminated",
    "urgent I/O condition",
    "suspended (signal)",
    "suspended",
    "continued",
    "child exited",
    "stopped (tty input)",
    "stopped (tty output)",
    "I/O possible",
    "cputime limit exceeded",
    "filesize limit exceeded",
    "virtual timer expired",
    "profiling timer expired",
    "window size changes",
    "information request",
    "user defined signal 1",
    "user defined signal 2",
];

/// Go `syscall.signals` on linux (`zerrors_linux_amd64.go`; arm64 is the
/// same table).
#[cfg(target_os = "linux")]
const GO_SIGNALS: [&str; 32] = [
    "",
    "hangup",
    "interrupt",
    "quit",
    "illegal instruction",
    "trace/breakpoint trap",
    "aborted",
    "bus error",
    "floating point exception",
    "killed",
    "user defined signal 1",
    "segmentation fault",
    "user defined signal 2",
    "broken pipe",
    "alarm clock",
    "terminated",
    "stack fault",
    "child exited",
    "continued",
    "stopped (signal)",
    "stopped",
    "stopped (tty input)",
    "stopped (tty output)",
    "urgent I/O condition",
    "CPU time limit exceeded",
    "file size limit exceeded",
    "virtual timer expired",
    "profiling timer expired",
    "window changed",
    "I/O possible",
    "power failure",
    "bad system call",
];

/// Other Unix targets are not release platforms: every signal prints as
/// `signal N`.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
const GO_SIGNALS: [&str; 0] = [];

/// Go `os.TempDir()`: on Unix `$TMPDIR`, else `/tmp`.
#[cfg(not(windows))]
pub(crate) fn temp_dir(cfg: &Config) -> String {
    match cfg.getenv("TMPDIR") {
        "" => "/tmp".to_string(),
        dir => dir.to_string(),
    }
}

/// Go `os.TempDir()` on Windows. A [`Config::load`] config (production)
/// asks the OS, as Go does: [`os_temp_dir`]. Its snapshot is this
/// process's environment, which the OS call reads. A [`Config::new`] config
/// (tests, embedders) has an injected environment that the OS cannot see,
/// so it gets the lexical model [`windows_temp_dir`] of that environment.
#[cfg(windows)]
pub(crate) fn temp_dir(cfg: &Config) -> String {
    if cfg.temp_dir_from_os() {
        return os_temp_dir();
    }
    windows_temp_dir(|key| cfg.getenv(key))
}

/// Go `os.tempDir` on Windows: `GetTempPath2W` when kernel32 has it, else
/// `GetTempPathW`, then [`trim_temp_path`]. The API reads this process's
/// `TMP`, `TEMP`, `USERPROFILE`, makes a relative value full, and gives a
/// SYSTEM process `C:\Windows\SystemTemp`. Go probes `GetTempPath2W` at run
/// time; so does this, so an older Windows without it still starts.
#[cfg(windows)]
pub fn os_temp_dir() -> String {
    use std::os::windows::ffi::OsStringExt;
    use std::sync::OnceLock;
    use windows_sys::Win32::Storage::FileSystem::GetTempPathW;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    type GetTempPathFn = unsafe extern "system" fn(u32, *mut u16) -> u32;
    static GET_TEMP_PATH: OnceLock<GetTempPathFn> = OnceLock::new();
    let get = *GET_TEMP_PATH.get_or_init(|| {
        let kernel32: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated module name; kernel32 is always loaded.
        let module = unsafe { GetModuleHandleW(kernel32.as_ptr()) };
        if !module.is_null() {
            // SAFETY: a valid module and a NUL-terminated ANSI name.
            if let Some(f) = unsafe { GetProcAddress(module, c"GetTempPath2W".as_ptr().cast()) } {
                // SAFETY: GetTempPath2W has exactly this signature.
                return unsafe {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, GetTempPathFn>(f)
                };
            }
        }
        GetTempPathW
    });
    let mut buf = vec![0u16; 260];
    loop {
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: buf has len writable u16s.
        let n = unsafe { get(len, buf.as_mut_ptr()) } as usize;
        if n > buf.len() {
            // Too small: n is the size needed.
            buf.resize(n, 0);
            continue;
        }
        // Go ignores the error; a failure (n = 0) reads as "".
        buf.truncate(n);
        let dir = std::ffi::OsString::from_wide(&buf);
        return trim_temp_path(&dir.to_string_lossy());
    }
}

/// Go's trim of a `GetTempPath` result: one trailing `\` goes, except for
/// a drive root like `C:\`.
pub fn trim_temp_path(dir: &str) -> String {
    let b = dir.as_bytes();
    let drive_root = b.len() == 3 && b[1] == b':' && b[2] == b'\\';
    if !drive_root && dir.ends_with('\\') {
        return dir[..dir.len() - 1].to_string();
    }
    dir.to_string()
}

/// A lexical model of `GetTempPath` over an injected environment, for
/// [`Config::new`] configs only: the first non-empty of `TMP`, `TEMP`,
/// `USERPROFILE`, else the Windows directory (`SystemRoot`, or `C:\Windows`
/// when unset), then [`trim_temp_path`]. Unlike the OS it neither makes a
/// relative value full nor knows the SYSTEM temp directory; production uses
/// [`os_temp_dir`]. Pure, for tests on every platform.
pub fn windows_temp_dir<'a>(getenv: impl Fn(&str) -> &'a str) -> String {
    let found = ["TMP", "TEMP", "USERPROFILE"]
        .into_iter()
        .map(&getenv)
        .find(|v| !v.is_empty());
    let dir = match found {
        Some(v) => v,
        None => match getenv("SystemRoot") {
            "" => r"C:\Windows",
            root => root,
        },
    };
    trim_temp_path(dir)
}

/// Go `os.PathSeparator`.
const SEPARATOR: char = if cfg!(windows) { '\\' } else { '/' };

/// Go `os.IsPathSeparator`.
fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Go `os.joinPath(dir, name)`: no separator is added after one.
fn join_temp(dir: &str, name: &str) -> String {
    if dir.ends_with(is_separator) {
        format!("{dir}{name}")
    } else {
        format!("{dir}{SEPARATOR}{name}")
    }
}

/// Go `os.CreateTemp(os.TempDir(), pattern)`: the last `*` becomes a random
/// number; the file is created `0600` with `O_EXCL`. Returns the open file
/// and its name. The error text is Go's: `open <name>: <errno>`, or
/// `createtemp <dir>/<prefix>*<suffix>: file already exists` after 10000
/// collisions.
pub(crate) fn create_temp(cfg: &Config, pattern: &str) -> Result<(File, String), String> {
    let (prefix, suffix) = match pattern.rfind('*') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => (pattern, ""),
    };
    let dir = temp_dir(cfg);
    let prefix = join_temp(&dir, prefix);
    for _ in 0..10000 {
        let name = format!("{prefix}{}{suffix}", next_random());
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        match opts.open(&name) {
            Ok(file) => return Ok((file, name)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("open {name}: {}", io_text(&e))),
        }
    }
    Err(format!("createtemp {prefix}*{suffix}: file already exists"))
}

/// Go `os.MkdirTemp("", pattern)` on Unix: the last `*` becomes a random
/// number; the directory is created `0700`. Returns its name. The error text
/// is Go's: `mkdir <name>: <errno>`, `stat <dir>: <errno>` when the temp dir
/// itself is missing, or after 10000 collisions
/// `mkdirtemp <dir>/<dir>/<prefix>*<suffix>: file already exists` (Go
/// repeats the dir there).
pub(crate) fn mkdir_temp(cfg: &Config, pattern: &str) -> Result<String, String> {
    if pattern.contains(is_separator) {
        return Err(format!(
            "mkdirtemp {pattern}: pattern contains path separator"
        ));
    }
    let (prefix, suffix) = match pattern.rfind('*') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => (pattern, ""),
    };
    let dir = temp_dir(cfg);
    let prefix = join_temp(&dir, prefix);
    for _ in 0..10000 {
        let name = format!("{prefix}{}{suffix}", next_random());
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&name) {
            Ok(()) => return Ok(name),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound
                    && let Err(stat) = std::fs::metadata(&dir)
                    && stat.kind() == io::ErrorKind::NotFound
                {
                    return Err(format!("{STAT_OP} {dir}: {}", io_text(&stat)));
                }
                return Err(format!("mkdir {name}: {}", io_text(&e)));
            }
        }
    }
    Err(format!(
        "mkdirtemp {dir}{SEPARATOR}{prefix}*{suffix}: file already exists"
    ))
}

/// Go `os.Stat`'s `*PathError` op for a missing path.
const STAT_OP: &str = if cfg!(windows) {
    "GetFileAttributesEx"
} else {
    "stat"
};

/// Go `nextRandom`: a random `uint32` in decimal.
fn next_random() -> String {
    (uuid::Uuid::new_v4().as_u128() as u32).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[cfg(unix)]
    use crate::executor::testutil::retry_busy;
    use crate::paths::Paths;

    fn cfg(vars: &[(&str, &str)]) -> Config {
        let env: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::new(
            Paths::from_home(std::path::Path::new("/nonexistent")),
            env,
            None,
        )
    }

    /// Go reports `exec format error` for an executable text file without a
    /// shebang; it never falls back to `/bin/sh` the way `execvp` does.
    #[cfg(unix)]
    #[test]
    fn executable_without_shebang_is_exec_format_error() {
        let bin = tempfile::tempdir().unwrap();
        let marker = bin.path().join("marker");
        let script = bin.path().join("noshebang");
        std::fs::write(&script, format!("echo ran > '{}'\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let c = cfg(&[("PATH", bin.path().to_str().unwrap())]);
        for out in [Output::Combined, Output::Discard] {
            let (captured, err) =
                retry_busy(|| run(&c, "noshebang", &[], out), |r| format!("{:?}", r.1));
            assert!(captured.is_empty());
            assert_eq!(
                err.unwrap_err(),
                format!("fork/exec {}: exec format error", script.display())
            );
        }
        assert!(!marker.exists(), "the file ran through a shell");
    }

    /// Go's `execve` image: argv[0] is the bare name, empty arguments stay,
    /// and the deduped env goes through byte for byte, in order, including
    /// an entry without `=` and a non-UTF-8 value. A NUL in an argument is
    /// EINVAL before the spawn.
    #[cfg(unix)]
    #[test]
    fn run_execs_go_argv_and_env() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let c = cfg(&[]).with_environ(vec![
            OsString::from("PATH=/usr/bin:/bin"),
            OsString::from("Z=1"),
            OsString::from("NOEQ"),
            OsString::from_vec(b"B=\xff\xfe".to_vec()),
            OsString::from("EMPTY="),
            OsString::from("Z=2"),
        ]);
        let (out, err) = run(&c, "sh", &["-c", "printf '<%s>' \"$0\""], Output::Combined);
        assert_eq!((out, err), (b"<sh>".to_vec(), Ok(())));
        let (out, err) = run(
            &c,
            "sh",
            &["-c", "printf '<%s>' \"$0\" \"$@\"", "", "a", "", "a"],
            Output::Combined,
        );
        assert_eq!((out, err), (b"<><a><><a>".to_vec(), Ok(())));

        // Linux: `cat` is the direct child and prints its own kernel env
        // block. uutils `env` hides the `NOEQ` entry when printing.
        #[cfg(target_os = "linux")]
        {
            let (out, err) = run(&c, "cat", &["/proc/self/environ"], Output::Combined);
            assert_eq!(err, Ok(()));
            assert_eq!(
                out,
                b"PATH=/usr/bin:/bin\0NOEQ\0B=\xff\xfe\0EMPTY=\0Z=2\0".to_vec()
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            let (out, err) = run(&c, "env", &[], Output::Combined);
            assert_eq!(err, Ok(()));
            assert_eq!(
                out,
                b"PATH=/usr/bin:/bin\nNOEQ\nB=\xff\xfe\nEMPTY=\nZ=2\n".to_vec()
            );
        }

        let sh = look_path(&c, "sh").unwrap();
        let (out, err) = run(&c, "sh", &["-c", "a\0b"], Output::Combined);
        assert!(out.is_empty());
        assert_eq!(
            err.unwrap_err(),
            format!("fork/exec {}: invalid argument", sh.display())
        );
    }

    #[cfg(unix)]
    #[test]
    fn exit_status_text_matches_go_signal_tables() {
        use std::os::unix::process::ExitStatusExt;
        // Raw wait statuses: exit code in bits 8-15, a signal in bits 0-6,
        // 0x80 = core dumped. No process is signalled.
        assert_eq!(
            exit_status_text(ExitStatus::from_raw(3 << 8)),
            "exit status 3"
        );
        assert_eq!(
            exit_status_text(ExitStatus::from_raw(libc::SIGKILL)),
            "signal: killed"
        );
        assert_eq!(
            exit_status_text(ExitStatus::from_raw(libc::SIGSEGV | 0x80)),
            "signal: segmentation fault (core dumped)"
        );
        assert_eq!(signal_name(0), "signal 0");
        assert_eq!(signal_name(-1), "signal -1");
        assert_eq!(signal_name(64), "signal 64");
        #[cfg(target_os = "macos")]
        let want = [
            (libc::SIGTRAP, "trace/BPT trap"),
            (libc::SIGABRT, "abort trap"),
            (libc::SIGEMT, "EMT trap"),
            (libc::SIGBUS, "bus error"),
            (libc::SIGSYS, "bad system call"),
            (libc::SIGTSTP, "suspended"),
            (libc::SIGXCPU, "cputime limit exceeded"),
            (libc::SIGWINCH, "window size changes"),
            (libc::SIGINFO, "information request"),
            (libc::SIGUSR1, "user defined signal 1"),
            (libc::SIGUSR2, "user defined signal 2"),
            (31, "user defined signal 2"),
            (32, "signal 32"),
        ];
        #[cfg(target_os = "linux")]
        let want = [
            (libc::SIGTRAP, "trace/breakpoint trap"),
            (libc::SIGABRT, "aborted"),
            (libc::SIGBUS, "bus error"),
            (libc::SIGSTKFLT, "stack fault"),
            (libc::SIGSYS, "bad system call"),
            (libc::SIGTSTP, "stopped"),
            (libc::SIGXCPU, "CPU time limit exceeded"),
            (libc::SIGWINCH, "window changed"),
            (libc::SIGPWR, "power failure"),
            (libc::SIGUSR1, "user defined signal 1"),
            (libc::SIGUSR2, "user defined signal 2"),
            (31, "bad system call"),
            (32, "signal 32"),
        ];
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        for (sig, name) in want {
            assert_eq!(signal_name(sig), name, "signal {sig}");
        }
        for (sig, name) in [
            (libc::SIGHUP, "hangup"),
            (libc::SIGINT, "interrupt"),
            (libc::SIGQUIT, "quit"),
            (libc::SIGILL, "illegal instruction"),
            (libc::SIGFPE, "floating point exception"),
            (libc::SIGKILL, "killed"),
            (libc::SIGSEGV, "segmentation fault"),
            (libc::SIGPIPE, "broken pipe"),
            (libc::SIGALRM, "alarm clock"),
            (libc::SIGTERM, "terminated"),
            (libc::SIGCHLD, "child exited"),
            (libc::SIGCONT, "continued"),
            (libc::SIGTTIN, "stopped (tty input)"),
            (libc::SIGTTOU, "stopped (tty output)"),
            (libc::SIGURG, "urgent I/O condition"),
            (libc::SIGIO, "I/O possible"),
            (libc::SIGVTALRM, "virtual timer expired"),
            (libc::SIGPROF, "profiling timer expired"),
        ] {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert_eq!(signal_name(sig), name, "signal {sig}");
        }
    }

    /// The host's temp-dir variable: Go reads `TMPDIR` on Unix and `TMP`
    /// first on Windows.
    const TMP_VAR: &str = if cfg!(windows) { "TMP" } else { "TMPDIR" };

    #[cfg(not(windows))]
    #[test]
    fn temp_dir_follows_go_unix() {
        assert_eq!(temp_dir(&cfg(&[])), "/tmp");
        assert_eq!(temp_dir(&cfg(&[("TMPDIR", "/x/y/")])), "/x/y/");
    }

    #[test]
    fn windows_temp_dir_follows_get_temp_path() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |key: &str| vars.iter().find(|(k, _)| *k == key).map_or("", |(_, v)| *v)
        };
        assert_eq!(
            windows_temp_dir(env(&[("TMP", r"D:\t\"), ("TEMP", r"E:\x")])),
            r"D:\t"
        );
        assert_eq!(windows_temp_dir(env(&[("TEMP", r"E:\x")])), r"E:\x");
        assert_eq!(
            windows_temp_dir(env(&[("TMP", ""), ("USERPROFILE", r"C:\Users\me")])),
            r"C:\Users\me"
        );
        assert_eq!(
            windows_temp_dir(env(&[("TMP", r"C:\")])),
            r"C:\",
            "drive root"
        );
        assert_eq!(
            windows_temp_dir(env(&[("SystemRoot", r"C:\WINDOWS")])),
            r"C:\WINDOWS"
        );
        assert_eq!(windows_temp_dir(env(&[])), r"C:\Windows");
        #[cfg(windows)]
        assert_eq!(temp_dir(&cfg(&[("TMP", r"D:\t\")])), r"D:\t");
        // Go's trim of the API result.
        assert_eq!(
            trim_temp_path(r"C:\Users\me\AppData\Local\Temp\"),
            r"C:\Users\me\AppData\Local\Temp"
        );
        assert_eq!(trim_temp_path(r"C:\"), r"C:\");
        assert_eq!(trim_temp_path(r"\\srv\share\t\"), r"\\srv\share\t");
        assert_eq!(trim_temp_path(""), "");
    }

    /// A config whose env is this process's own asks the OS (Windows) or
    /// reads its `TMPDIR` snapshot (Unix). Nothing mutates the environment.
    #[test]
    fn loaded_config_temp_dir_is_the_process_answer() {
        let home = tempfile::tempdir().unwrap();
        let loaded = Config::load(&Paths::from_home(home.path()));
        assert!(loaded.temp_dir_from_os());
        assert!(
            !cfg(&[]).temp_dir_from_os(),
            "an injected env is not the process's"
        );
        #[cfg(windows)]
        assert_eq!(temp_dir(&loaded), os_temp_dir());
        #[cfg(not(windows))]
        {
            let want = std::env::var("TMPDIR")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "/tmp".to_string());
            assert_eq!(temp_dir(&loaded), want);
        }
    }

    /// The real `GetTempPath2W`/`GetTempPathW` answer, trimmed like Go; std
    /// asks the same API and keeps the trailing `\`.
    #[cfg(windows)]
    #[test]
    fn os_temp_dir_is_the_api_answer() {
        let got = os_temp_dir();
        assert!(crate::winpath::is_abs(got.as_bytes()), "{got}");
        assert_eq!(got, trim_temp_path(&std::env::temp_dir().to_string_lossy()));
    }

    #[cfg(windows)]
    const TEMP_HELPER_ENV: &str = "RIVAL_OSCMD_TEMP_HELPER";

    /// Helper process for [`os_temp_dir_resolves_a_relative_tmp`]: writes its
    /// [`os_temp_dir`] to the file named by the env var. A no-op otherwise.
    #[cfg(windows)]
    #[test]
    #[ignore = "helper process for os_temp_dir_resolves_a_relative_tmp"]
    fn os_temp_dir_helper() {
        if let Some(out) = std::env::var_os(TEMP_HELPER_ENV) {
            std::fs::write(out, os_temp_dir()).unwrap();
        }
    }

    /// The API makes a relative `TMP` full against the current directory.
    /// The helper gets its own environment; this process's stays untouched.
    #[cfg(windows)]
    #[test]
    fn os_temp_dir_resolves_a_relative_tmp() {
        use std::time::{Duration, Instant};
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("temp-answer");
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "executor::oscmd::tests::os_temp_dir_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(home.path())
        .env(TEMP_HELPER_ENV, &out)
        .env("TMP", r"rel\tmp\")
        .env("TEMP", r"Z:\not-used")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("RIVAL_HOME", home.path().join(".rival"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let mut child = process::spawn(&mut cmd).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.try_wait();
                panic!("temp helper did not exit in 30s");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success(), "{status:?}");
        let got = std::fs::read_to_string(&out).unwrap();
        let want = home.path().join("rel").join("tmp");
        assert_eq!(got, want.to_str().unwrap());
    }

    #[test]
    fn windows_exit_text_uses_hex_above_16_bits() {
        assert_eq!(windows_exit_text(0), "exit status 0");
        assert_eq!(windows_exit_text(3), "exit status 3");
        assert_eq!(windows_exit_text(0xffff), "exit status 65535");
        assert_eq!(windows_exit_text(1 << 16), "exit status 0x10000");
        assert_eq!(windows_exit_text(0xC000_0005), "exit status 0xc0000005");
        assert_eq!(windows_exit_text(u32::MAX), "exit status 0xffffffff");
    }

    /// Raw Windows statuses through the real `ExitStatus`: Go's unsigned
    /// reading, never a negative code.
    #[cfg(windows)]
    #[test]
    fn exit_status_text_matches_go_windows() {
        use std::os::windows::process::ExitStatusExt;
        assert_eq!(exit_status_text(ExitStatus::from_raw(1)), "exit status 1");
        assert_eq!(
            exit_status_text(ExitStatus::from_raw(0xC000_013A)),
            "exit status 0xc000013a"
        );
    }

    #[test]
    fn create_temp_names_and_errors_match_go() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let (file, name) = create_temp(&cfg(&[(TMP_VAR, d)]), "rival-grok-*.md").unwrap();
        drop(file);
        let base = name
            .strip_prefix(&format!("{d}{SEPARATOR}rival-grok-"))
            .unwrap();
        let digits = base.strip_suffix(".md").unwrap();
        assert!(
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
            "{name}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&name).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A trailing separator is not doubled (on Unix TMPDIR keeps it; on
        // Windows GetTempPath's trim already removed it), and no '*'
        // appends the random part.
        let slash = format!("{d}{SEPARATOR}");
        let (_f, name) = create_temp(&cfg(&[(TMP_VAR, &slash)]), "plain").unwrap();
        assert!(name.starts_with(&format!("{d}{SEPARATOR}plain")), "{name}");
        assert!(!name.contains(&format!("{SEPARATOR}{SEPARATOR}")), "{name}");

        let missing = format!("{d}{SEPARATOR}missing");
        let err = create_temp(&cfg(&[(TMP_VAR, &missing)]), "rival-grok-*.md").unwrap_err();
        assert!(
            err.starts_with(&format!("open {missing}{SEPARATOR}rival-grok-")),
            "{err}"
        );
        let not_found = if cfg!(windows) {
            "The system cannot find the path specified."
        } else {
            "no such file or directory"
        };
        assert!(err.ends_with(&format!(".md: {not_found}")), "{err}");
    }

    #[test]
    fn mkdir_temp_names_and_errors_match_go() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let name = mkdir_temp(&cfg(&[(TMP_VAR, d)]), "rival-mr-*").unwrap();
        let digits = name
            .strip_prefix(&format!("{d}{SEPARATOR}rival-mr-"))
            .unwrap();
        assert!(
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
            "{name}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&name).unwrap();
            assert!(meta.is_dir());
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }

        let slash = format!("{d}{SEPARATOR}");
        let name = mkdir_temp(&cfg(&[(TMP_VAR, &slash)]), "plain").unwrap();
        assert!(name.starts_with(&format!("{d}{SEPARATOR}plain")), "{name}");

        let missing = format!("{d}{SEPARATOR}missing");
        let gone = if cfg!(windows) {
            "The system cannot find the file specified."
        } else {
            "no such file or directory"
        };
        assert_eq!(
            mkdir_temp(&cfg(&[(TMP_VAR, &missing)]), "rival-mr-*").unwrap_err(),
            format!("{STAT_OP} {missing}: {gone}")
        );
        assert_eq!(
            mkdir_temp(&cfg(&[(TMP_VAR, d)]), "a/b*").unwrap_err(),
            "mkdirtemp a/b*: pattern contains path separator"
        );
        // A backslash is a separator only on Windows.
        let backslash = mkdir_temp(&cfg(&[(TMP_VAR, d)]), r"a\b*");
        if cfg!(windows) {
            assert_eq!(
                backslash.unwrap_err(),
                r"mkdirtemp a\b*: pattern contains path separator"
            );
        } else {
            assert!(backslash.is_ok(), "{backslash:?}");
        }
        // A file in the way of the parent fails in mkdir itself (ENOTDIR on
        // Unix), not in the temp-dir stat.
        let file = format!("{d}{SEPARATOR}file");
        std::fs::write(&file, "x").unwrap();
        let err = mkdir_temp(&cfg(&[(TMP_VAR, &file)]), "rival-mr-*").unwrap_err();
        assert!(
            err.starts_with(&format!("mkdir {file}{SEPARATOR}rival-mr-")),
            "{err}"
        );
        #[cfg(unix)]
        assert!(err.ends_with(": not a directory"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_reports_go_errors_and_combined_output() {
        let bin = tempfile::tempdir().unwrap();
        let path = bin.path().to_str().unwrap();
        let c = cfg(&[("PATH", path)]);
        let (out, err) = run(&c, "absent-tool", &[], Output::Combined);
        assert!(out.is_empty());
        assert_eq!(
            err.unwrap_err(),
            "exec: \"absent-tool\": executable file not found in $PATH"
        );

        let script = bin.path().join("tool");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'out:%s\\n' \"$*\"\nprintf 'err\\n' >&2\nexit 3\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (out, err) = retry_busy(
            || run(&c, "tool", &["a", "", "a"], Output::Combined),
            |r| format!("{:?}", r.1),
        );
        assert_eq!(String::from_utf8(out).unwrap(), "out:a  a\nerr\n");
        assert_eq!(err.unwrap_err(), "exit status 3");

        let (out, err) = retry_busy(
            || run(&c, "tool", &[], Output::Discard),
            |r| format!("{:?}", r.1),
        );
        assert!(out.is_empty());
        assert_eq!(err.unwrap_err(), "exit status 3");
    }
}
