//! Provider execution. Go: `internal/executor`.
//!
//! Task 2.3 ports the subprocess runner and quota detection; Task 2.4 adds
//! the provider adapters on top of [`subprocess::run_subprocess`].
//!
//! Every adapter reads the injected [`crate::config::Config`]: `$PATH` for
//! the preflight lookups, the env getters for credentials, and `environ()`
//! for the child. Nothing reads or mutates the process environment.

pub mod claude;
pub mod claude_docker;
pub mod codex;
pub mod grok;
pub mod kimi;
pub mod opencode;
mod oscmd;
pub mod process;
pub mod quota;
pub mod subprocess;
#[cfg(test)]
mod testutil;

pub use claude::{claude_auth_hint, claude_preflight, run_claude};
pub use claude_docker::claude_docker_preflight;
pub use codex::{codex_preflight_for, run_codex_model};
pub use grok::{grok_effort, grok_preflight, run_grok, run_grok_model};
pub use kimi::{kimi_preflight, run_kimi};
pub use opencode::{
    OpencodeRunOpts, opencode_preflight_entry, opencode_preflight_model, run_opencode,
    run_opencode_entry, run_opencode_with,
};
pub use process::{LookPathError, look_path};
pub use quota::is_quota_exhausted;
pub use subprocess::{PIPE_DRAIN_GRACE, Request, RunResult, run_subprocess, safe_env};

/// Go's `mirror io.Writer` argument: the live copy of the provider's stdout,
/// `None` in command mode.
pub type Mirror<'a> = Option<&'a mut (dyn std::io::Write + Send)>;
