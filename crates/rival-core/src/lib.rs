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
pub mod review;
pub mod session;

/// Build version. Mirrors Go's `var version = "dev"`; release builds override it later.
pub const VERSION: &str = "dev";

/// Returns the `rival version` line.
pub fn version_line() -> String {
    format!("rival {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_is_placeholder() {
        assert_eq!(version_line(), "rival dev");
    }
}
