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

/// Build version: `RIVAL_VERSION` at compile time, else `"dev"` like Go's
/// `var version = "dev"`. GoReleaser sets it to the tag without the `v`, as
/// the Go build's `-X main.version={{.Version}}` did.
pub const VERSION: &str = version_or_dev(option_env!("RIVAL_VERSION"));

/// An unset or empty `RIVAL_VERSION` is a development build.
const fn version_or_dev(version: Option<&'static str>) -> &'static str {
    match version {
        Some(version) if !version.is_empty() => version,
        _ => "dev",
    }
}

#[cfg(test)]
mod version_tests {
    use super::version_or_dev;

    #[test]
    fn version_or_dev_cases() {
        for (name, input, want) in [
            ("unset is dev", None, "dev"),
            ("empty is dev", Some(""), "dev"),
            ("release tag", Some("4.2.0"), "4.2.0"),
            (
                "snapshot",
                Some("4.1.1-SNAPSHOT-cb182c1"),
                "4.1.1-SNAPSHOT-cb182c1",
            ),
        ] {
            assert_eq!(version_or_dev(input), want, "{name}");
        }
    }
}
