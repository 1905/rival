//! Core library for the rival CLI.

pub mod cancel;
pub mod config;
pub mod executor;
pub mod gitscope;
pub mod gojson;
pub mod gostd;
pub mod logfmt;
pub mod logging;
pub mod mergerequest;
pub mod parser;
pub mod paths;
pub mod procinfo;
pub mod queue;
pub mod result;
pub mod review;
pub mod session;
pub mod sessionview;
pub mod skills;
pub mod telemetry;
pub mod update;
pub mod winpath;

/// Build version. Mirrors Go's `var version = "dev"`; release builds override it later.
pub const VERSION: &str = "dev";
