//! Test fixtures for the command workflows: a temp HOME and config, fake
//! stdin, a fake provider and a fake MR resolver. Nothing reads or mutates
//! the process environment.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rival_core::cancel::Context;
use rival_core::config::Config;
use rival_core::executor::RunResult;
use rival_core::mergerequest::{self, Snapshot};
use rival_core::paths::Paths;
use rival_core::session::Session;

use crate::model_specs::{ModelSpec, codex_spec};
use crate::root::{CmdEnv, CmdError, PrepareMr, StdinSource};

pub const TEST_MR_URL: &str = "https://gitlab.example.com/team/app/-/merge_requests/42";

/// Stands for the provider transcript. The formatted review must replace
/// it, not follow it.
pub const TRANSCRIPT_MARKER: &str = "TRANSCRIPT-LINE-thinking about the code";

pub fn json_answer_log() -> String {
    format!(
        "{TRANSCRIPT_MARKER}\n{}\n",
        r#"{"summary": "One bug.", "findings": [{"file": "a.go", "line": 3, "severity": "high", "category": "bug", "title": "nil deref", "body": "x is nil", "suggestion": "check x", "confidence": 9}]}"#
    )
}

/// A review whose wording the checker flags: "utilize" and "ensure" are not
/// approved words.
pub const FLAGGED_REVIEW: &str = r#"{"summary":"We utilize the cache.","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"Cache key is wrong","body":"The code in `load()` reads 3 entries. Ensure the key is unique.","suggestion":"Return an error.","confidence":9}]}"#;

/// [`FLAGGED_REVIEW`] with approved words, as the repair call returns it.
pub const REPAIRED_REVIEW: &str = r#"{"summary":"We use the cache.","findings":[{"file":"a.go","line":3,"severity":"high","category":"bug","title":"Cache key is wrong","body":"The code in `load()` reads 3 entries. Make sure that the key is unique.","suggestion":"Return an error.","confidence":9}]}"#;

/// A provider log whose answer is [`FLAGGED_REVIEW`].
pub fn flagged_answer_log() -> String {
    format!("{TRANSCRIPT_MARKER}\n{FLAGGED_REVIEW}\n")
}

/// The `io::Error` text for the OS errors the fixtures provoke. Windows
/// prints the English system message.
pub const NO_SUCH_FILE: &str = if cfg!(windows) {
    "The system cannot find the file specified. (os error 2)"
} else {
    "No such file or directory (os error 2)"
};

/// A missing parent directory: ENOENT on Unix, `ERROR_PATH_NOT_FOUND` on
/// Windows.
pub const NO_SUCH_PATH: &str = if cfg!(windows) {
    "The system cannot find the path specified. (os error 3)"
} else {
    "No such file or directory (os error 2)"
};

/// A write to a closed standard handle: EBADF on Unix,
/// `ERROR_INVALID_HANDLE` on Windows.
pub const CLOSED_HANDLE: &str = if cfg!(windows) {
    "The handle is invalid. (os error 6)"
} else {
    "Bad file descriptor (os error 9)"
};

/// The failed step and its text when a directory is read as a file. On Unix
/// the open works and the read fails. On Windows the open fails.
pub const DIR_AS_FILE: (&str, &str) = if cfg!(windows) {
    ("open", "Access is denied. (os error 5)")
} else {
    ("read", "Is a directory (os error 21)")
};

/// The OS error behind [`CLOSED_HANDLE`].
pub fn closed_handle_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(if cfg!(windows) { 6 } else { libc::EBADF })
}

/// A temp HOME with its own `.rival`.
pub struct Fixture {
    /// Keeps the temp HOME alive.
    home: tempfile::TempDir,
    pub cfg: Config,
}

impl Fixture {
    pub fn new() -> Fixture {
        Self::with(&[], None)
    }

    /// An absolute path under the temp HOME that does not exist. A literal
    /// `/nonexistent` is drive-relative on Windows, so it needs a cwd.
    pub fn missing_dir(&self, name: &str) -> String {
        self.home.path().join(name).to_str().unwrap().to_string()
    }

    /// Extra env entries and an explicit working directory for relative
    /// `--workdir` values.
    pub fn with(extra: &[(&str, &str)], cwd: Option<PathBuf>) -> Fixture {
        Self::build(extra, cwd, None)
    }

    /// A fixture whose `~/.rival/config.yaml` holds `yaml`.
    pub fn with_config_yaml(yaml: &str) -> Fixture {
        Self::build(&[], None, Some(yaml))
    }

    /// [`Fixture::with_config_yaml`] plus extra env entries.
    pub fn with_config_and_env(yaml: &str, extra: &[(&str, &str)]) -> Fixture {
        Self::build(extra, None, Some(yaml))
    }

    fn build(extra: &[(&str, &str)], cwd: Option<PathBuf>, yaml: Option<&str>) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        if let Some(yaml) = yaml {
            std::fs::create_dir_all(home.path().join(".rival")).unwrap();
            std::fs::write(home.path().join(".rival/config.yaml"), yaml).unwrap();
        }
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("HOME".into(), home.path().to_str().unwrap().into());
        // The home dir lookup reads USERPROFILE on Windows: the same temp
        // home, so no test sees the real profile. Tests that change the home
        // use paths::HOME_VAR.
        if cfg!(windows) {
            env.insert("USERPROFILE".into(), home.path().to_str().unwrap().into());
        }
        env.insert(
            "RIVAL_HOME".into(),
            home.path().join(".rival").to_str().unwrap().into(),
        );
        env.insert("PATH".into(), "/usr/bin:/bin".into());
        for (k, v) in extra {
            env.insert((*k).into(), (*v).into());
        }
        let paths = Paths::from_vars(
            Some(home.path().join(".rival").into_os_string()),
            Some(home.path().as_os_str().to_owned()),
        );
        let cfg = Config::new(paths, env, cwd);
        Fixture { home, cfg }
    }

    /// Every saved session.
    pub fn sessions(&self) -> Vec<Session> {
        Session::load_all(self.cfg.paths())
    }
}

/// Fake stdin: it holds `data` (a regular file, so not a char device).
pub struct FakeStdin {
    pub data: Vec<u8>,
    pub char_device: bool,
    /// The stat of stdin fails (closed fd 0).
    pub stat_failed: bool,
    pub read_error: Option<String>,
    /// Panics on read, to prove a workflow never reached it.
    pub forbid_read: bool,
    pub reads: usize,
}

impl FakeStdin {
    pub fn new(input: &str) -> FakeStdin {
        FakeStdin {
            data: input.as_bytes().to_vec(),
            char_device: false,
            stat_failed: false,
            read_error: None,
            forbid_read: false,
            reads: 0,
        }
    }
}

impl StdinSource for FakeStdin {
    fn is_char_device(&self) -> bool {
        self.char_device
    }

    fn stat_failed(&self) -> bool {
        self.stat_failed
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        assert!(!self.forbid_read, "stdin was read");
        self.reads += 1;
        match &self.read_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.data.clone()),
        }
    }

    fn reader(&mut self) -> Box<dyn std::io::BufRead + '_> {
        assert!(!self.forbid_read, "stdin was read");
        assert!(self.read_error.is_none(), "read_error is for read_all");
        self.reads += 1;
        Box::new(std::io::Cursor::new(self.data.clone()))
    }
}

/// The fake provider: records what it was handed and writes `log` as
/// its output.
#[derive(Debug, Default)]
pub struct FakeRun {
    pub log: String,
    pub exit_code: i64,
    /// Return this error instead of running.
    pub error: Option<String>,
    /// Written to the live mirror when there is one.
    pub mirror_text: String,
    pub called: bool,
    pub prompt: String,
    pub effort: String,
    pub workdir: String,
    pub cred_workdir: String,
    pub review: bool,
    pub had_mirror: bool,
    /// Whether `workdir` was still on disk during the run, so a test can
    /// tell the MR checkout was closed after, not before.
    pub workdir_existed: bool,
    /// The session status seen by the provider.
    pub status_during_run: String,
    pub preflight_calls: usize,
    /// What the repair call writes to its log; `None` writes nothing.
    pub repair_reply: Option<String>,
    pub repair_calls: usize,
    pub repair_prompt: String,
    pub repair_effort: String,
    pub repair_review: bool,
    pub repair_log: String,
    pub repair_had_mirror: bool,
    pub status_during_repair: String,
}

pub type SharedRun = Rc<RefCell<FakeRun>>;

pub fn fake_run(log: &str) -> SharedRun {
    Rc::new(RefCell::new(FakeRun {
        log: log.to_string(),
        ..FakeRun::default()
    }))
}

/// Codex's spec with a passing preflight and the fake provider.
pub fn fake_spec(f: &SharedRun) -> ModelSpec {
    let mut spec = codex_spec();
    let pre = Rc::clone(f);
    spec.preflight = Box::new(move |_, _| {
        pre.borrow_mut().preflight_calls += 1;
        Ok(())
    });
    let run = Rc::clone(f);
    spec.run = Box::new(move |c| {
        let mut f = run.borrow_mut();
        if let Some(log) = c.log {
            f.repair_calls += 1;
            f.repair_prompt = c.prompt.to_string();
            f.repair_effort = c.effort.to_string();
            f.repair_review = c.review;
            f.repair_log = log.to_string();
            f.repair_had_mirror = c.out.is_some();
            f.status_during_repair = c.sess.status.clone();
            if let Some(reply) = &f.repair_reply {
                std::fs::write(log, reply)?;
            }
            return Ok(RunResult {
                exit_code: 0,
                output_bytes: 0,
                output_lines: 0,
            });
        }
        f.called = true;
        f.prompt = c.prompt.to_string();
        f.effort = c.effort.to_string();
        f.workdir = c.workdir.to_string();
        f.cred_workdir = c.cred_workdir.to_string();
        f.review = c.review;
        f.workdir_existed = Path::new(c.workdir).exists();
        f.status_during_run = c.sess.status.clone();
        if let Some(out) = c.out {
            f.had_mirror = true;
            out.write_all(f.mirror_text.as_bytes()).unwrap();
        }
        if let Some(e) = &f.error {
            anyhow::bail!("{e}");
        }
        std::fs::write(&c.sess.log_file, &f.log)?;
        Ok(RunResult {
            exit_code: f.exit_code,
            output_bytes: f.log.len() as i64,
            output_lines: 0,
        })
    });
    spec
}

/// The fake MR resolver: returns a snapshot in a temp dir for MR
/// scopes and nothing otherwise; `calls` counts every call.
pub struct FakeMr {
    pub _dir: tempfile::TempDir,
    pub snapshot_dir: PathBuf,
    pub calls: Rc<Cell<usize>>,
    pub resolver: Box<PrepareMr>,
}

pub fn fake_mr() -> FakeMr {
    let dir = tempfile::tempdir().unwrap();
    let snapshot_dir = dir.path().join("rival-mr-fake");
    std::fs::create_dir(&snapshot_dir).unwrap();
    let calls = Rc::new(Cell::new(0));
    let counter = Rc::clone(&calls);
    let snap = snapshot_dir.to_str().unwrap().to_string();
    let resolver: Box<PrepareMr> = Box::new(
        move |_: &Context, _: &Config, scope: &str, _: &str| -> Result<Option<Snapshot>, String> {
            counter.set(counter.get() + 1);
            if !mergerequest::contains(scope) {
                return Ok(None);
            }
            Ok(Some(Snapshot::new(
                snap.clone(),
                "PINNED-SNAPSHOT-SCOPE with the patch".to_string(),
                format!("GitLab MR: {TEST_MR_URL}"),
            )))
        },
    );
    FakeMr {
        _dir: dir,
        snapshot_dir,
        calls,
        resolver,
    }
}

/// What one workflow call printed and returned.
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub result: Result<(), CmdError>,
}

/// Runs `body` with a [`CmdEnv`] over `fix`, `stdin` and `prepare`.
pub fn with_env(
    fix: &Fixture,
    stdin: &mut FakeStdin,
    prepare: &PrepareMr,
    body: impl FnOnce(&mut CmdEnv<'_>) -> Result<(), CmdError>,
) -> Outcome {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = {
        let mut env = CmdEnv {
            cfg: &fix.cfg,
            stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            live_stdout: None,
            prepare_mr: prepare,
            signals: false,
        };
        body(&mut env)
    };
    Outcome {
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
        result,
    }
}

/// `rival command codex --no-queue` with `input` on
/// stdin and the fake provider.
pub fn run_command_with(
    fix: &Fixture,
    f: &SharedRun,
    input: &str,
    workdir: &str,
    prepare: &PrepareMr,
) -> Outcome {
    let spec = fake_spec(f);
    let mut stdin = FakeStdin::new(input);
    with_env(fix, &mut stdin, prepare, |env| {
        crate::model_command::run_model_command(env, &spec, workdir, true)
    })
}

/// `rival <args>` parsed by the real tree.
pub fn invocation(args: &[&str]) -> crate::tree::Invocation {
    let mut root = crate::tree::build(&crate::tree::Defaults { wait_timeout: 0 });
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match crate::tree::parse(&mut root, &args) {
        Ok(crate::tree::Parsed::Run(inv)) => inv,
        Ok(crate::tree::Parsed::Help(path)) => panic!("{args:?} asked for help on {path:?}"),
        Err(e) => panic!("{args:?}: {e}"),
    }
}

/// `rival <args>` through the root with no-op reap, update check and
/// detach. Commands must fail before any provider runs.
pub fn execute(fix: &Fixture, stdin: &mut FakeStdin, args: &[&str]) -> (i32, String, String) {
    use std::sync::Arc;

    use crate::detach::DetachOutcome;
    use crate::root::RootHooks;

    let hooks = RootHooks {
        reap: Arc::new(|_| {}),
        update_check: Arc::new(|_, _| {}),
        detach: Box::new(|| DetachOutcome::Continue),
        tui: Box::new(|_, _| Err("the TUI does not run in tests".into())),
    };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let prepare = no_mr();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = {
        let mut env = CmdEnv {
            cfg: &fix.cfg,
            stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            live_stdout: None,
            prepare_mr: &*prepare,
            signals: false,
        };
        crate::root::execute_with_wait(
            &mut env,
            &hooks,
            &crate::tree::Defaults { wait_timeout: 0 },
            &args,
            std::time::Duration::from_secs(5),
        )
    };
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// A resolver that must never be called.
pub fn no_mr() -> Box<PrepareMr> {
    Box::new(
        |_: &Context, _: &Config, scope: &str, _: &str| -> Result<Option<Snapshot>, String> {
            panic!("MR resolver called for {scope:?}")
        },
    )
}
