//! The result of a check: findings, word hits and the auto-glossary, as
//! text for the repair prompt and as the reference checker's JSON.

#[cfg(test)]
use std::fmt::Write as _;

/// One structure or grammar finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) rule: String,
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) r#where: String,
    /// For sorting.
    pub(crate) para: usize,
    /// For sorting; 0 for a paragraph finding.
    pub(crate) sent: usize,
}

/// One word that is not approved or not in the dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WordHit {
    /// The base word: the key in the JSON "words" object.
    pub(crate) word: String,
    /// "non_approved", "maybe_noun" or "unknown".
    pub(crate) kind: String,
    pub(crate) alts: String,
    pub(crate) count: usize,
    /// The first sentence with the word, with its `code` spans.
    pub(crate) example: String,
    pub(crate) pos: String,
    /// The hedge hint, or "".
    pub(crate) hint: String,
}

/// One token accepted by the auto-glossary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoHit {
    pub(crate) term: String,
    pub(crate) kind: String,
    pub(crate) count: usize,
}

/// The result of a check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// In the order the checker found them.
    pub(crate) findings: Vec<Finding>,
    /// In first-seen order.
    pub(crate) words: Vec<WordHit>,
    /// In first-seen order.
    pub(crate) auto: Vec<AutoHit>,
}

/// The order of finding kinds in the text report.
fn kind_order(kind: &str) -> usize {
    match kind {
        "length" => 0,
        "paragraph" => 1,
        "tense" => 2,
        "passive" => 3,
        "ing" => 4,
        "punctuation" => 5,
        "contraction" => 6,
        "noun-cluster" => 7,
        "latin" => 8,
        "pronoun" => 9,
        _ => 0,
    }
}

const RULE: &str = "------------------------------------------------------------";

impl Report {
    /// The word hit for a base word.
    #[cfg(test)]
    pub(crate) fn word(&self, key: &str) -> Option<&WordHit> {
        self.words.iter().find(|w| w.word == key)
    }

    /// The findings that are definite. A rule that ends in "?" ("3.5?",
    /// "3.6?") asks the writer to confirm, so it is not hard.
    pub(crate) fn hard(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| !f.rule.ends_with('?'))
            .count()
    }

    /// The word hits of one kind, most frequent first (stable).
    fn words_of_kind(&self, kind: &str) -> Vec<&WordHit> {
        let mut out: Vec<&WordHit> = self.words.iter().filter(|w| w.kind == kind).collect();
        out.sort_by_key(|w| std::cmp::Reverse(w.count));
        out
    }

    /// The text report for the repair prompt. It has the sections of the
    /// reference checker's text report. Each word hit is one line with its
    /// alternatives and one example sentence.
    pub(crate) fn render(&self) -> String {
        let na = self.words_of_kind("non_approved");
        let maybe = self.words_of_kind("maybe_noun");
        let unk = self.words_of_kind("unknown");
        let mut out = vec![
            "DICTIONARY CHECK".to_string(),
            "=".repeat(60),
            format!(
                "Structure/grammar findings: {}   Non-approved words: {}   Not approved as a verb: {}   Words not in dictionary: {}",
                self.findings.len(),
                na.len(),
                maybe.len(),
                unk.len()
            ),
            String::new(),
        ];
        let mut section = |title: &str, lines: Vec<String>| {
            if !lines.is_empty() {
                out.push(title.to_string());
                out.push(RULE.to_string());
                out.extend(lines);
                out.push(String::new());
            }
        };
        section(
            "NON-APPROVED WORDS (Rules 1.1–1.3, 9.1) — replace or restructure",
            na.iter()
                .map(|h| {
                    format!(
                        "  {} ({}) x{}  ->  {}   example: \"{}\"",
                        h.word, h.pos, h.count, h.alts, h.example
                    )
                })
                .collect(),
        );
        section(
            "NOT APPROVED AS A VERB — fine if used as a noun / technical noun here; otherwise replace",
            maybe
                .iter()
                .map(|h| {
                    format!(
                        "  {} x{}  ->  as a verb use: {}   example: \"{}\"",
                        h.word, h.count, h.alts, h.example
                    )
                })
                .collect(),
        );
        section(
            "WORDS NOT IN THE DICTIONARY — each must be a technical noun/verb (Rules 1.5, 1.12) or be replaced",
            unk.iter()
                .map(|h| format!("  {} x{}   example: \"{}\"", h.word, h.count, h.example))
                .collect(),
        );
        section(
            "HEDGE WORDS — keep the strength of the claim; never change a possibility into a fact",
            na.iter()
                .chain(&maybe)
                .chain(&unk)
                .filter(|h| !h.hint.is_empty())
                .map(|h| format!("  {} x{}  ->  {}", h.word, h.count, h.hint))
                .collect(),
        );
        section(
            "AUTO-GLOSSARY — code-like tokens accepted as technical nouns (Rules 1.5, 8.6); check the list",
            self.auto
                .iter()
                .map(|a| format!("  {} ({}) x{}", a.term, a.kind, a.count))
                .collect(),
        );
        let mut f: Vec<&Finding> = self.findings.iter().collect();
        f.sort_by_key(|x| (kind_order(&x.kind), x.para, x.sent));
        section(
            "STRUCTURE AND GRAMMAR",
            f.iter()
                .map(|x| format!("  [{}] {}   ({})", x.rule, x.message, x.r#where))
                .collect(),
        );
        if self.findings.len() + na.len() + maybe.len() + unk.len() == 0 {
            out.push("No findings. The text passes the mechanical checks. Still read it against the rules that no script can check (one topic per sentence, condition first, notes vs. instructions, approved meaning of each word).".to_string());
        }
        out.join("\n")
    }

    /// Exactly what the reference checker prints with `--json`: indent of
    /// one space, keys in its order, no HTML escaping, and the final
    /// newline. The golden test compares it with the reference output.
    #[cfg(test)]
    pub(crate) fn to_json(&self) -> String {
        let mut b = String::from("{\n \"findings\": ");
        if self.findings.is_empty() {
            b.push_str("[]");
        } else {
            b.push('[');
            for (i, f) in self.findings.iter().enumerate() {
                b.push_str(if i == 0 { "\n  {" } else { ",\n  {" });
                field(&mut b, "rule", &f.rule, true);
                field(&mut b, "kind", &f.kind, false);
                field(&mut b, "message", &f.message, false);
                field(&mut b, "where", &f.r#where, false);
                b.push_str("\n  }");
            }
            b.push_str("\n ]");
        }
        b.push_str(",\n \"words\": ");
        if self.words.is_empty() {
            b.push_str("{}");
        } else {
            b.push('{');
            for (i, w) in self.words.iter().enumerate() {
                b.push_str(if i == 0 { "\n  " } else { ",\n  " });
                quote(&mut b, &w.word);
                b.push_str(": {");
                field(&mut b, "kind", &w.kind, true);
                field(&mut b, "alts", &w.alts, false);
                let _ = write!(b, ",\n   \"count\": {}", w.count);
                field(&mut b, "example", &w.example, false);
                field(&mut b, "pos", &w.pos, false);
                if !w.hint.is_empty() {
                    field(&mut b, "hint", &w.hint, false);
                }
                b.push_str("\n  }");
            }
            b.push_str("\n }");
        }
        b.push_str(",\n \"auto_glossary\": ");
        if self.auto.is_empty() {
            b.push_str("[]");
        } else {
            b.push('[');
            for (i, a) in self.auto.iter().enumerate() {
                b.push_str(if i == 0 { "\n  {" } else { ",\n  {" });
                field(&mut b, "term", &a.term, true);
                field(&mut b, "kind", &a.kind, false);
                let _ = write!(b, ",\n   \"count\": {}", a.count);
                b.push_str("\n  }");
            }
            b.push_str("\n ]");
        }
        b.push_str("\n}\n");
        b
    }
}

/// One string field of an object at depth 2.
#[cfg(test)]
fn field(b: &mut String, key: &str, value: &str, first: bool) {
    b.push_str(if first { "\n   " } else { ",\n   " });
    quote(b, key);
    b.push_str(": ");
    quote(b, value);
}

/// A JSON string as the reference encoder writes it without HTML escaping:
/// short escapes for `\b \f \n \r \t`, `\u00XX` for other control
/// characters, and ` `, ` ` escaped.
#[cfg(test)]
fn quote(b: &mut String, s: &str) {
    b.push('"');
    for c in s.chars() {
        match c {
            '"' => b.push_str("\\\""),
            '\\' => b.push_str("\\\\"),
            '\u{8}' => b.push_str("\\b"),
            '\u{c}' => b.push_str("\\f"),
            '\n' => b.push_str("\\n"),
            '\r' => b.push_str("\\r"),
            '\t' => b.push_str("\\t"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(b, "\\u{:04x}", c as u32);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(b, "\\u{:04x}", c as u32);
            }
            c => b.push(c),
        }
    }
    b.push('"');
}
