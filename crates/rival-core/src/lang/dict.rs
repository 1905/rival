//! The embedded dictionary: approved words, words that are not approved,
//! and the index of every permitted surface form.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use super::check::{WS, has_any_suffix, lower, rune_len, word_set};

const DICTIONARY_JSON: &str = include_str!("../../data/lang/dictionary.json");

/// The built-in software glossary, loaded on every `check`.
pub(crate) const SOFTWARE_GLOSSARY: &str = include_str!("../../data/lang/software.txt");

/// An approved word. Unknown fields (`*_raw`, `*_table`) are ignored;
/// `examples` can be empty.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ApprovedEntry {
    pub(crate) word: String,
    pub(crate) pos: String,
    pub(crate) meaning: Vec<String>,
    pub(crate) examples: Vec<String>,
    #[serde(rename = "for_other_meanings_use")]
    pub(crate) for_other: String,
    pub(crate) forms: String,
    pub(crate) note: String,
}

/// A word that is not approved, with its approved alternatives.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct NotApprovedEntry {
    pub(crate) word: String,
    pub(crate) pos: String,
    pub(crate) use_instead: Vec<String>,
    pub(crate) example: Vec<String>,
    #[serde(rename = "incorrect_example")]
    pub(crate) wrong_example: Vec<String>,
    pub(crate) note: String,
}

/// The approved and non-approved word lists and the surface-form index.
#[derive(Debug, Default)]
pub(crate) struct Dict {
    pub(crate) source: String,
    /// Lower-case word -> its entries, one per part of speech.
    pub(crate) approved: HashMap<String, Vec<ApprovedEntry>>,
    /// Lower-case word -> its entries. "few (a few)" is under "few" and
    /// under "few (a few)".
    pub(crate) non_approved: HashMap<String, Vec<NotApprovedEntry>>,
    /// Every permitted surface form -> its approved base word.
    pub(crate) forms: HashMap<String, String>,
}

static DICT: LazyLock<Dict> =
    LazyLock::new(|| parse_dict(DICTIONARY_JSON).expect("embedded dictionary.json is valid"));

/// The embedded dictionary, parsed once.
pub(crate) fn dict() -> &'static Dict {
    &DICT
}

static WORD_PAREN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{WS}*\(.*?\){WS}*")).unwrap());
static FORM_TOK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z][a-z'\-]*").unwrap());

static FORM_STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    word_set(
        "no other verb forms also for example use this word only as an adjective or the a in of not to do with
	that is it you and when but form same plural noun singular adverb these words before after used meaning meanings sense context refer",
    )
});

pub(crate) fn parse_dict(data: &str) -> Result<Dict, serde_json::Error> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        source: serde_json::Value,
        #[serde(default)]
        approved: Vec<ApprovedEntry>,
        #[serde(default)]
        non_approved: Vec<NotApprovedEntry>,
    }
    let raw: Raw = serde_json::from_str(data)?;
    let mut d = Dict {
        source: match raw.source {
            serde_json::Value::String(s) => s,
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        },
        ..Dict::default()
    };
    let mut order: Vec<String> = Vec::new();
    for e in raw.approved {
        let k = lower(&e.word);
        let list = d.approved.entry(k.clone()).or_default();
        if list.is_empty() {
            order.push(k);
        }
        list.push(e);
    }
    for e in raw.non_approved {
        let full = lower(&e.word);
        // "few (a few)" -> "few"
        let w = WORD_PAREN_RE.replace_all(&full, " ").trim().to_string();
        if w != full {
            d.non_approved.entry(full).or_default().push(e.clone());
        }
        d.non_approved.entry(w).or_default().push(e);
    }
    fn setdef(forms: &mut HashMap<String, String>, k: String, v: &str) {
        forms.entry(k).or_insert_with(|| v.to_string());
    }
    for base in &order {
        for e in &d.approved[base] {
            setdef(&mut d.forms, base.clone(), base);
            let text = lower(&format!("{} {}", e.forms, e.note));
            for tok in FORM_TOK_RE.find_iter(&text) {
                let tok = tok.as_str();
                if !FORM_STOP.contains(tok) && rune_len(tok) > 1 {
                    setdef(&mut d.forms, tok.to_string(), base);
                }
            }
            if e.pos == "n" {
                for p in plural_forms(base) {
                    setdef(&mut d.forms, p, base);
                }
            }
            if e.pos == "v" && e.forms.is_empty() {
                setdef(&mut d.forms, format!("{base}s"), base);
                setdef(&mut d.forms, format!("{base}ed"), base);
            }
        }
    }
    for f in ["is", "was", "are", "were", "be"] {
        d.forms.insert(f.to_string(), "be".to_string());
    }
    setdef(&mut d.forms, "makes sure".to_string(), "make sure");
    setdef(&mut d.forms, "made sure".to_string(), "make sure");
    Ok(d)
}

/// The plural forms of a noun: `+s`, and `+es`, `-y +ies`, `-f(e) +ves`
/// where the ending fits.
pub(crate) fn plural_forms(w: &str) -> Vec<String> {
    let mut out = vec![format!("{w}s")];
    if has_any_suffix(w, &["s", "x", "z", "ch", "sh"]) {
        out.push(format!("{w}es"));
    }
    let rs: Vec<char> = w.chars().collect();
    if rs.len() >= 2 && rs[rs.len() - 1] == 'y' && !"aeiou".contains(rs[rs.len() - 2]) {
        out.push(format!("{}ies", &w[..w.len() - 1]));
    }
    if let Some(stem) = w.strip_suffix('f') {
        out.push(format!("{stem}ves"));
    }
    if let Some(stem) = w.strip_suffix("fe") {
        out.push(format!("{stem}ves"));
    }
    out
}

/// A suffix to remove, and the text to put back, to find a base word.
pub(crate) struct SuffixRule {
    pub(crate) suf: &'static str,
    pub(crate) repl: &'static str,
}

pub(crate) const DEINFLECT: [SuffixRule; 10] = [
    SuffixRule {
        suf: "ies",
        repl: "y",
    },
    SuffixRule {
        suf: "es",
        repl: "",
    },
    SuffixRule { suf: "s", repl: "" },
    SuffixRule {
        suf: "ed",
        repl: "",
    },
    SuffixRule {
        suf: "ed",
        repl: "e",
    },
    SuffixRule {
        suf: "ing",
        repl: "",
    },
    SuffixRule {
        suf: "ing",
        repl: "e",
    },
    SuffixRule {
        suf: "er",
        repl: "",
    },
    SuffixRule {
        suf: "est",
        repl: "",
    },
    SuffixRule {
        suf: "ly",
        repl: "",
    },
];

/// Approved rewrites that keep the strength of a hedged claim. Every
/// capitalized word is approved (a test checks it).
pub(crate) const HEDGE_HINTS: [(&str, &str); 8] = [
    (
        "may",
        r#"POSSIBLY, or "IT IS POSSIBLE THAT ..." (CAN only for ability or permission). Keep it a possibility."#,
    ),
    (
        "might",
        r#"POSSIBLY, or "IT IS POSSIBLE THAT ...". Keep it a possibility."#,
    ),
    (
        "perhaps",
        r#"POSSIBLY, or "IT IS POSSIBLE THAT ...". Keep it a possibility."#,
    ),
    (
        "likely",
        r#""IT IS VERY POSSIBLE THAT ...". Keep it a possibility, not a fact."#,
    ),
    (
        "probably",
        r#""IT IS VERY POSSIBLE THAT ...". Keep it a possibility, not a fact."#,
    ),
    (
        "probable",
        r#""IT IS VERY POSSIBLE THAT ...", or "A VERY POSSIBLE RISK" for a danger. Keep it a possibility, not a fact."#,
    ),
    (
        "unlikely",
        r#"JUDGE: "IT IS NOT VERY POSSIBLE THAT ...", or "THE RISK IS SMALL" for a danger. Do not write that it cannot occur."#,
    ),
    (
        "should",
        r#"JUDGE: advice -> "WE RECOMMEND THAT ..."; obligation -> MUST. Do not change advice into an order by accident."#,
    ),
];

/// The hedge hint of a word, or "".
pub(crate) fn hedge_hint(w: &str) -> &'static str {
    HEDGE_HINTS
        .iter()
        .find(|(k, _)| *k == w)
        .map_or("", |(_, h)| h)
}

/// The dictionary entries of each word, as text. Examples are printed only
/// when the dictionary has them. The repair prompt uses this.
pub(crate) fn lookup(d: &Dict, words: &[&str]) -> String {
    let mut out = String::new();
    lookup_into(d, words, &mut out);
    out
}

fn lookup_into(d: &Dict, words: &[&str], out: &mut String) {
    for w in words {
        let w = lower(w.trim());
        let base = d.forms.get(&w).cloned().unwrap_or_else(|| w.clone());
        let _ = writeln!(out, "== {w} ==");
        let mut hit = false;
        for e in d.approved.get(&base).into_iter().flatten() {
            hit = true;
            let _ = writeln!(
                out,
                "  APPROVED  {} ({}): {}",
                e.word,
                e.pos,
                e.meaning.join("; ")
            );
            for (name, value) in [
                ("forms", &e.forms),
                ("note", &e.note),
                ("for other meanings use", &e.for_other),
            ] {
                if !value.is_empty() {
                    let _ = writeln!(out, "    {name}: {value}");
                }
            }
            for ex in e.examples.iter().take(2) {
                let _ = writeln!(out, "    e.g. {ex}");
            }
        }
        let mut na: Vec<&NotApprovedEntry> = d.non_approved.get(&w).into_iter().flatten().collect();
        if base != w {
            na.extend(d.non_approved.get(&base).into_iter().flatten());
        }
        for e in na {
            hit = true;
            let _ = writeln!(
                out,
                "  NOT APPROVED  {} ({})  ->  {} {}",
                e.word,
                e.pos,
                e.use_instead.join(", "),
                e.note
            );
            let limit = if e.wrong_example.is_empty() {
                3
            } else {
                e.wrong_example.len()
            };
            for (k, ex) in e.example.iter().take(limit).enumerate() {
                let _ = writeln!(out, "    Example: {ex}");
                if let Some(wrong) = e.wrong_example.get(k).filter(|x| !x.is_empty()) {
                    let _ = writeln!(out, "    not: {wrong}");
                }
            }
        }
        // "removing" -> see "remove"; "thing" is too short to be "the"+"ing"
        for r in &DEINFLECT[..7] {
            if !hit && w.ends_with(r.suf) && rune_len(&w) > r.suf.len() + 2 {
                let b = format!("{}{}", &w[..w.len() - r.suf.len()], r.repl);
                if d.approved.contains_key(&b) || d.non_approved.contains_key(&b) {
                    let _ = writeln!(out, "  (see \"{b}\")");
                    lookup_into(d, &[&b], out);
                    hit = true;
                }
            }
        }
        if !hit {
            out.push_str("  Not in the dictionary. Permitted only as a technical noun (Rule 1.5) or technical verb (Rule 1.12), otherwise replace it.\n");
        }
        let hint = hedge_hint(&w);
        if !hint.is_empty() {
            let _ = writeln!(out, "  keep the strength: {hint}");
        }
        out.push('\n');
    }
}
