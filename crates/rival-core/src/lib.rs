//! Core library for the rival CLI.

pub mod cancel;
pub mod config;
pub mod duration;
mod envname;
pub mod executor;
pub mod gitscope;
pub mod json;
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

/// The `io::Error` texts of the OS errors that tests provoke. Windows
/// prints the English system message.
#[cfg(test)]
pub(crate) mod errtext {
    /// A missing file in an existing directory.
    pub const NO_SUCH_FILE: &str = if cfg!(windows) {
        "The system cannot find the file specified. (os error 2)"
    } else {
        "No such file or directory (os error 2)"
    };

    /// A missing parent directory (`ERROR_PATH_NOT_FOUND` on Windows).
    pub const NO_SUCH_PATH: &str = if cfg!(windows) {
        "The system cannot find the path specified. (os error 3)"
    } else {
        "No such file or directory (os error 2)"
    };

    /// The failed step and its text when a directory is read as a file. On
    /// Unix the open works and the read fails. On Windows the open fails.
    pub const DIR_AS_FILE: (&str, &str) = if cfg!(windows) {
        ("open", "Access is denied. (os error 5)")
    } else {
        ("read", "Is a directory (os error 21)")
    };
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
