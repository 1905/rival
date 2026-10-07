//! Word check for review text against the Simplified Technical English word
//! lists in `data/ste.json`. It reports; it never edits a finding.
//!
//! A word is flagged when the not-approved list holds it and the approved
//! list does not. The check has no part-of-speech tagger, so a word that is
//! approved as one part of speech and not approved as another (for example
//! "check") is never flagged. Code spans, identifiers, paths and words with
//! digits or inner capitals are skipped. A word that the list also allows as
//! a technical noun ("file", "list", "cover") is skipped for the same reason.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use serde::Deserialize;

use super::prompt::WRITING_RULES;
use super::types::{ReviewerFinding, ReviewerOutput};

/// The word lists, compiled into the binary.
const DATA: &str = include_str!("../../data/ste.json");

/// One row: word, part of speech, and a list (inflections or replacements).
type Row = (String, String, Vec<String>);

#[derive(Deserialize)]
struct Lists {
    approved: Vec<Row>,
    not_approved: Vec<Row>,
}

struct Dict {
    approved: HashSet<String>,
    /// Lowercase word or phrase to its replacements.
    banned: HashMap<String, Vec<String>>,
    /// Token count of the longest banned phrase.
    longest: usize,
}

static DICT: LazyLock<Dict> = LazyLock::new(|| {
    let lists: Lists = serde_json::from_str(DATA).expect("data/ste.json is valid");
    let mut approved = HashSet::new();
    for (word, _pos, forms) in lists.approved {
        approved.insert(word);
        approved.extend(forms);
    }
    let mut banned: HashMap<String, Vec<String>> = HashMap::new();
    let mut longest = 1;
    for (word, _pos, alts) in lists.not_approved {
        // "cover -> COVER (TN)": the word is a legal technical noun and only
        // its verb use is banned. Without a tagger, leave it alone.
        let own_tn = alts.iter().any(|a| {
            a.strip_suffix(" (TN)")
                .or_else(|| a.strip_suffix(" (TV)"))
                .is_some_and(|w| w.eq_ignore_ascii_case(&word))
        });
        if own_tn {
            continue;
        }
        longest = longest.max(word.split(' ').count());
        let entry = banned.entry(word).or_default();
        for alt in alts {
            if !entry.contains(&alt) {
                entry.push(alt);
            }
        }
    }
    Dict {
        approved,
        banned,
        longest,
    }
});

/// A not-approved word or phrase found in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub word: String,
    pub count: usize,
    /// Replacements the list gives, as written there.
    pub use_instead: Vec<String>,
}

/// Splits `text` into lowercase word tokens, dropping code and identifiers.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Drop inline code spans: text between a pair of backticks.
        let plain: String = line
            .split('`')
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, s)| s)
            .collect::<Vec<_>>()
            .join(" ");
        let chars: Vec<char> = plain.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if !chars[i].is_alphabetic() {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let before = start.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i).copied();
            // A path, a method call or a scoped name is an identifier.
            let glued = matches!(before, Some('/' | '.' | ':' | '_' | '-' | '$' | '@'))
                || matches!(after, Some('/' | '_'))
                || (after == Some('.') && chars.get(i + 1).is_some_and(|c| c.is_alphanumeric()))
                || (after == Some(':') && chars.get(i + 1) == Some(&':'));
            let skip = glued
                || word.contains('_')
                || word.chars().any(|c| c.is_ascii_digit())
                || word.chars().skip(1).any(char::is_uppercase)
                || word.chars().count() < 2;
            // "don't": an apostrophe joins the next letters to this word.
            if after == Some('\'') && chars.get(i + 1).is_some_and(|c| c.is_alphabetic()) {
                while i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '\'') {
                    i += 1;
                }
                // Contractions are a separate rule. Keep a break in the text.
                out.push(String::new());
                continue;
            }
            // An empty token breaks a phrase, so skipped words never join.
            out.push(if skip {
                String::new()
            } else {
                word.to_lowercase()
            });
        }
        out.push(String::new());
    }
    out
}

/// Finds not-approved words and phrases in `text`, most frequent first.
pub fn check_text(text: &str) -> Vec<Hit> {
    let dict = &*DICT;
    let toks = tokens(text);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut i = 0;
    'outer: while i < toks.len() {
        if toks[i].is_empty() {
            i += 1;
            continue;
        }
        for n in (1..=dict.longest.min(toks.len() - i)).rev() {
            let window = &toks[i..i + n];
            if window.iter().any(String::is_empty) {
                continue;
            }
            let phrase = window.join(" ");
            if dict.banned.contains_key(&phrase) && !(n == 1 && dict.approved.contains(&phrase)) {
                *counts.entry(phrase).or_default() += 1;
                i += n;
                continue 'outer;
            }
        }
        i += 1;
    }
    let mut hits: Vec<Hit> = counts
        .into_iter()
        .map(|(word, count)| Hit {
            use_instead: dict.banned[&word].clone(),
            word,
            count,
        })
        .collect();
    hits.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    hits
}

/// Checks the free-text fields of one finding.
pub fn check_finding(f: &ReviewerFinding) -> Vec<Hit> {
    check_text(&format!(
        "{}\n{}\n{}\n{}",
        f.title, f.body, f.failure_scenario, f.suggestion
    ))
}

/// A review with fewer hits than this is left as it is.
pub const MIN_HITS: usize = 3;

/// Checks the summary and every finding of a review.
pub fn check_output(out: &ReviewerOutput) -> Vec<Hit> {
    let mut all = check_text(&out.summary);
    for f in &out.findings {
        all.extend(check_finding(f));
    }
    merge(all)
}

/// Adds up hits that name the same word.
fn merge(hits: Vec<Hit>) -> Vec<Hit> {
    let mut by_word: BTreeMap<String, Hit> = BTreeMap::new();
    for h in hits {
        by_word
            .entry(h.word.clone())
            .and_modify(|e| e.count += h.count)
            .or_insert(h);
    }
    let mut hits: Vec<Hit> = by_word.into_values().collect();
    hits.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    hits
}

/// The number of flagged words in `hits`, repeats included.
pub fn total(hits: &[Hit]) -> usize {
    hits.iter().map(|h| h.count).sum()
}

/// Builds the prompt for the second model call. It carries the review as
/// JSON and the flagged words with their replacements.
pub fn rewrite_prompt(out: &ReviewerOutput, hits: &[Hit]) -> String {
    let mut words = String::new();
    for h in hits.iter().take(30) {
        if h.use_instead.is_empty() {
            words.push_str(&format!("- {}: rephrase it\n", h.word));
        } else {
            words.push_str(&format!("- {} -> {}\n", h.word, h.use_instead.join(" / ")));
        }
    }
    let json = serde_json::to_string_pretty(out).expect("a review serializes");
    format!(
        "## Task: Rewrite review text\n\n\
Rewrite the text fields of the review JSON below so it follows the writing rules. \
This is an edit of wording only. Do not read code and do not run tools.\n\n\
Change only summary, title, body, failure_scenario and suggestion. \
Keep every other field, the number of findings and their order exactly as given. \
Keep every fact, number, identifier and path. Add no fact. \
Keep the strength of each hedge: \"may\" stays \"may\".\n\n\
These words are not approved. Replace each one, or rephrase the sentence:\n{words}\n\
Some words are code terms that have no replacement. Keep a word that is the exact name of a thing in the code.\n\n\
{WRITING_RULES}\
Return JSON only, with the same schema and keys as the input. No prose and no markdown.\n\n\
## Review\n\n{json}\n"
    )
}

/// The review as one line of JSON, the form the parser reads back.
pub fn to_json(out: &ReviewerOutput) -> String {
    serde_json::to_string(out).expect("a review serializes")
}

/// Reports whether `new` is a safe replacement for `old`: the same findings
/// with the same non-text fields, similar length, and fewer flagged words.
pub fn accept_rewrite(old: &ReviewerOutput, new: &ReviewerOutput) -> bool {
    if old.findings.len() != new.findings.len() {
        return false;
    }
    let same_shape = old.findings.iter().zip(&new.findings).all(|(a, b)| {
        a.file == b.file
            && a.line == b.line
            && a.severity == b.severity
            && a.category == b.category
            && a.confidence == b.confidence
            && keeps_text(&a.title, &b.title)
            && keeps_text(&a.body, &b.body)
            && keeps_text(&a.failure_scenario, &b.failure_scenario)
            && keeps_text(&a.suggestion, &b.suggestion)
    });
    same_shape
        && keeps_text(&old.summary, &new.summary)
        && total(&check_output(new)) < total(&check_output(old))
}

/// A text field stays empty if it was empty, stays non-empty if it was not,
/// and keeps between half and twice its length.
fn keeps_text(old: &str, new: &str) -> bool {
    let (a, b) = (old.trim().chars().count(), new.trim().chars().count());
    if a == 0 || b == 0 {
        return a == b;
    }
    b * 2 >= a && b <= a * 2
}

#[cfg(test)]
mod tests;
