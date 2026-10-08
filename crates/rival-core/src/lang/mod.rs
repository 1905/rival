//! A mechanical checker for controlled technical English, for the language
//! pass on review text.
//!
//! A port of the reference checker (`scripts/check` of the English-writing
//! skill, version 2.4.0). The embedded data is copied unchanged from that
//! skill:
//! - `data/lang/dictionary.json`, sha256
//!   92dfcec5d44fbeecc05837f5b0398740551dccbe2a5633269f85ee770191e700
//! - `data/lang/software.txt`, sha256
//!   c97c12aeb5b4bb11d896c2f68f5e583d9a08205872f3d72faaed02fdd6f0c356
//!
//! `Report::to_json` prints the same bytes as the reference checker's
//! `--json` output; the golden test in `tests.rs` checks it on the corpus
//! in `testdata/lang`.
//!
//! The repair pass ([`repair`]) is the only user.

mod check;
mod dict;
pub(crate) mod repair;
mod report;

use std::sync::LazyLock;

pub(crate) use report::Report;

use check::{Checker, Glossary};

/// How the sentence length limit is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Each sentence: procedural if it starts as an instruction, else
    /// descriptive.
    Auto,
    /// Every sentence has the limit of 20 words.
    Procedural,
    /// Every sentence has the limit of 25 words.
    Descriptive,
}

// The port tests read the reference checker's mode names.
#[cfg(test)]
impl Mode {
    /// The mode for its name: "auto", "procedural" or "descriptive".
    pub(crate) fn parse(name: &str) -> Option<Mode> {
        match name {
            "auto" => Some(Mode::Auto),
            "procedural" => Some(Mode::Procedural),
            "descriptive" => Some(Mode::Descriptive),
            _ => None,
        }
    }
}

static SOFTWARE: LazyLock<Glossary> =
    LazyLock::new(|| Glossary::new(dict::SOFTWARE_GLOSSARY.split('\n')));

/// Checks `text` in auto sentence mode with the software glossary.
pub(crate) fn check(text: &str) -> Report {
    check_mode(text, Mode::Auto)
}

/// Checks `text` with the software glossary. CR LF and CR become LF first,
/// as the reference checker does when it reads a file.
pub(crate) fn check_mode(text: &str, mode: Mode) -> Report {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut c = Checker::new(dict::dict(), mode, &SOFTWARE);
    c.check(&text);
    c.into_report()
}

#[cfg(test)]
mod tests;
