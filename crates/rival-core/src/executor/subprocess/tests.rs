//! Go: `internal/executor/subprocess_test.go` and `subprocess_unix_test.go`,
//! plus Rust-only checks of the drain bound, output accounting, env and
//! start errors.
//!
//! Every test uses a temp home, an injected `environ` (no process env is read
//! for the child or mutated), and task-owned `/bin/sh` scripts or this test
//! binary as the provider. Cleanup signals only recorded PIDs whose start
//! time still matches.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::cancel::{Context, ContextError};
use crate::session::NewSession;

const PATH_ENTRY: &str = "PATH=/usr/bin:/bin";
const HOLDER_TEST: &str = "executor::subprocess::tests::holder_helper";
const HOLDER_ENV: &str = "RIVAL_TEST_HOLDER_PIDFILE";
/// Every wait for a helper is bounded by this.
const BOUND: Duration = Duration::from_secs(30);
/// Successful fakes read the whole prompt first (a shell builtin), so a
/// prompt write never races their exit into EPIPE.
const DRAIN: &str = "while IFS= read -r l; do :; done; ";

struct Fixture {
    _home: tempfile::TempDir,
    work: tempfile::TempDir,
    paths: Paths,
    sess: Session,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(home.path());
        let mut sess = Session::new_queued(
            &paths,
            NewSession {
                cli: "test",
                mode: "raw",
                model: "none",
                effort: "low",
                workdir: work.path().to_str().unwrap(),
                ..NewSession::default()
            },
        )
        .unwrap();
        sess.mark_running(&paths).unwrap();
        Fixture {
            _home: home,
            work,
            paths,
            sess,
        }
    }

    fn log(&self) -> Vec<u8> {
        fs::read(&self.sess.log_file).unwrap()
    }

    fn run(
        &mut self,
        ctx: &Context,
        req: &Request<'_>,
        mirror: Option<&mut (dyn Write + Send)>,
    ) -> anyhow::Result<RunResult> {
        run_subprocess(ctx, &self.paths, &mut self.sess, req, mirror)
    }
}

fn environ(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn sh_args(script: &str) -> Vec<String> {
    strings(&["-c", script])
}

/// A mirror that records each write call and when it happened.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Call>>>);

/// One recorded write call.
type Call = (Instant, Vec<u8>);

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().push((Instant::now(), buf.to_vec()));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Recorder {
    fn writes(&self) -> Vec<Vec<u8>> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, w)| w.clone())
            .collect()
    }
    fn times(&self) -> Vec<Instant> {
        self.0.lock().unwrap().iter().map(|(t, _)| *t).collect()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A process this test recorded while it was alive. Drop SIGKILLs it only if
/// the PID still carries the recorded start time, so a recycled PID is never
/// signalled. It is not our child; init reaps it.
struct Recorded {
    pid: i32,
    start: i64,
}

impl Recorded {
    /// The start time must have been read while the process ran; a PID
    /// without one is never signalled or trusted.
    fn new(pid: i32, start: i64) -> Recorded {
        assert!(pid > 0 && start != 0, "no identity for pid {pid}");
        Recorded { pid, start }
    }

    fn alive(&self) -> bool {
        procinfo::same_process(self.pid, self.start)
    }
}

impl Drop for Recorded {
    fn drop(&mut self) {
        if self.alive() {
            // SAFETY: plain syscall on a PID whose identity was just checked.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
        }
    }
}

fn write_atomic(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

// ---- subprocess_test.go ----

/// Go: `TestSafeEnv_BlocksOpencodePermission`.
#[test]
fn safe_env_blocks_opencode_permission() {
    let env = safe_env(&environ(&[
        PATH_ENTRY,
        r#"OPENCODE_PERMISSION={"bash":"allow"}"#,
        "OPENCODE_CONFIG=/tmp/evil.json",
        "HTTPS_PROXY=http://evil:8080",
        "RIVAL_SAFEENV_KEEP=keepme",
    ]));
    let joined = env
        .iter()
        .map(|kv| kv.to_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for blocked in ["OPENCODE_PERMISSION=", "OPENCODE_CONFIG=", "HTTPS_PROXY="] {
        assert!(
            !joined.contains(blocked),
            "safeEnv leaked a blocked var: {blocked}"
        );
    }
    assert!(joined.contains("RIVAL_SAFEENV_KEEP=keepme"));
}

/// Go: `TestSafeEnv_BlocksGrokRuntimeVars`.
#[test]
fn safe_env_blocks_grok_runtime_vars() {
    let env = safe_env(&environ(&[PATH_ENTRY, "GROK_ANYTHING=x", "XAI_API_KEY=x"]));
    assert_eq!(env, environ(&[PATH_ENTRY]));
}

/// Go: `TestSafeEnvKeepsReviewerInItsOwnRepository`.
#[test]
fn safe_env_keeps_reviewer_in_its_own_repository() {
    let mut items: Vec<String> = [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ]
    .iter()
    .map(|k| format!("{k}=caller-repository"))
    .collect();
    items.push("GIT_SSH_COMMAND=ssh -p 2222".into());
    let items: Vec<&str> = items.iter().map(String::as_str).collect();
    let env = safe_env(&environ(&items));
    for item in &env {
        assert!(
            !item.to_str().unwrap().ends_with("=caller-repository"),
            "reviewer inherited a caller repository override: {item:?}"
        );
    }
    assert_eq!(env, environ(&["GIT_SSH_COMMAND=ssh -p 2222"]));
}

#[test]
fn safe_env_blocks_every_prefix_as_a_raw_prefix() {
    let blocked: Vec<String> = BLOCKED_ENV_PREFIXES
        .iter()
        .map(|p| format!("{p}SUFFIX=1"))
        .collect();
    let mut items: Vec<&str> = blocked.iter().map(String::as_str).collect();
    // Go matches the raw "KEY=VALUE" prefix, so these go too.
    items.extend(["HTTP_PROXY=x", "LD_PRELOAD_X=1", "DYLD_INSERT_LIBRARIES=x"]);
    items.extend(["HTTP_PROXI=kept", "ld_preload=kept", "OPENCODE=kept"]);
    assert_eq!(
        safe_env(&environ(&items)),
        environ(&["HTTP_PROXI=kept", "ld_preload=kept", "OPENCODE=kept"])
    );
}

#[test]
fn drop_matches_prefix_and_exact_rules() {
    let drop = ["ANTHROPIC_API_KEY", "AWS_", ""];
    let cases = [
        ("ANTHROPIC_API_KEY=k", true),
        ("ANTHROPIC_API_KEY_2=k", false),
        ("ANTHROPIC_API_KEY", false),
        ("AWS_REGION=x", true),
        ("AWS_=x", true),
        ("AWSX=x", false),
        // Go's HasPrefix(kv, ""+"="): an empty name drops "=…" entries.
        ("=C:=C:\\", true),
    ];
    for (kv, want) in cases {
        assert_eq!(drop_matches(OsStr::new(kv), &drop), want, "{kv}");
    }
}

#[test]
fn child_env_filters_drops_then_appends_and_dedup_keeps_last() {
    let base = environ(&[
        PATH_ENTRY,
        "KEEP=1",
        "OPENCODE_PERMISSION=x",
        "GIT_DIR=x",
        "ANTHROPIC_API_KEY=k",
        "AWS_A=1",
        "AWSX=3",
        "DUP=first",
        "DUP=second",
    ]);
    let env = strings(&["ADDED=1", "KEEP=2"]);
    let req = Request {
        binary: "env",
        args: &[],
        env: &env,
        prompt: "",
        drop_env: &["ANTHROPIC_API_KEY", "AWS_"],
        environ: &base,
    };
    let child = child_env(&req);
    assert_eq!(
        child,
        environ(&[
            PATH_ENTRY,
            "KEEP=1",
            "AWSX=3",
            "DUP=first",
            "DUP=second",
            "ADDED=1",
            "KEEP=2",
        ])
    );
    assert_eq!(
        dedup_env(&child).unwrap(),
        environ(&[PATH_ENTRY, "AWSX=3", "DUP=second", "ADDED=1", "KEEP=2"])
    );
}

#[test]
fn dedup_env_follows_go() {
    let got = dedup_env(&environ(&[
        "A=1", "noequals", "", "=X=1", "=X=2", "=lone", "A=2",
    ]))
    .unwrap();
    assert_eq!(got, environ(&["noequals", "=X=2", "=lone", "A=2"]));
    let err = dedup_env(&environ(&["A=1", "B=\0"])).unwrap_err();
    assert_eq!(err, "exec: environment variable contains NUL");
}

/// Go: `TestRunSubprocess_ContextTimeoutKillsChild`.
#[test]
fn context_timeout_kills_child() {
    let mut fx = Fixture::new();
    let (ctx, _cancel) = Context::background().with_timeout(Duration::from_millis(100));
    let base = environ(&[PATH_ENTRY]);
    let args = strings(&["5"]);
    let req = Request {
        binary: "sleep",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: &base,
    };
    let start = Instant::now();
    // sleep would run 5s; the 100ms deadline must cut it short.
    let result = fx.run(&ctx, &req, None);
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_secs(2), "child ran {elapsed:?}");
    assert_eq!(ctx.err(), Some(ContextError::DeadlineExceeded));
    // Go accepts an error or a nonzero code; SIGKILL gives Go's -1.
    assert_eq!(result.unwrap().exit_code, -1);
}

// ---- subprocess_unix_test.go ----

/// Go: `TestRunSubprocess_TimeoutKillsLauncherGrandchild`. The npm `codex`
/// launcher shape: a wrapper spawns the real binary with inherited stdio and
/// cannot forward SIGKILL. The grandchild ignores SIGTERM and keeps the
/// pipes open. A context deadline must still return promptly and leave the
/// grandchild dead.
///
/// Go's deadline is 500ms and Go reads the grandchild PID after the run.
/// Here the deadline is 3s so a loaded machine still records the
/// grandchild's identity (PID + start time) before it expires; Go's 3.5s of
/// slack past the deadline is kept.
#[test]
fn timeout_kills_launcher_grandchild() {
    launcher_grandchild_case(Some(Duration::from_secs(3)));
}

/// The same launcher, cancelled by hand right after the grandchild is
/// recorded: the return is bounded by Go's 3.5s from the cancel.
#[test]
fn cancel_kills_launcher_grandchild() {
    launcher_grandchild_case(None);
}

/// `deadline`: a context deadline from the start; `None`: a manual cancel
/// once the grandchild is recorded.
fn launcher_grandchild_case(deadline: Option<Duration>) {
    let mut fx = Fixture::new();
    let dir = fx.work.path().to_path_buf();
    let pid_file = dir.join("grandchild.pid");
    // The subshell ignores SIGTERM; SIG_IGN survives exec, so the sleep does too.
    // The pid file is published only after the marker line is written, so a
    // cancel on the pid file cannot kill the launcher before its echo.
    let launcher = write_script(
        &dir,
        "launcher.sh",
        &format!(
            "( trap '' TERM; exec sleep 20 ) &\necho $! > {}.tmp\necho launcher-started\nmv {0}.tmp {0}\nsleep 20\n",
            shell_quote(pid_file.to_str().unwrap())
        ),
    );
    let base = environ(&[PATH_ENTRY]);
    let script = vec![launcher.to_str().unwrap().to_string()];
    let (ctx, cancel) = match deadline {
        Some(d) => Context::background().with_timeout(d),
        None => Context::background().with_cancel(),
    };

    let started = Instant::now();
    let (grandchild, cancelled_at, (returned_at, res)) = std::thread::scope(|s| {
        // On a failed assertion: kill the grandchild (identity-checked),
        // then cancel, so the scope's join of the run is short. A run that
        // returns before the grandchild is recorded fails without any kill.
        let _stop = CancelOnDrop(cancel.clone());
        let rx = spawn_run(s, &mut fx, &ctx, &script, &[], "", &base);
        let grandchild = wait_until(&rx, "grandchild pid", || {
            let pid = fs::read_to_string(&pid_file).ok()?.trim().parse().ok()?;
            Some(Recorded::new(pid, procinfo::start_nanos(pid)?))
        });
        assert!(grandchild.alive());
        let cancelled_at = match deadline {
            Some(d) => started + d,
            None => {
                let at = Instant::now();
                cancel.cancel();
                at
            }
        };
        let outcome = rx.recv_timeout(BOUND).expect("run never returned");
        (grandchild, cancelled_at, outcome)
    });
    if deadline.is_some() {
        assert_eq!(ctx.err(), Some(ContextError::DeadlineExceeded));
    }
    let waited = returned_at.saturating_duration_since(cancelled_at);
    assert!(
        waited < Duration::from_millis(3500),
        "run hung {waited:?} after the cancel (grandchild held the pipes)"
    );
    assert_eq!(res.unwrap().exit_code, -1);
    assert!(contains(&fx.log(), b"launcher-started\n"));

    // The grandchild is reparented and reaped asynchronously; poll briefly.
    let end = Instant::now() + Duration::from_secs(3);
    while grandchild.alive() {
        assert!(
            Instant::now() < end,
            "grandchild {} still alive after the run was cancelled",
            grandchild.pid
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---- drain bound (escaped process group) ----

/// Cancels on drop, so a failed assertion inside a scope ends the run
/// before the scope joins it.
struct CancelOnDrop(crate::cancel::CancelFunc);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Polls `probe` until it yields a value. Fails at once if the run already
/// returned, and after `BOUND`.
fn wait_until<T>(
    run: &mpsc::Receiver<Outcome>,
    what: &str,
    mut probe: impl FnMut() -> Option<T>,
) -> T {
    let end = Instant::now() + BOUND;
    loop {
        if let Some(v) = probe() {
            return v;
        }
        if let Ok((_, res)) = run.try_recv() {
            panic!("run returned before {what} was ready: {res:?}");
        }
        assert!(Instant::now() < end, "{what} never ready");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What the holder reports about itself.
struct Holder {
    proc: Recorded,
    /// setsid succeeded: the holder left the provider group.
    escaped: bool,
    /// Its fd 0 is a pipe (the provider stdin).
    stdin_pipe: bool,
}

fn wait_holder(path: &Path, run: &mpsc::Receiver<Outcome>) -> Holder {
    wait_until(run, "holder record", || {
        let text = fs::read_to_string(path).ok()?;
        let f: Vec<i64> = text
            .split_whitespace()
            .map(|f| f.parse().unwrap())
            .collect();
        Some(Holder {
            proc: Recorded::new(f[0] as i32, f[1]),
            escaped: f[2] == 1,
            stdin_pipe: f[3] == 1,
        })
    })
}

/// Runs only inside the holder process: leaves the provider's process group
/// (setsid), records `<pid> <start> <setsid ok> <stdin is a pipe>`, then
/// holds the inherited stdin, stdout and stderr pipes without reading or
/// writing until killed or `BOUND`.
#[test]
#[ignore = "helper process for the drain-bound tests"]
fn holder_helper() {
    let Some(pid_file) = std::env::var_os(HOLDER_ENV) else {
        return;
    };
    // SAFETY: plain syscall in a single-purpose helper process.
    let escaped = unsafe { libc::setsid() } != -1;
    let pid = std::process::id() as i32;
    let start = procinfo::start_nanos(pid).unwrap_or(0);
    // SAFETY: fstat into a zeroed stat buffer.
    let stdin_pipe = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstat(0, &mut st) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
    };
    write_atomic(
        Path::new(&pid_file),
        &format!(
            "{pid} {start} {} {}\n",
            u8::from(escaped),
            u8::from(stdin_pipe)
        ),
    );
    std::thread::sleep(BOUND);
    // SAFETY: ends the helper without libtest's summary.
    unsafe { libc::_exit(0) };
}

/// The launcher script: starts the holder (this test binary in helper mode)
/// in the background, prints a line, then runs `tail`. A background job's
/// stdin is /dev/null in a non-interactive shell, so the provider stdin is
/// passed through fd 3 explicitly.
fn escaped_launcher(dir: &Path, tail: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    write_script(
        dir,
        "launcher.sh",
        &format!(
            "exec 3<&0\n{} --exact {HOLDER_TEST} --ignored --quiet 0<&3 3<&- &\nexec 3<&-\necho launcher-started\n{tail}\n",
            shell_quote(exe.to_str().unwrap())
        ),
    )
}

/// The run's outcome and when it returned.
type Outcome = (Instant, anyhow::Result<RunResult>);

/// Starts `run_subprocess` of `sh <script>` on its own thread inside `scope`
/// and returns the receiver of its outcome. The launcher scripts run through
/// `sh` (what their shebang does) rather than being executed directly:
/// executing a just-written file can fail with ETXTBSY on Linux while a
/// concurrent test's spawn still holds a copy of its write descriptor.
fn spawn_run<'s, 'e: 's>(
    s: &'s std::thread::Scope<'s, 'e>,
    fx: &'e mut Fixture,
    ctx: &'e Context,
    script: &'e [String],
    env: &'e [String],
    prompt: &'e str,
    base: &'e [OsString],
) -> mpsc::Receiver<Outcome> {
    let (tx, rx) = mpsc::channel();
    s.spawn(move || {
        let req = Request {
            binary: "sh",
            args: script,
            env,
            prompt,
            drop_env: &[],
            environ: base,
        };
        let res = fx.run(ctx, &req, None);
        tx.send((Instant::now(), res)).unwrap();
    });
    rx
}

/// A provider child left the group and holds every pipe. After the cancel the
/// group kill ends the launcher, the holder keeps the pipes, and the run
/// returns once the exact grace has passed — not earlier, and not much later.
/// The 1 MiB prompt is never read, so the prompt writer is blocked on a full
/// pipe until the grace ends it too.
#[test]
fn escaped_pipe_holder_bounds_drain_to_grace() {
    let mut fx = Fixture::new();
    let dir = fx.work.path().to_path_buf();
    let pid_file = dir.join("holder.pid");
    let launcher = escaped_launcher(&dir, "exec sleep 30");
    let script = vec![launcher.to_str().unwrap().to_string()];
    let env = vec![format!("{HOLDER_ENV}={}", pid_file.display())];
    let prompt = "p".repeat(1 << 20);
    let base = environ(&[PATH_ENTRY]);
    let (ctx, cancel) = Context::background().with_cancel();

    let (holder, cancelled_at, (returned_at, res)) = std::thread::scope(|s| {
        let _stop = CancelOnDrop(cancel.clone());
        let rx = spawn_run(s, &mut fx, &ctx, &script, &env, &prompt, &base);
        let holder = wait_holder(&pid_file, &rx);
        assert!(holder.proc.alive(), "holder must run before the cancel");
        assert!(holder.escaped, "holder must leave the provider group");
        assert!(
            holder.stdin_pipe,
            "holder must hold the provider stdin pipe"
        );
        let cancelled_at = Instant::now();
        cancel.cancel();
        let outcome = rx.recv_timeout(BOUND).expect("run never returned");
        (holder, cancelled_at, outcome)
    });

    let waited = returned_at - cancelled_at;
    assert!(
        waited >= PIPE_DRAIN_GRACE,
        "returned {waited:?} after cancel"
    );
    assert!(
        waited < PIPE_DRAIN_GRACE + Duration::from_secs(2),
        "drain overran the grace: {waited:?}"
    );
    // The launcher died of the group SIGKILL.
    assert_eq!(res.unwrap().exit_code, -1);
    // The holder never let go: the bound, not EOF, ended the drain.
    assert!(
        holder.proc.alive(),
        "holder exited; the bound was not exercised"
    );
    assert!(contains(&fx.log(), b"launcher-started\n"));
}

/// The grace starts at the cancel, not when the direct child exits: the
/// launcher exits 0 at once, the holder keeps the pipes, and the run keeps
/// draining (Go waits for the pipes before cmd.Wait) until the cancel plus
/// the grace.
#[test]
fn grace_starts_at_cancel_not_at_leader_exit() {
    let mut fx = Fixture::new();
    let dir = fx.work.path().to_path_buf();
    let pid_file = dir.join("holder.pid");
    let launcher = escaped_launcher(&dir, "exit 0");
    let script = vec![launcher.to_str().unwrap().to_string()];
    let env = vec![format!("{HOLDER_ENV}={}", pid_file.display())];
    let base = environ(&[PATH_ENTRY]);
    let (ctx, cancel) = Context::background().with_cancel();

    let (holder, cancelled_at, (returned_at, res)) = std::thread::scope(|s| {
        let _stop = CancelOnDrop(cancel.clone());
        let rx = spawn_run(s, &mut fx, &ctx, &script, &env, "", &base);
        let holder = wait_holder(&pid_file, &rx);
        assert!(holder.escaped && holder.stdin_pipe);
        // The launcher exits right after starting the holder; the run must
        // still be draining well after that.
        assert!(
            rx.recv_timeout(Duration::from_secs(1)).is_err(),
            "run returned while the holder still held the pipes"
        );
        let cancelled_at = Instant::now();
        cancel.cancel();
        let outcome = rx.recv_timeout(BOUND).expect("run never returned");
        (holder, cancelled_at, outcome)
    });

    let waited = returned_at - cancelled_at;
    assert!(
        waited >= PIPE_DRAIN_GRACE,
        "returned {waited:?} after cancel"
    );
    assert!(
        waited < PIPE_DRAIN_GRACE + Duration::from_secs(2),
        "{waited:?}"
    );
    assert!(holder.proc.alive());
    // The leader exited 0 before the cancel, so the group holds only its
    // unreaped zombie. Go's Wait then reports the Cancel result: macOS
    // kill(-pgid) answers EPERM (seen here); Linux delivers to the zombie and
    // Go reports ctx.Err() (from the kernel source, not run here); ESRCH
    // would be a clean exit.
    let text = res.as_ref().map_err(|e| e.to_string());
    if cfg!(target_os = "macos") {
        assert_eq!(
            text.unwrap_err(),
            "subprocess sh: error sending signal to Cmd: operation not permitted"
        );
    } else {
        match text {
            Ok(r) => assert_eq!(r.exit_code, 0),
            Err(e) => assert_eq!(e, "subprocess sh: context canceled"),
        }
    }
}

/// A child that never reads stdin and stays in the group: the group kill
/// closes the pipe's only reader, so the blocked prompt writer ends at once.
#[test]
fn cancel_unblocks_prompt_writer_without_grace() {
    let mut fx = Fixture::new();
    let (ctx, _cancel) = Context::background().with_timeout(Duration::from_millis(300));
    let base = environ(&[PATH_ENTRY]);
    let args = sh_args("exec sleep 30");
    let prompt = "p".repeat(1 << 20);
    let req = Request {
        binary: "sh",
        args: &args,
        env: &[],
        prompt: &prompt,
        drop_env: &[],
        environ: &base,
    };
    let start = Instant::now();
    let res = fx.run(&ctx, &req, None).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    // EPIPE on stdin is not reported: the child did not exit 0.
    assert_eq!(res.exit_code, -1);
}

/// The leader closes every pipe but keeps running: the drain ends at once,
/// and the reap (Go's cmd.Wait) still waits for it until the cancel kills
/// the group.
#[test]
fn leader_outliving_its_pipes_is_killed_on_cancel() {
    let mut fx = Fixture::new();
    let (ctx, _cancel) = Context::background().with_timeout(Duration::from_millis(300));
    let base = environ(&[PATH_ENTRY]);
    let args = sh_args("exec 0<&- 1>&- 2>&-; exec sleep 30");
    let req = Request {
        binary: "sh",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: &base,
    };
    let start = Instant::now();
    let res = fx.run(&ctx, &req, None).unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(
        res,
        RunResult {
            exit_code: -1,
            output_bytes: 0,
            output_lines: 0
        }
    );
}

#[test]
fn grace_log_event_matches_go_fields() {
    use chrono::TimeZone;
    let t = chrono::FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .unwrap();
    assert_eq!(
        grace_event("sid").format("provider pipes still open after cancel — closing them", t),
        r#"{"level":"warn","app":"rival","session":"sid","grace":5000,"time":"2026-01-02T03:04:05Z","message":"provider pipes still open after cancel — closing them"}"#
    );
}

// ---- output accounting ----

fn run_sh(
    fx: &mut Fixture,
    script: &str,
    prompt: &str,
    mirror: Option<&mut (dyn Write + Send)>,
) -> anyhow::Result<RunResult> {
    let base = environ(&[PATH_ENTRY]);
    let args = sh_args(script);
    let req = Request {
        binary: "sh",
        args: &args,
        env: &[],
        prompt,
        drop_env: &[],
        environ: &base,
    };
    fx.run(&Context::background(), &req, mirror)
}

#[test]
fn stdout_counts_lines_and_bytes_stderr_goes_to_log_only() {
    let mut fx = Fixture::new();
    let mut mirror = Recorder::default();
    let res = run_sh(
        &mut fx,
        &format!("{DRAIN}printf 'a\\nbb\\n'; printf 'err\\n' >&2; printf 'ccc'"),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    assert_eq!(
        res,
        RunResult {
            exit_code: 0,
            output_bytes: 8,
            output_lines: 3
        }
    );
    assert_eq!(
        mirror.writes(),
        vec![b"a\n".to_vec(), b"bb\n".to_vec(), b"ccc".to_vec()]
    );
    // stdout and stderr are separate locked writes; their order is up to
    // the scheduler, so check each chunk and the total.
    let log = fx.log();
    assert_eq!(log.len(), 12);
    for chunk in [&b"a\n"[..], b"bb\n", b"err\n", b"ccc"] {
        assert!(contains(&log, chunk), "{chunk:?}");
    }
}

/// Go's `ReadBytes('\n')` emits whole lines however the reads split them,
/// and emits each line as soon as it is complete.
#[test]
fn partial_lines_join_across_reads_and_stream_in_time() {
    let mut fx = Fixture::new();
    let mut mirror = Recorder::default();
    let started = Instant::now();
    let res = run_sh(
        &mut fx,
        &format!(
            "{DRAIN}printf 'he'; sleep 0.3; printf 'llo\\nwor'; sleep 0.3; printf 'ld'; printf 'E1\\n' >&2"
        ),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    let ended = Instant::now();
    assert_eq!((res.output_lines, res.output_bytes), (2, 11));
    assert_eq!(
        mirror.writes(),
        vec![b"hello\n".to_vec(), b"world".to_vec()]
    );
    let times = mirror.times();
    // "hello\n" arrived mid-run, not at exit.
    assert!(
        times[0] - started >= Duration::from_millis(250),
        "{:?}",
        times[0] - started
    );
    assert!(
        ended - times[0] >= Duration::from_millis(250),
        "{:?}",
        ended - times[0]
    );
    let log = fx.log();
    assert_eq!(log.len(), 14);
    assert!(contains(&log, b"hello\n") && contains(&log, b"world") && contains(&log, b"E1\n"));
}

#[test]
fn long_lines_have_no_limit() {
    let mut fx = Fixture::new();
    let mut mirror = Recorder::default();
    let res = run_sh(
        &mut fx,
        &format!("{DRAIN}head -c 1048576 /dev/zero | tr '\\000' x; echo; printf y"),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    assert_eq!((res.output_lines, res.output_bytes), (2, (1 << 20) + 2));
    let writes = mirror.writes();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].len(), (1 << 20) + 1);
    assert_eq!(writes[1], b"y");
}

#[test]
fn prompt_reaches_stdin() {
    let mut fx = Fixture::new();
    let mut mirror = Recorder::default();
    let res = run_sh(&mut fx, "cat", "line1\nline2", Some(&mut mirror)).unwrap();
    assert_eq!(
        (res.exit_code, res.output_lines, res.output_bytes),
        (0, 2, 11)
    );
    assert_eq!(mirror.writes().concat(), b"line1\nline2");

    let big = "q".repeat(3 << 20);
    let res = run_sh(&mut fx, "cat", &big, None).unwrap();
    assert_eq!((res.output_lines, res.output_bytes), (1, 3 << 20));
}

#[test]
fn no_mirror_still_counts_and_logs() {
    let mut fx = Fixture::new();
    let res = run_sh(&mut fx, &format!("{DRAIN}echo one; echo two"), "", None).unwrap();
    assert_eq!((res.output_lines, res.output_bytes), (2, 8));
    assert_eq!(fx.log(), b"one\ntwo\n");
}

#[test]
fn exit_codes_and_signals() {
    let mut fx = Fixture::new();
    assert_eq!(run_sh(&mut fx, "exit 3", "", None).unwrap().exit_code, 3);
    // Go's ExitCode() is -1 for a signal, not 128+n.
    assert_eq!(
        run_sh(&mut fx, "kill -TERM $$", "", None)
            .unwrap()
            .exit_code,
        -1
    );
    assert_eq!(
        run_sh(&mut fx, "kill -KILL $$", "", None)
            .unwrap()
            .exit_code,
        -1
    );
}

#[test]
fn stdin_error_is_returned_only_when_child_exits_zero() {
    let mut fx = Fixture::new();
    // The prompt is larger than any pipe buffer, so the write must hit the
    // closed reader.
    let prompt = "p".repeat(4 << 20);
    let err = run_sh(&mut fx, "exec 0<&-; exit 0", &prompt, None).unwrap_err();
    assert_eq!(
        err.to_string(),
        "write prompt to stdin: write |1: broken pipe"
    );
    let res = run_sh(&mut fx, "exec 0<&-; exit 4", &prompt, None).unwrap();
    assert_eq!(res.exit_code, 4);
}

#[test]
fn mirror_write_error_keeps_counting_and_reading() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EPIPE))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut fx = Fixture::new();
    let res = run_sh(
        &mut fx,
        &format!("{DRAIN}echo one; echo two"),
        "",
        Some(&mut Broken),
    )
    .unwrap();
    // The log write succeeded; MultiWriter reports the mirror's count (0).
    assert_eq!((res.output_lines, res.output_bytes), (2, 0));
    assert_eq!(fx.log(), b"one\ntwo\n");
}

/// A mirror that accepts at most 2 bytes per call.
struct Short(Vec<u8>);

impl Write for Short {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len().min(2);
        self.0.extend_from_slice(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn short_mirror_write_counts_its_n_once_per_line() {
    let mut fx = Fixture::new();
    let mut mirror = Short(Vec::new());
    let res = run_sh(
        &mut fx,
        &format!("{DRAIN}echo one; printf 'err\\n' >&2; echo two"),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    // One Write per line (no retry), n=2 each; stderr bytes never count.
    assert_eq!((res.output_lines, res.output_bytes), (2, 4));
    assert_eq!(mirror.0, b"ontw");
    assert_eq!(fx.log().len(), 12);
}

#[test]
fn tee_line_returns_go_multiwriter_count_and_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let open = |read_only: bool| SyncLog {
        file: Mutex::new(if read_only {
            File::open(&path).unwrap()
        } else {
            File::create(&path).unwrap()
        }),
        name: path.to_str().unwrap().to_string(),
    };
    let log = open(false);

    let mut short = Short(Vec::new());
    assert_eq!(
        tee_line(b"one\n", &log, Some(&mut short)),
        (2, Some("short write".into()))
    );

    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::EPIPE))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        tee_line(b"one\n", &log, Some(&mut Failing)),
        (0, Some("broken pipe".into()))
    );
    assert_eq!(tee_line::<Short>(b"one\n", &log, None), (4, None));
    assert_eq!(fs::read(&path).unwrap(), b"one\none\none\n");

    // A failing log write stops before the mirror.
    let log = open(true);
    let mut short = Short(Vec::new());
    let (n, err) = tee_line(b"one\n", &log, Some(&mut short));
    assert_eq!(n, 0);
    assert_eq!(
        err.unwrap(),
        format!("write {}: bad file descriptor", path.display())
    );
    assert!(short.0.is_empty());
}

#[test]
fn worker_panic_kills_reaps_and_resumes() {
    struct Panics;
    impl Write for Panics {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            panic!("mirror panic");
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut fx = Fixture::new();
    let start = Instant::now();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_sh(&mut fx, "echo x; exec sleep 30", "", Some(&mut Panics))
    }));
    assert!(caught.is_err());
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    // The provider was killed and reaped by the run itself.
    let pid = fx.sess.pid as i32;
    assert!(!procinfo::same_process(pid, fx.sess.pid_start));
}

// ---- env, workdir, session record ----

#[test]
fn child_gets_exactly_the_filtered_env() {
    let mut fx = Fixture::new();
    let base = environ(&[
        PATH_ENTRY,
        "KEEP=1",
        "OPENCODE_PERMISSION=x",
        "GIT_DIR=x",
        "GIT_SSH_COMMAND=ssh -p 2222",
        "ANTHROPIC_API_KEY=k",
        "AWS_A=1",
        "AWSX=3",
        "HTTPS_PROXYZ=1",
        "DUP=first",
        "DUP=second",
    ]);
    let env = strings(&["ADDED=1", "KEEP=2"]);
    let args = sh_args(&format!("{DRAIN}exec /usr/bin/env"));
    let req = Request {
        binary: "sh",
        args: &args,
        env: &env,
        prompt: "some prompt",
        drop_env: &["ANTHROPIC_API_KEY", "AWS_"],
        environ: &base,
    };
    let mut mirror = Recorder::default();
    fx.run(&Context::background(), &req, Some(&mut mirror))
        .unwrap();
    let out = String::from_utf8(mirror.writes().concat()).unwrap();
    // The shell itself exports these; none of them is in the input.
    let shell_vars = ["PWD=", "OLDPWD=", "SHLVL=", "_="];
    let mut got: Vec<&str> = out
        .lines()
        .filter(|l| !shell_vars.iter().any(|v| l.starts_with(v)))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            "ADDED=1",
            "AWSX=3",
            "DUP=second",
            "GIT_SSH_COMMAND=ssh -p 2222",
            "KEEP=2",
            PATH_ENTRY,
        ]
    );
}

#[test]
fn workdir_pid_and_owner_are_recorded() {
    let mut fx = Fixture::new();
    let owner = (fx.sess.owner_pid, fx.sess.owner_pid_start);
    let mut mirror = Recorder::default();
    run_sh(
        &mut fx,
        &format!("{DRAIN}echo $$; pwd -P"),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    let out = String::from_utf8(mirror.writes().concat()).unwrap();
    let mut lines = out.lines();
    let child_pid: i64 = lines.next().unwrap().parse().unwrap();
    assert_eq!(
        PathBuf::from(lines.next().unwrap()),
        fs::canonicalize(fx.work.path()).unwrap()
    );
    assert_eq!(fx.sess.pid, child_pid);
    if procinfo::start_nanos(std::process::id() as i32).is_some() {
        assert_ne!(fx.sess.pid_start, 0);
    }
    let saved = Session::load(&fx.paths, &fx.sess.id).unwrap();
    assert_eq!(
        (saved.pid, saved.pid_start),
        (fx.sess.pid, fx.sess.pid_start)
    );
    assert_eq!((saved.owner_pid, saved.owner_pid_start), owner);
    assert_eq!(owner.0, i64::from(std::process::id()));
}

// ---- start errors ----

fn start_error(fx: &mut Fixture, ctx: &Context, binary: &str, base: &[OsString]) -> String {
    let req = Request {
        binary,
        args: &[],
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: base,
    };
    fx.run(ctx, &req, None).unwrap_err().to_string()
}

#[test]
fn start_errors_match_go() {
    let mut fx = Fixture::new();
    let bg = Context::background();
    let base = environ(&[PATH_ENTRY]);

    // The log file is opened before Start, so it exists even then.
    let _ = fs::remove_file(&fx.sess.log_file);
    assert_eq!(
        start_error(&mut fx, &bg, "no-such-rival-provider", &base),
        r#"start no-such-rival-provider: exec: "no-such-rival-provider": executable file not found in $PATH"#
    );
    assert!(Path::new(&fx.sess.log_file).exists());

    // $PATH comes from the injected environ, not the process.
    assert_eq!(
        start_error(&mut fx, &bg, "sh", &environ(&["PATH=/nonexistent"])),
        r#"start sh: exec: "sh": executable file not found in $PATH"#
    );
    assert_eq!(
        start_error(&mut fx, &bg, "", &base),
        "start : exec: no command"
    );

    let (done, cancel) = bg.with_cancel();
    cancel.cancel();
    assert_eq!(
        start_error(&mut fx, &done, "sh", &base),
        "start sh: context canceled"
    );

    let plain = fx.work.path().join("plain");
    fs::write(&plain, "x").unwrap();
    let plain = plain.to_str().unwrap();
    assert_eq!(
        start_error(&mut fx, &bg, plain, &base),
        format!("start {plain}: fork/exec {plain}: permission denied")
    );
    let missing = fx.work.path().join("missing");
    let missing = missing.to_str().unwrap();
    assert_eq!(
        start_error(&mut fx, &bg, missing, &base),
        format!("start {missing}: fork/exec {missing}: no such file or directory")
    );

    // A missing workdir fails inside the child, as with Go's Setpgid start.
    fx.sess.work_dir = fx.work.path().join("gone").to_str().unwrap().to_string();
    assert_eq!(
        start_error(&mut fx, &bg, "/bin/sh", &base),
        "start /bin/sh: fork/exec /bin/sh: no such file or directory"
    );

    // Go's dedupEnv rejects a NUL.
    let mut fx = Fixture::new();
    let env = strings(&["A=\0"]);
    let req = Request {
        binary: "sh",
        args: &[],
        env: &env,
        prompt: "",
        drop_env: &[],
        environ: &base,
    };
    assert_eq!(
        fx.run(&bg, &req, None).unwrap_err().to_string(),
        "start sh: exec: environment variable contains NUL"
    );

    // The PID is not touched when the start fails.
    assert_eq!(fx.sess.pid, i64::from(std::process::id()));
}

#[test]
fn open_log_error_comes_before_start() {
    let mut fx = Fixture::new();
    fx.sess.log_file = fx
        .work
        .path()
        .join("no/such/dir.log")
        .to_str()
        .unwrap()
        .to_string();
    let base = environ(&[PATH_ENTRY]);
    assert_eq!(
        start_error(
            &mut fx,
            &Context::background(),
            "no-such-rival-provider",
            &base
        ),
        format!(
            "open log: open {}: no such file or directory",
            fx.sess.log_file
        )
    );
}

/// Go's argv: argv[0] is the bare name the caller gave, not the resolved
/// path, and empty arguments stay. A NUL in an argument is EINVAL.
#[test]
fn argv0_is_the_bare_name_and_empty_args_stay() {
    let mut fx = Fixture::new();
    let mut mirror = Recorder::default();
    run_sh(
        &mut fx,
        &format!("{DRAIN}printf '<%s>' \"$0\""),
        "",
        Some(&mut mirror),
    )
    .unwrap();
    assert_eq!(mirror.writes().concat(), b"<sh>");

    let base = environ(&[PATH_ENTRY]);
    let args = strings(&[
        "-c",
        &format!("{DRAIN}printf '<%s>' \"$@\""),
        "x",
        "",
        "a",
        "",
    ]);
    let req = Request {
        binary: "sh",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: &base,
    };
    let mut mirror = Recorder::default();
    fx.run(&Context::background(), &req, Some(&mut mirror))
        .unwrap();
    assert_eq!(mirror.writes().concat(), b"<><a><>");

    let args = strings(&["-c", "a\0b"]);
    let req = Request { args: &args, ..req };
    let sh = process::look_path("sh", getenv(&base, "PATH")).unwrap();
    assert_eq!(
        fx.run(&Context::background(), &req, None)
            .unwrap_err()
            .to_string(),
        format!("start sh: fork/exec {}: invalid argument", sh.display())
    );
}

/// Controller finding (Task 2.4): an executable text file without a shebang
/// is Go's `fork/exec <path>: exec format error`, never a `/bin/sh`
/// fallback. The provider spawn sets a process group and a workdir, so it
/// may take a different std spawn path than a plain `Command`.
#[test]
fn executable_without_shebang_is_exec_format_error() {
    let mut fx = Fixture::new();
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("marker");
    let script = bin.path().join("noshebang");
    fs::write(&script, format!("echo ran > '{}'\n", marker.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let path_entry = format!("PATH={}", bin.path().display());
    let env = environ(&[&path_entry]);
    let args: Vec<String> = Vec::new();
    let req = Request {
        binary: "noshebang",
        args: &args,
        env: &[],
        prompt: "",
        drop_env: &[],
        environ: &env,
    };
    let err = crate::executor::testutil::retry_busy(
        || fx.run(&Context::background(), &req, None),
        |r| format!("{r:?}"),
    )
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        format!(
            "start noshebang: fork/exec {}: exec format error",
            script.display()
        )
    );
    assert!(!marker.exists(), "the file ran through a shell");
}
