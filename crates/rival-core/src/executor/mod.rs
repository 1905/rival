//! Provider execution. Go: `internal/executor`.
//!
//! Task 2.3 ports the subprocess runner and quota detection; Task 2.4 adds
//! the provider adapters on top of [`subprocess::run_subprocess`].

pub mod process;
pub mod quota;
pub mod subprocess;

pub use process::{LookPathError, look_path};
pub use quota::is_quota_exhausted;
pub use subprocess::{PIPE_DRAIN_GRACE, Request, RunResult, run_subprocess, safe_env};
