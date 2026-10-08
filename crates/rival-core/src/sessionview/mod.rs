//! Display data derived from stored sessions: how a group is bucketed,
//! labelled, and timed, plus the incremental cache and directory watcher the
//! dashboard reads through.
//!
//! It sits between [`crate::session`] (file parsing) and the front end, and
//! never mutates the sessions it receives.

pub mod cache;
pub mod group;
pub mod watcher;

pub use cache::Cache;
pub use group::{Bucket, effort, elapsed, elapsed_at, group, kind, status};
pub use watcher::{LoadProgress, SessionEvent, SessionWatcher, watch_sessions};
