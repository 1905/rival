//! Git scope helpers. Go: `internal/gitscope`.
//!
//! Task 2.3 ports only `env.go`, because the provider subprocess env needs
//! it. Task 2.5 adds the rest of the package here and reuses
//! [`repository_env`] unchanged.

mod env;

pub use env::repository_env;
