//! Shared fixtures for the adapter tests: a temp home with a [`Config`] built
//! from explicit variables, task-owned fake executables, and a spawn seam
//! that records the [`Request`] instead of starting a process.
//!
//! No test reads or mutates the process environment, and no real provider
//! CLI or Docker is ever found: `PATH` holds only the test's own `bin` dir.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::subprocess::{Request, RunResult};
use crate::config::Config;
use crate::paths::Paths;
use crate::session::{NewSession, Session};

/// Retries `f` while its error text says ETXTBSY. On Linux, a fake written
/// just before its exec can be "busy" when another test thread forked while
/// the file was open for writing; the fork's child drops it at its exec.
pub(crate) fn retry_busy<T>(mut f: impl FnMut() -> T, text: impl Fn(&T) -> String) -> T {
    for _ in 0..20 {
        let out = f();
        if !text(&out).contains("text file busy") {
            return out;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    f()
}

/// A temp `HOME`, an empty `bin` dir on `PATH`, and a workdir.
pub(crate) struct Env {
    pub home: tempfile::TempDir,
    pub bin: tempfile::TempDir,
    pub work: tempfile::TempDir,
    pub vars: HashMap<String, String>,
}

impl Env {
    pub fn new() -> Env {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("HOME".to_string(), path_str(home.path()));
        vars.insert("PATH".to_string(), path_str(bin.path()));
        Env {
            home,
            bin,
            work,
            vars,
        }
    }

    /// Sets (or, with `None`, unsets) one variable.
    pub fn set(&mut self, key: &str, value: Option<&str>) -> &mut Env {
        match value {
            Some(v) => self.vars.insert(key.to_string(), v.to_string()),
            None => self.vars.remove(key),
        };
        self
    }

    pub fn paths(&self) -> Paths {
        Paths::from_home(self.home.path())
    }

    /// The config: env getters and the child `environ` hold exactly `vars`.
    pub fn config(&self) -> Config {
        Config::new(
            self.paths(),
            self.vars.clone(),
            Some(self.work.path().to_path_buf()),
        )
    }

    pub fn work_str(&self) -> String {
        path_str(self.work.path())
    }

    /// Writes an executable fake named `name` into `bin`.
    pub fn fake(&self, name: &str, script: &str) -> PathBuf {
        write_exe(&self.bin.path().join(name), script)
    }

    /// A queued session in a temp home, as Go's `session.NewQueued`.
    pub fn session(&self, cli: &str, mode: &str, model: &str, workdir: &str) -> Session {
        Session::new_queued(
            &self.paths(),
            NewSession {
                cli,
                mode,
                model,
                effort: "medium",
                workdir,
                prompt: "review",
                review_scope: "",
                group_id: "",
            },
        )
        .unwrap()
    }
}

pub(crate) fn path_str(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

/// Writes `script` to `path` with mode 0700.
pub(crate) fn write_exe(path: &Path, script: &str) -> PathBuf {
    std::fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path.to_path_buf()
}

/// What a spawn seam saw: an owned copy of one [`Request`] plus the session
/// state at the spawn.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Spawned {
    pub binary: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub prompt: String,
    pub drop_env: Vec<String>,
    pub environ: Vec<OsString>,
    pub mode: String,
    pub account: String,
}

impl Spawned {
    pub fn from(sess: &Session, req: &Request<'_>) -> Spawned {
        Spawned {
            binary: req.binary.to_string(),
            args: req.args.to_vec(),
            env: req.env.to_vec(),
            prompt: req.prompt.to_string(),
            drop_env: req.drop_env.iter().map(|s| s.to_string()).collect(),
            environ: req.environ.to_vec(),
            mode: sess.mode.clone(),
            account: sess.account.clone(),
        }
    }
}

/// A spawn seam that records the request and returns `result`.
pub(crate) fn recorder(
    seen: &mut Option<Spawned>,
    result: anyhow::Result<RunResult>,
) -> impl FnOnce(&mut Session, &Request<'_>) -> anyhow::Result<RunResult> + '_ {
    move |sess, req| {
        *seen = Some(Spawned::from(sess, req));
        result
    }
}

/// `items` as owned strings.
pub(crate) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}
