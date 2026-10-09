//! The OS calls the provider preflights make outside
//! [`super::run_subprocess`]: one-shot commands (spawn, then wait or
//! capture the combined output), the temp dir and temp files.
//!
//! Everything reads the injected [`Config`]: the binary is looked up in its
//! `$PATH` and the child gets its `environ()` as the full environment.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, PipeReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use super::process;
use super::subprocess::{dedup_env, getenv, io_text};
use crate::cancel::Context;
use crate::config::Config;

/// The `$PATH` that [`look_path`] reads: the first `PATH=` entry of
/// the inherited env.
pub(crate) fn path_env(cfg: &Config) -> Option<&OsStr> {
    getenv(cfg.environ(), "PATH")
}

/// Looks up `name` in `cfg`'s `$PATH`.
pub fn look_path(cfg: &Config, name: &str) -> Result<PathBuf, process::LookPathError> {
    process::look_path(name, path_env(cfg))
}

/// Where a one-shot command's stdout and stderr go.
#[derive(Clone, Copy)]
pub(crate) enum Output {
    /// The null device.
    Discard,
    /// Both streams into one buffer.
    Combined,
    Stderr,
}

/// Runs `name` with `args` and waits for it. Returns
/// the captured output (empty unless [`Output::Combined`]) and the error
/// text: the `LookPath` error, `start <path>: <io error>`, or the exit
/// status (`exit status: 1`). stdin is the null device. Under
/// [`with_context`], a done context starts nothing, and one that ends while
/// the command runs kills it; the error is then the context's
/// (`context canceled`, `context deadline exceeded`).
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
    if let Some(e) = current_context().and_then(|ctx| ctx.err()) {
        return (Vec::new(), Err(e.to_string()));
    }
    let env = match dedup_env(cfg.environ()) {
        Ok(env) => env,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let fork_error = |e: &io::Error| fork_error(&path, e);
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
    let (captured, read_err, status) = match current_context() {
        None => {
            let mut captured = Vec::new();
            let read_err = reader.and_then(|mut r| r.read_to_end(&mut captured).err());
            (captured, read_err, child.wait())
        }
        Some(ctx) => match wait_or_kill(&ctx, child, reader) {
            Ok(done) => done,
            Err(e) => return (Vec::new(), Err(e)),
        },
    };
    let result = match status {
        Err(e) => Err(format!("wait: {}", io_text(&e))),
        Ok(status) if status.success() => match read_err {
            Some(e) => Err(format!("read |0: {}", io_text(&e))),
            None => Ok(()),
        },
        Ok(status) => Err(status.to_string()),
    };
    (captured, result)
}

thread_local! {
    /// The context [`run`] honours on this thread; see [`with_context`].
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Runs `f` with `ctx` as the context of every [`run`] it makes on this
/// thread: the preflights take no context of their own, and the model
/// check must still end them on cancel or timeout.
pub fn with_context<T>(ctx: &Context, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Context>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CONTEXT.with(|c| *c.borrow_mut() = previous);
        }
    }
    let previous = CONTEXT.with(|c| c.borrow_mut().replace(ctx.clone()));
    let _restore = Restore(previous);
    f()
}

fn current_context() -> Option<Context> {
    CONTEXT.with(|c| c.borrow().clone())
}

/// A command that ran to its end: the captured output, the read error and
/// the exit status.
type Finished = (Vec<u8>, Option<io::Error>, io::Result<ExitStatus>);

/// How often a command under a context is polled.
const POLL: Duration = Duration::from_millis(20);

/// Waits for `child` and reads `reader` (on a helper thread) until both are
/// done or `ctx` is. A done context kills the child and returns its error;
/// a reader still held open by a descendant is left to its thread.
fn wait_or_kill(
    ctx: &Context,
    mut child: Child,
    reader: Option<PipeReader>,
) -> Result<Finished, String> {
    let reading = reader.map(|mut r| {
        std::thread::spawn(move || {
            let mut captured = Vec::new();
            let err = r.read_to_end(&mut captured).err();
            (captured, err)
        })
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        if let Some(e) = ctx.wait_timeout(POLL) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.to_string());
        }
    };
    let Some(reading) = reading else {
        return Ok((Vec::new(), None, status));
    };
    while !reading.is_finished() {
        if let Some(e) = ctx.wait_timeout(POLL) {
            return Err(e.to_string());
        }
    }
    let (captured, read_err) = reading
        .join()
        .unwrap_or_else(|_| (Vec::new(), Some(io::Error::other("the reader panicked"))));
    Ok((captured, read_err, status))
}

/// Builds the command for `name` with `args`, for callers that pick the
/// child's stdio and spawn it themselves. A bare name is looked up in
/// `cfg`'s `$PATH`; a name with a separator is used as-is, with no `$PATH`
/// lookup (on Windows it still gets its `PATHEXT` extension).
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
    use super::process::windows::{LookEnv, is_bare_name, look_extensions};
    if is_bare_name(name) {
        return look_path(cfg, name).map_err(|e| e.to_string());
    }
    // A relative name resolves against the child's working directory;
    // callers here leave it unset, so the current directory applies.
    look_extensions(name, "", &LookEnv::process()).map_err(|e| e.to_string())
}

/// The error text of a failed start: `start <path>: <io error>`.
pub fn fork_error(path: &std::path::Path, e: &io::Error) -> String {
    format!("start {}: {e}", path.display())
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

/// On Unix `$TMPDIR`, else `/tmp`.
#[cfg(not(windows))]
pub(crate) fn temp_dir(cfg: &Config) -> String {
    match cfg.getenv("TMPDIR") {
        "" => "/tmp".to_string(),
        dir => dir.to_string(),
    }
}

/// The temp dir on Windows. A [`Config::load`] config (production)
/// asks the OS: [`os_temp_dir`]. Its snapshot is this
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

/// The OS temp dir on Windows: `GetTempPath2W` when kernel32 has it, else
/// `GetTempPathW`, then [`trim_temp_path`]. The API reads this process's
/// `TMP`, `TEMP`, `USERPROFILE`, makes a relative value full, and gives a
/// SYSTEM process `C:\Windows\SystemTemp`. `GetTempPath2W` is probed at run
/// time, so an older Windows without it still starts.
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
        // The error is ignored; a failure (n = 0) reads as "".
        buf.truncate(n);
        let dir = std::ffi::OsString::from_wide(&buf);
        return trim_temp_path(&dir.to_string_lossy());
    }
}

/// The trim of a `GetTempPath` result: one trailing `\` goes, except for
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

const SEPARATOR: char = if cfg!(windows) { '\\' } else { '/' };

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// No separator is added after one.
fn join_temp(dir: &str, name: &str) -> String {
    if dir.ends_with(is_separator) {
        format!("{dir}{name}")
    } else {
        format!("{dir}{SEPARATOR}{name}")
    }
}

/// The last `*` becomes a random
/// number; the file is created `0600` with `O_EXCL`. Returns the open file
/// and its name. The error text is `open <name>: <errno>`, or
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

/// Makes a temp directory on Unix: the last `*` of `pattern` becomes a random
/// number; the directory is created `0700`. Returns its name. The error text
/// is `mkdir <name>: <errno>`, `stat <dir>: <errno>` when the temp dir
/// itself is missing, or after 10000 collisions
/// `mkdirtemp <dir>/<dir>/<prefix>*<suffix>: file already exists` (the dir
/// is repeated there on purpose).
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

/// The error op of a stat on a missing path.
const STAT_OP: &str = if cfg!(windows) {
    "GetFileAttributesEx"
} else {
    "stat"
};

/// A random `uint32` in decimal.
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

    /// An executable text file without a shebang is `exec format error`
    /// (os error 8); it never falls back to `/bin/sh` the way `execvp` does.
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
                format!("start {}: Exec format error (os error 8)", script.display())
            );
        }
        assert!(!marker.exists(), "the file ran through a shell");
    }

    /// A preflight command under [`with_context`]: a context already done
    /// starts nothing; one that ends while the command runs kills it and
    /// returns its error at once, whatever the output mode.
    #[cfg(unix)]
    #[test]
    fn a_done_context_kills_the_command() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        use crate::cancel::Context;

        let bin = tempfile::tempdir().unwrap();
        let pidfile = bin.path().join("pid");
        let script = bin.path().join("slow");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep 30\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let c = cfg(&[("PATH", bin.path().to_str().unwrap())]);

        let (done, cancel) = Context::background().with_cancel();
        cancel.cancel();
        let (_, err) = with_context(&done, || run(&c, "slow", &[], Output::Discard));
        assert_eq!(err.unwrap_err(), "context canceled");
        assert!(!pidfile.exists(), "a done context starts nothing");

        for out in [Output::Combined, Output::Discard, Output::Stderr] {
            let _ = std::fs::remove_file(&pidfile);
            let (ctx, _cancel) = Context::background().with_timeout(Duration::from_millis(300));
            let start = Instant::now();
            let (_, err) = retry_busy(
                || with_context(&ctx, || run(&c, "slow", &[], out)),
                |r| format!("{:?}", r.1),
            );
            assert_eq!(err.unwrap_err(), "context deadline exceeded");
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{:?}",
                start.elapsed()
            );
            let pid: i32 = std::fs::read_to_string(&pidfile)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // SAFETY: signal 0 only checks that the PID exists.
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            assert!(!alive, "the command still runs");
        }
        // Outside with_context, run waits as before.
        let quick = bin.path().join("quick");
        std::fs::write(&quick, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&quick, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (out, err) = retry_busy(
            || run(&c, "quick", &[], Output::Combined),
            |r| format!("{:?}", r.1),
        );
        assert_eq!((out, err), (b"hi\n".to_vec(), Ok(())));
    }

    /// The `execve` image: argv[0] is the bare name, empty arguments stay,
    /// and the deduped env goes through byte for byte, in order, including
    /// an entry without `=` and a non-UTF-8 value. A NUL in an argument is
    /// EINVAL before the spawn.
    #[cfg(unix)]
    #[test]
    fn run_execs_bare_argv_and_deduped_env() {
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
            format!("start {}: Invalid argument (os error 22)", sh.display())
        );
    }

    /// The exit text is std's `ExitStatus` text. Raw wait statuses: exit
    /// code in bits 8-15, a signal in bits 0-6, 0x80 = core dumped. No
    /// process is signalled.
    #[cfg(unix)]
    #[test]
    fn exit_status_text_is_the_std_text() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        assert_eq!(ExitStatus::from_raw(3 << 8).to_string(), "exit status: 3");
        assert_eq!(
            ExitStatus::from_raw(libc::SIGKILL).to_string(),
            "signal: 9 (SIGKILL)"
        );
        assert_eq!(
            ExitStatus::from_raw(libc::SIGSEGV | 0x80).to_string(),
            "signal: 11 (SIGSEGV) (core dumped)"
        );
    }

    /// The host's temp-dir variable: `TMPDIR` on Unix and `TMP`
    /// first on Windows.
    const TMP_VAR: &str = if cfg!(windows) { "TMP" } else { "TMPDIR" };

    #[cfg(not(windows))]
    #[test]
    fn temp_dir_reads_tmpdir_on_unix() {
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
        // The trim of the API result.
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

    /// The real `GetTempPath2W`/`GetTempPathW` answer, trimmed; std
    /// asks the same API and keeps the trailing `\`.
    #[cfg(windows)]
    #[test]
    fn os_temp_dir_is_the_api_answer() {
        let got = os_temp_dir();
        assert!(std::path::Path::new(&got).is_absolute(), "{got}");
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

    /// Raw Windows statuses through the real `ExitStatus`: std prints a
    /// code with the high bit set in hex.
    #[cfg(windows)]
    #[test]
    fn exit_status_text_is_the_std_windows_text() {
        use std::os::windows::process::ExitStatusExt;
        use std::process::ExitStatus;
        assert_eq!(ExitStatus::from_raw(1).to_string(), "exit code: 1");
        assert_eq!(
            ExitStatus::from_raw(0xC000_013A).to_string(),
            "exit code: 0xc000013a"
        );
    }

    #[test]
    fn create_temp_names_and_errors() {
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
        let not_found = crate::errtext::NO_SUCH_PATH;
        assert!(err.ends_with(&format!(".md: {not_found}")), "{err}");
    }

    #[test]
    fn mkdir_temp_names_and_errors() {
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
        let gone = crate::errtext::NO_SUCH_FILE;
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
        assert!(err.ends_with(": Not a directory (os error 20)"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_reports_errors_and_combined_output() {
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
        assert_eq!(err.unwrap_err(), "exit status: 3");

        let (out, err) = retry_busy(
            || run(&c, "tool", &[], Output::Discard),
            |r| format!("{:?}", r.1),
        );
        assert!(out.is_empty());
        assert_eq!(err.unwrap_err(), "exit status: 3");
    }
}
