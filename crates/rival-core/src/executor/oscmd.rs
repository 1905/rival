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
            let (r, w) = match io::pipe() {
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

    let spawned = cmd.spawn();
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

/// Two handles on this process's stderr, for a child's stdout and stderr.
#[cfg(unix)]
fn stderr_stdio() -> io::Result<(Stdio, Stdio)> {
    use std::os::fd::AsFd;
    let fd = io::stderr().as_fd().try_clone_to_owned()?;
    let fd2 = fd.try_clone()?;
    Ok((Stdio::from(fd), Stdio::from(fd2)))
}

#[cfg(not(unix))]
fn stderr_stdio() -> io::Result<(Stdio, Stdio)> {
    Ok((Stdio::inherit(), Stdio::inherit()))
}

/// Go `(*exec.ExitError).Error()`: `exit status N`, or `signal: <name>`.
pub(crate) fn exit_status_text(status: ExitStatus) -> String {
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

/// Go `os.TempDir()` on Unix: `$TMPDIR`, else `/tmp`.
pub(crate) fn temp_dir(cfg: &Config) -> String {
    match cfg.getenv("TMPDIR") {
        "" => "/tmp".to_string(),
        dir => dir.to_string(),
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
    let prefix = if dir.ends_with('/') {
        format!("{dir}{prefix}")
    } else {
        format!("{dir}/{prefix}")
    };
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

/// Go `nextRandom`: a random `uint32` in decimal.
fn next_random() -> String {
    (uuid::Uuid::new_v4().as_u128() as u32).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

        let (out, err) = run(&c, "env", &[], Output::Combined);
        assert_eq!(err, Ok(()));
        assert_eq!(
            out,
            b"PATH=/usr/bin:/bin\nNOEQ\nB=\xff\xfe\nEMPTY=\nZ=2\n".to_vec()
        );

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

    #[test]
    fn temp_dir_follows_go_unix() {
        assert_eq!(temp_dir(&cfg(&[])), "/tmp");
        assert_eq!(temp_dir(&cfg(&[("TMPDIR", "/x/y/")])), "/x/y/");
    }

    #[test]
    fn create_temp_names_and_errors_match_go() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let (file, name) = create_temp(&cfg(&[("TMPDIR", d)]), "rival-grok-*.md").unwrap();
        drop(file);
        let base = name.strip_prefix(&format!("{d}/rival-grok-")).unwrap();
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

        // A trailing slash is not doubled, and no '*' appends the random part.
        let slash = format!("{d}/");
        let (_f, name) = create_temp(&cfg(&[("TMPDIR", &slash)]), "plain").unwrap();
        assert!(name.starts_with(&format!("{d}/plain")), "{name}");

        let missing = format!("{d}/missing");
        let err = create_temp(&cfg(&[("TMPDIR", &missing)]), "rival-grok-*.md").unwrap_err();
        assert!(
            err.starts_with(&format!("open {missing}/rival-grok-")),
            "{err}"
        );
        assert!(err.ends_with(".md: no such file or directory"), "{err}");
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
