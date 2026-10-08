//! Tests ported one to one from the reference checker's test file, plus a
//! golden test against the reference output for the corpus in
//! `testdata/lang`.

use std::collections::HashMap;

use super::check::{
    Checker, Glossary, LIST_MARKER_RE, classify_code, contractions, count_words, ing_form,
    is_title_case, is_upper_word, restore_code, split_punct, split_sentences, strip_code,
};
use super::dict::{HEDGE_HINTS, dict, lookup, parse_dict};
use super::report::Report;
use super::{Mode, check, check_mode};

/// The reference tests run without the built-in software glossary.
fn run(glossary: &[&str], text: &str) -> Report {
    let g = Glossary::new(glossary.iter().copied());
    let mut c = Checker::new(dict(), Mode::Auto, &g);
    c.check(text);
    c.into_report()
}

fn kinds(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .map(|f| format!("{} {}: {}", f.rule, f.kind, f.message))
        .collect()
}

fn has_finding(r: &Report, rule: &str) -> bool {
    r.findings.iter().any(|f| f.rule == rule)
}

fn findings_of(r: &Report, rule: &str) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| f.rule == rule)
        .map(|f| f.message.clone())
        .collect()
}

/// A word-hit case: name, glossary, text, expected (word, kind) pairs.
type WordCase<'a> = (&'a str, &'a [&'a str], &'a str, &'a [(&'a str, &'a str)]);

/// A lookup case: name or word, words, wanted and unwanted substrings.
type LookupCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

fn word_keys(r: &Report) -> Vec<&str> {
    r.words.iter().map(|w| w.word.as_str()).collect()
}

#[test]
fn lang_lookup_base() {
    let g = Glossary::new(["breaker", "flux capacitor", "# comment", ""]);
    let c = Checker::new(dict(), Mode::Auto, &g);
    for (name, word, kind, base) in [
        ("approved base", "remove", "approved", "remove"),
        ("approved listed form", "removed", "approved", "remove"),
        ("approved plural", "tools", "approved", "tool"),
        ("irregular be", "were", "approved", "be"),
        (
            "inflected by suffix",
            "removing",
            "approved-inflected",
            "remove",
        ),
        ("non-approved", "ensure", "non_approved", "ensure"),
        (
            "non-approved inflected",
            "ensures",
            "non_approved",
            "ensure",
        ),
        ("unknown", "xyzzy", "unknown", "xyzzy"),
        ("glossary", "breaker", "glossary", "breaker"),
        ("glossary plural", "breakers", "glossary", "breakers"),
        ("multi-word approved", "make sure", "approved", "make sure"),
        ("multi-word inflected", "made sure", "approved", "make sure"),
    ] {
        assert_eq!(c.lookup_base(word), (kind, base.to_string()), "{name}");
    }
    assert_eq!(
        g.multi,
        vec!["flux capacitors".to_string(), "flux capacitor".to_string()],
        "longest first"
    );
}

#[test]
fn lang_word_hits() {
    let cases: &[WordCase] = &[
        (
            "non-approved and unknown",
            &[],
            "Ensure that the xyzzy is clean.",
            &[("ensure", "non_approved"), ("xyzzy", "unknown")],
        ),
        ("approved only", &[], "Remove the cover.", &[]),
        (
            "verb used as noun after determiner",
            &[],
            "Do the test.",
            &[],
        ),
        ("glossary word", &["xyzzy"], "Clean the xyzzy.", &[]),
        (
            "multi-word glossary",
            &["flux capacitor"],
            "Clean the flux capacitor.",
            &[],
        ),
        (
            "multi-word without glossary",
            &[],
            "Clean the flux capacitor.",
            &[("flux", "unknown"), ("capacitor", "unknown")],
        ),
        (
            "proper noun mid-sentence is skipped",
            &[],
            "Connect the Zorbatron.",
            &[],
        ),
        (
            "all-caps abbreviation is skipped",
            &[],
            "Connect the ZQX unit.",
            &[],
        ),
    ];
    for (name, glossary, text, want) in cases {
        let r = run(glossary, text);
        let got: HashMap<&str, &str> = r
            .words
            .iter()
            .map(|w| (w.word.as_str(), w.kind.as_str()))
            .collect();
        let want: HashMap<&str, &str> = want.iter().copied().collect();
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn lang_count_words() {
    for (rule, sentence, want) in [
        ("plain", "Remove the cover.", 3),
        ("8.4 step number not counted", "1. Remove the cover.", 3),
        ("8.4 list bullet not counted", "- Remove the cover.", 3),
        (
            "8.4 safety label not counted",
            "WARNING: Remove the cover.",
            3,
        ),
        (
            "8.5 parenthesis is one word",
            "Remove the cover (see the note below).",
            4,
        ),
        ("8.6 number and unit", "Drill a 10 mm hole.", 4),
        ("8.6 number and unit, no space", "Drill a 10mm hole.", 4),
        ("8.6 temperature", "Heat it to 25 °C.", 4),
        ("8.6 percent then word", "Fill 50% of it.", 4),
        ("8.6 identifier", "Install the A320-200 unit.", 4),
        ("8.6 abbreviation", "Connect the AB/CD-12 cable.", 4),
        ("8.6 quoted text", "Set the switch to \"SYSTEM ON NOW\".", 5),
        ("8.6 curly quoted text", "Push “START TEST NOW”.", 2),
        ("8.6 decimal number", "Torque to 2.5 psi.", 3),
        ("8.6 units are lower case only", "Torque to 2.5 kPa.", 4),
        ("8.6 ohm sign", "Set it to 5 Ω now.", 5),
        ("8.7 hyphenated word", "Do a self-test.", 3),
        (
            "possessive of abbreviation counts twice",
            "Read ABC's value.",
            4,
        ),
        ("Unicode letters", "Café naïve résumé.", 3),
    ] {
        assert_eq!(count_words(sentence), want, "{rule}: {sentence:?}");
    }
}

#[test]
fn lang_split_sentences() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "two sentences",
            "Remove the cover. Clean it.",
            &["Remove the cover.", "Clean it."],
        ),
        (
            "lower case after a period does not split",
            "Use it. then go.",
            &["Use it. then go."],
        ),
        (
            "protected abbreviation",
            "Use a tool, e.g. A wrench.",
            &["Use a tool, e.g. A wrench."],
        ),
        (
            "protected abbreviation in parenthesis",
            "Use a tool (e.g. A wrench).",
            &["Use a tool (e.g. A wrench)."],
        ),
        (
            "protected abbreviation at start",
            "No. 5 is open.",
            &["No. 5 is open."],
        ),
        (
            "decimal number",
            "Set 1.5 V. Then stop.",
            &["Set 1.5 V.", "Then stop."],
        ),
        (
            "colon ends a list intro (8.4)",
            "Do these steps: Remove the cover.",
            &["Do these steps:", "Remove the cover."],
        ),
        (
            "question and exclamation",
            "Is it on? Stop it! Run.",
            &["Is it on?", "Stop it!", "Run."],
        ),
        (
            "opening quote or parenthesis",
            "Stop. \"Start\" it. (Note) here.",
            &["Stop.", "\"Start\" it.", "(Note) here."],
        ),
        (
            "number after period",
            "Do step 3. 4 bolts remain.",
            &["Do step 3.", "4 bolts remain."],
        ),
        ("empty", "   ", &[]),
    ];
    for (name, block, want) in cases {
        assert_eq!(split_sentences(block), *want, "{name}");
    }
}

#[test]
fn lang_sentence_findings() {
    for (name, text, want) in [
        ("-ing verb form", "When you are doing it, stop.", "3.5"),
        (
            "-ing word that may be a noun",
            "Examine the wiring diagram.",
            "3.5?",
        ),
        ("approved -ing form", "Do the servicing.", ""),
        ("perfect tense", "The pump has failed.", "3.4"),
        ("been", "It has been done.", "3.4"),
        (
            "passive with agent",
            "The valve is removed by the operator.",
            "3.6",
        ),
        ("passive or adjective", "The valve is removed.", "3.6?"),
        ("semicolon", "Stop the pump; open the valve.", "8.1"),
        ("contraction n't", "Don't open the valve.", "4.2"),
        ("contraction it's", "It's the valve.", "4.2"),
        ("possessive is permitted", "Open the operator's valve.", ""),
        ("Latin e.g.", "Use a tool, e.g. a wrench.", "GR-6"),
        ("Latin etc", "Use a tool, a wrench etc.", "GR-6"),
        ("gendered pronoun", "He opens the valve.", "GR-7"),
        ("gendered pronoun, other case", "Give it to HER.", "GR-7"),
        (
            "noun cluster",
            "Examine the fuel pump pressure sensor connector.",
            "2.1",
        ),
        (
            "procedural length",
            "Remove the cover and the filter and the tool and the unit and the cover and the filter and the tool and the unit.",
            "5.1",
        ),
        (
            "descriptive length",
            "The pump is a unit that is in the system and it is in the area of the left wing and it is near the area of the right wing too.",
            "6.3",
        ),
    ] {
        let r = run(&[], text);
        if want.is_empty() {
            assert!(
                r.findings.is_empty(),
                "{name}: want no findings, got {:?}",
                kinds(&r)
            );
        } else {
            assert!(
                has_finding(&r, want),
                "{name}: want rule {want}, got {:?}",
                kinds(&r)
            );
        }
    }
}

#[test]
fn lang_paragraph_length() {
    for (name, text, want) in [
        (
            "seven sentences",
            "Remove it. Clean it. Install it. Examine it. Measure it. Tighten it. Record it.",
            true,
        ),
        (
            "seven short sentences",
            "One. Two. Three. Four. Five. Six. Seven.",
            true,
        ),
        (
            "six sentences",
            "Remove it. Clean it. Install it. Examine it. Measure it. Tighten it.",
            false,
        ),
        (
            "list items are not a paragraph",
            "1. One.\n2. Two.\n3. Three.\n4. Four.\n5. Five.\n6. Six.\n7. Seven.",
            false,
        ),
    ] {
        assert_eq!(has_finding(&run(&[], text), "6.6"), want, "{name}");
    }
}

#[test]
fn lang_headings_skipped() {
    let r = run(
        &[],
        "# Remove Doing Ensure\n\nInstallation Of The Unit\n\nRemove the cover.",
    );
    assert!(
        r.findings.is_empty() && r.words.is_empty(),
        "headings must be skipped, got findings {:?} words {:?}",
        kinds(&r),
        word_keys(&r)
    );
}

#[test]
fn lang_lookup_output() {
    let cases: &[LookupCase] = &[
        (
            "approved word",
            &["remove"],
            &["== remove ==", "  APPROVED  REMOVE (v):"],
            &[],
        ),
        (
            "non-approved word",
            &["ensure"],
            &["  NOT APPROVED  ensure (v)  ->  ", "    Example: "],
            &[],
        ),
        (
            "inflected form is resolved",
            &["ensures"],
            &["  (see \"ensure\")", "== ensure =="],
            &[],
        ),
        (
            "unknown word",
            &["xyzzy"],
            &["  Not in the dictionary."],
            &[],
        ),
        (
            "several words, case and spaces",
            &["  MAY ", "valve"],
            &["== may ==", "== valve =="],
            &[],
        ),
        ("one word", &["may"], &["== may =="], &[]),
        (
            "hedge hint",
            &["may"],
            &["  keep the strength: POSSIBLY"],
            &[],
        ),
        (
            "hedge hint for an unknown word",
            &["probably"],
            &["Not in the dictionary", "IT IS VERY POSSIBLE THAT"],
            &[],
        ),
        (
            "short word is not de-inflected",
            &["thing"],
            &["Not in the dictionary"],
            &["(see \"the\")"],
        ),
    ];
    for (name, words, want, not_want) in cases {
        let out = lookup(dict(), words);
        for w in *want {
            assert!(out.contains(w), "{name}: output lacks {w:?}:\n{out}");
        }
        for w in *not_want {
            assert!(!out.contains(w), "{name}: output has {w:?}:\n{out}");
        }
    }
}

/// The real dictionary has extra fields and empty approved examples. The
/// loader ignores unknown fields, and the lookup prints no example lines
/// when there are none.
#[test]
fn lang_dictionary_shape() {
    let d = parse_dict(
        r#"{
        "source": "x", "approved_table": {"preamble": "p"},
        "approved": [{"word": "VALVE", "pos": "n", "meaning": ["A device"], "forms": "", "examples": [], "meaning_raw": "A device", "notes_raw": ""},
                     {"word": "OPEN", "pos": "v", "meaning": ["To move"], "forms": "OPENS, OPENED", "examples": ["OPEN THE VALVE."]}],
        "non_approved": [{"word": "ensure", "pos": "v", "use_instead": ["MAKE SURE (v)"], "good_example": [], "bad_example": [], "use_instead_raw": "MAKE SURE (v)"}]}"#,
    )
    .unwrap();
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "valve",
            &["APPROVED  VALVE (n): A device"],
            &["e.g.", "forms:"],
        ),
        (
            "opened",
            &[
                "APPROVED  OPEN (v)",
                "forms: OPENS, OPENED",
                "e.g. OPEN THE VALVE.",
            ],
            &[],
        ),
        (
            "ensure",
            &["NOT APPROVED  ensure (v)  ->  MAKE SURE (v)"],
            &["Example:", "not:"],
        ),
    ];
    for (word, want, not_want) in cases {
        let out = lookup(&d, &[word]);
        for w in *want {
            assert!(out.contains(w), "{word}: output lacks {w:?}:\n{out}");
        }
        for w in *not_want {
            assert!(!out.contains(w), "{word}: output has {w:?}:\n{out}");
        }
    }
    // plural form index
    assert_eq!(d.forms.get("valves").map(String::as_str), Some("valve"));
    assert_eq!(d.forms.get("opened").map(String::as_str), Some("open"));
    // embedded dictionary loads
    let d = dict();
    assert!(
        d.approved.len() >= 800 && d.non_approved.len() >= 1000,
        "approved {}, non-approved {}",
        d.approved.len(),
        d.non_approved.len()
    );
    assert!(!d.source.is_empty());
}

#[test]
fn lang_text_helpers() {
    for (input, want) in [
        ("Hello World", true),
        ("Hello world", false),
        ("Don't", false),
        ("1st", false),
        ("Installation Of The Unit", true),
        ("CODESPANAAA", false),
    ] {
        assert_eq!(is_title_case(input), want, "is_title_case({input:?})");
    }
    for (input, want) in [("ABC", true), ("A-B", true), ("AbC", false), ("--", false)] {
        assert_eq!(is_upper_word(input), want, "is_upper_word({input:?})");
    }
}

#[test]
fn lang_code_skip() {
    for (name, text, words, gender) in [
        (
            "inline span not checked",
            "Use `he utilizes xyzzy` with the tool.",
            0,
            false,
        ),
        (
            "fenced block removed",
            "Stop it.\n```\nhe utilizes xyzzy\n```\nUse the tool.",
            0,
            false,
        ),
        ("tilde fence", "~~~\nhe utilizes xyzzy\n~~~", 0, false),
        (
            "double backtick span",
            "Use ``a ` b he`` with the tool.",
            0,
            false,
        ),
        (
            "unclosed backtick stays text",
            "Use `he with the tool.",
            1,
            true,
        ),
    ] {
        let r = run(&[], text);
        assert_eq!(
            r.words.len(),
            words,
            "{name}: word hits {:?}",
            word_keys(&r)
        );
        assert_eq!(has_finding(&r, "GR-7"), gender, "{name}: {:?}", kinds(&r));
    }
    // span counts as one word (8.6)
    let (text, spans) = strip_code("Run `go test -race -count=1 ./... ./x` now.");
    assert_eq!(count_words(&text), 3, "count_words({text:?})");
    assert_eq!(
        restore_code(&text, &spans),
        "Run `go test -race -count=1 ./... ./x` now."
    );
    // example quotes the original span
    let r = run(&[], "Run `make build` now.");
    assert_eq!(r.word("run").unwrap().example, "Run `make build` now.");
}

#[test]
fn lang_auto_glossary_classify() {
    let d = dict();
    for (token, kind) in [
        ("internal/chat/report.go", "path"),
        ("a/b.c", "path"),
        ("/etc/hosts", "path"),
        ("x.go:12", "path"),
        ("report.go:120:5", "path"),
        ("https://example.com/x", "url"),
        ("main.go", "file"),
        (".env", "file"),
        ("snake_case", "snake_case"),
        ("__init__", "snake_case"),
        ("MAX_RETRIES", "snake_case"),
        ("camelCase", "mixed_case"),
        ("PascalCase", "mixed_case"),
        ("gRPC", "mixed_case"),
        ("APIs", "mixed_case"),
        ("fmt.Println", "dotted"),
        ("os.Exit", "dotted"),
        ("fmt.Println()", "dotted"),
        ("t.Run()", "dotted"),
        ("t.Run", "dotted"),
        ("r.Body", "dotted"),
        // the reference test uses a two-letter method name here
        ("t.Do()", "dotted"),
        ("run()", "call"),
        ("--no-cache", "flag"),
        ("--mode=auto", "flag"),
        ("-race", "flag"),
        ("API", "acronym"),
        ("CI", "acronym"),
        ("HTTP", "acronym"),
        ("EC2", "acronym"),
        ("cmd/server/", "path"),
        ("./scripts", "path"),
        // not code
        ("and/or", ""),
        ("may/might/likely/should", ""),
        ("internal/chat", ""),
        ("-ing", ""),
        ("km/h", ""),
        ("1/2", ""),
        ("e.g", ""),
        ("i.e", ""),
        ("a.m", ""),
        ("Ph.D", ""),
        ("t.Do", ""),
        ("Hello", ""),
        ("hello", ""),
        ("NOTE", ""),
        ("CAUTION", ""),
        ("SET", ""),
        ("OFF", ""),
        ("ABCDEFG", ""),
        ("x", ""),
    ] {
        assert_eq!(classify_code(d, token), kind, "classify_code({token:?})");
    }
}

#[test]
fn lang_split_punct() {
    for (chunk, pre, core, suf) in [
        ("fmt.Println,", "", "fmt.Println", ","),
        ("(fmt.Println())", "(", "fmt.Println()", ")"),
        ("x.go:12.", "", "x.go:12", "."),
        ("“gRPC”", "“", "gRPC", "”"),
        ("API's", "", "API", "'s"),
        ("API's.", "", "API", "'s."),
    ] {
        assert_eq!(
            split_punct(chunk),
            (pre, core, suf),
            "split_punct({chunk:?})"
        );
    }
}

#[test]
fn lang_auto_glossary_check() {
    let text = "The handler in internal/chat/report.go:120 calls fmt.Println and parse_tool_result. Use --no-cache with gRPC and the API.";
    let r = run(&[], text);
    for w in ["fmt", "grpc", "no-cache", "chat"] {
        assert!(r.word(w).is_none(), "{w:?} must not be a word hit");
    }
    let terms: Vec<String> = r
        .auto
        .iter()
        .map(|a| format!("{}={}", a.term, a.kind))
        .collect();
    assert_eq!(
        terms,
        [
            "internal/chat/report.go:120=path",
            "fmt.Println=dotted",
            "parse_tool_result=snake_case",
            "--no-cache=flag",
            "gRPC=mixed_case",
            "API=acronym"
        ]
    );

    // capitals in a shouted sentence are not acronyms
    let r = run(
        &[],
        "REPLACE THE GASKET ON THE API UNIT. Read the API page.",
    );
    let api = r.auto.iter().find(|a| a.term == "API");
    assert_eq!(
        api.map(|a| a.count),
        Some(1),
        "only from the normal sentence"
    );
    assert!(
        !r.auto.iter().any(|a| a.term == "GASKET"),
        "GASKET listed as an acronym"
    );

    // token counts as one word
    let g = Glossary::new([]);
    let mut c = Checker::new(dict(), Mode::Auto, &g);
    let (s, _) = c.mark_auto("Read internal/chat/report.go:120 now.");
    assert_eq!(count_words(&s), 3, "count_words({s:?})");

    // token joins a noun cluster with its own text
    let r = run(&[], "Examine the fmt.Println output buffer size.");
    let got = findings_of(&r, "2.1");
    assert!(
        got.len() == 1 && got[0].contains("fmt.Println output buffer size"),
        "want a 2.1 finding with the token text, got {:?}",
        kinds(&r)
    );
}

#[test]
fn lang_hedge_hints() {
    let text = "It may fail. It might fail. It is likely to fail. It probably fails. Perhaps it fails. It is unlikely to fail. A probable cause. You should stop.";
    let r = run(&[], text);
    for (w, sub) in [
        ("may", "POSSIBLY"),
        ("might", "POSSIBLY"),
        ("perhaps", "POSSIBLY"),
        ("likely", "IT IS VERY POSSIBLE THAT"),
        ("probably", "IT IS VERY POSSIBLE THAT"),
        ("probable", "IT IS VERY POSSIBLE THAT"),
        ("unlikely", "THE RISK IS SMALL"),
        ("should", "WE RECOMMEND THAT"),
    ] {
        let h = r
            .word(w)
            .unwrap_or_else(|| panic!("{w:?} is not a word hit"));
        assert!(h.hint.contains(sub), "hint for {w:?} = {:?}", h.hint);
    }
    let h = &r.word("should").unwrap().hint;
    assert!(
        h.contains("MUST") && h.contains("JUDGE"),
        "should hint = {h:?}"
    );
    if let Some(h) = r.word("fail") {
        assert!(h.hint.is_empty(), "fail has a hint: {:?}", h.hint);
    }
    assert!(r.render().contains("HEDGE WORDS"), "{}", r.render());
}

/// Every capitalized word in a hint is an approved dictionary word.
#[test]
fn lang_hedge_hint_words_approved() {
    let d = dict();
    for (w, hint) in HEDGE_HINTS {
        for tok in hint.split(|c: char| !c.is_ascii_uppercase()) {
            if tok.len() < 2 || tok == "JUDGE" {
                continue;
            }
            let low = tok.to_ascii_lowercase();
            let base = d
                .forms
                .get(&low)
                .unwrap_or_else(|| panic!("hint for {w:?} uses {tok:?}, not an approved word"));
            assert!(
                d.approved.contains_key(base),
                "hint for {w:?} uses {tok:?} (base {base:?}), not approved"
            );
        }
    }
}

#[test]
fn lang_json_shape() {
    let out = run(
        &[],
        "Ensure the xyzzy <is> clean & dry; use fmt.Println. It may fail.",
    )
    .to_json();
    // layout
    assert!(
        out.starts_with("{\n \"findings\": [\n  {\n   \"rule\": "),
        "unexpected layout:\n{out}"
    );
    assert!(
        !out.contains("\\u003c") && !out.contains("\\u0026") && out.contains("<is> clean & dry"),
        "HTML characters are escaped:\n{out}"
    );
    assert!(
        out.find("\"kind\": \"non_approved\"").unwrap()
            < out.find("\"alts\": \"MAKE SURE").unwrap(),
        "word entry keys out of order"
    );
    assert!(
        out.find("\"ensure\": {").unwrap() < out.find("\"xyzzy\": {").unwrap(),
        "words are not in first-seen order"
    );
    // keys
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let findings = v["findings"].as_array().unwrap();
    assert!(!findings.is_empty() && findings[0].as_object().unwrap().len() == 4);
    let auto = v["auto_glossary"].as_array().unwrap();
    assert!(
        auto.len() == 1 && auto[0]["term"] == "fmt.Println",
        "{auto:?}"
    );
    assert!(
        v["words"]["may"]["hint"]
            .as_str()
            .is_some_and(|h| !h.is_empty())
    );
    assert_eq!(v["words"]["ensure"]["kind"], "non_approved");
    // empty lists; to_json ends with the newline that the command prints
    assert_eq!(
        run(&[], "Remove the cover.").to_json(),
        "{\n \"findings\": [],\n \"words\": {},\n \"auto_glossary\": []\n}\n"
    );
}

/// The reference command line parses `--mode`; the crate has no command
/// line, so the mode names are the only part left to test.
#[test]
fn lang_parse_mode() {
    assert_eq!(Mode::parse("procedural"), Some(Mode::Procedural));
    assert_eq!(Mode::parse("descriptive"), Some(Mode::Descriptive));
    assert_eq!(Mode::parse("auto"), Some(Mode::Auto));
    assert_eq!(Mode::parse("fast"), None);
    assert_eq!(Mode::parse(""), None);
}

/// The reference command line reads a file and changes CR LF and CR to LF
/// before the check. `check` does the same.
#[test]
fn lang_line_endings() {
    let lf = check("Remove the cover.\n\nIt may fail.\n").to_json();
    assert_eq!(
        check("Remove the cover.\r\n\r\nIt may fail.\r\n").to_json(),
        lf
    );
    assert_eq!(check("Remove the cover.\r\rIt may fail.\r").to_json(), lf);
    assert_eq!(
        check_mode("Remove the cover.", Mode::Descriptive).to_json(),
        "{\n \"findings\": [],\n \"words\": {},\n \"auto_glossary\": []\n}\n"
    );
}

// ---------------------------------------------------------------- regressions

/// A possessive 's before a contraction hid the contraction. A typographic
/// apostrophe (U+2019) was not an apostrophe.
#[test]
fn lang_regression_contractions() {
    let cases: &[(&str, &str, &[&str], &str)] = &[
        (
            "possessive then contraction",
            "The user's file doesn't open.",
            &["doesn't"],
            "",
        ),
        (
            "two contractions",
            "It doesn't open and it won't close.",
            &["doesn't", "won't"],
            "",
        ),
        (
            "possessive then it's",
            "The user's valve is open and it's hot.",
            &["it's"],
            "",
        ),
        ("possessive only", "Open the operator's valve.", &[], ""),
        ("curly don't", "Don’t open the valve.", &["Don't"], "don"),
        ("curly it's", "It’s the valve.", &["It's"], ""),
        (
            "curly possessive",
            "Open the operator’s valve.",
            &[],
            "operator’s",
        ),
    ];
    for (name, text, want, not_word) in cases {
        let r = run(&[], text);
        let got: Vec<String> = findings_of(&r, "4.2")
            .iter()
            .map(|m| m.split('"').nth(1).unwrap().to_string())
            .collect();
        assert_eq!(got, *want, "{name}: {:?}", kinds(&r));
        if !not_word.is_empty() {
            assert!(
                r.word(not_word).is_none(),
                "{name}: {not_word:?} is a word hit"
            );
        }
    }
    let r = run(&[], "Don’t open the xyzzy.");
    assert_eq!(r.word("xyzzy").unwrap().example, "Don’t open the xyzzy.");
    assert_eq!(contractions("It doesn't"), ["doesn't"]);
}

/// "admin." ends with "min.", so the abbreviation guard joined two sentences.
#[test]
fn lang_regression_abbreviation_boundary() {
    for (block, want) in [
        ("Log in as admin. Then restart the server now.", 2),
        ("Set the maximum to vs. Then stop.", 1),
        ("Read the max. Value on the gauge.", 1),
        ("Move it to the Fig. 3 position.", 1),
        ("Use the formax. Then stop.", 2),
        ("Open the No. 2 valve.", 1),
        ("Read the info. Then stop.", 2),
    ] {
        let got = split_sentences(block);
        assert_eq!(got.len(), want, "split_sentences({block:?}) = {got:?}");
    }
}

/// A hard-wrapped line that starts with "it. Then" looked like a list item.
#[test]
fn lang_regression_wrapped_prose() {
    for (line, want) in [
        ("it. Then go.", false),
        ("the. End.", false),
        ("to) here", false),
        ("Stop. Run.", false),
        ("1. Run.", true),
        ("10) Run.", true),
        ("2a. Run.", true),
        ("a) Run.", true),
        ("(b) Run.", true),
        ("iv. Run.", true),
        ("- Run.", true),
        ("• Run.", true),
        ("* Run.", true),
        ("– Run.", true),
    ] {
        assert_eq!(LIST_MARKER_RE.is_match(line), want, "list marker {line:?}");
    }
    let r = run(
        &[],
        "Remove it. Clean it. Install it. Examine\nit. Measure it. Tighten it. Record it.",
    );
    assert!(has_finding(&r, "6.6"), "want 6.6, got {:?}", kinds(&r));
    let r = run(
        &[],
        "The pump is a unit that is in the system and it is in the area of the left wing and it is very near\nit. Then it stops.",
    );
    assert!(has_finding(&r, "6.3"), "want 6.3, got {:?}", kinds(&r));
}

/// A fence indented 4+ spaces (in a nested list) and an indented code block
/// were checked as prose.
#[test]
fn lang_regression_indented_code() {
    for (name, text, code) in [
        (
            "fence in a nested list",
            "- Step one:\n\n    ```\n    he utilizes xyzzy\n    ```\n\nThen stop.",
            true,
        ),
        (
            "tab-indented fence",
            "1. Step:\n\t~~~\n\the utilizes xyzzy\n\t~~~",
            true,
        ),
        (
            "indented block after a paragraph",
            "Run this:\n\n    he utilizes xyzzy\n\n    and more code\n\nThen stop.",
            true,
        ),
        (
            "indented block at the start",
            "    he utilizes xyzzy\n\nThen stop.",
            true,
        ),
        (
            "indented continuation of a list item",
            "- Step one.\n\n    he utilizes xyzzy.",
            false,
        ),
        (
            "indented line inside a paragraph",
            "Read this line\n    he utilizes xyzzy.",
            false,
        ),
        (
            "indented line after the list ends",
            "- Step one.\n\nA paragraph.\n\n    he utilizes xyzzy",
            true,
        ),
    ] {
        let r = run(&[], text);
        let skipped = r.word("xyzzy").is_none();
        assert!(
            skipped == code && has_finding(&r, "GR-7") != code,
            "{name}: code = {skipped}, want {code} (words {:?}, findings {:?})",
            word_keys(&r),
            kinds(&r)
        );
    }
    let r = run(&[], "Run this:\n\n    go test\n\nEnsure it works.");
    assert!(r.word("ensure").is_some(), "words = {:?}", word_keys(&r));
}

/// Every word that ends in "ing" got a 3.5 finding.
#[test]
fn lang_regression_ing_words() {
    for (word, want) in [
        ("thing", false),
        ("string", false),
        ("nothing", false),
        ("anything", false),
        ("everything", false),
        ("something", false),
        ("bring", false),
        ("ring", false),
        ("spring", false),
        ("king", false),
        ("ing", false),
        ("during", false),
        ("servicing", false),
        ("doing", true),
        ("merging", true),
        ("taking", true),
        ("according", true),
        ("wiring", true),
        ("testing", true),
    ] {
        assert_eq!(ing_form(word), want, "ing_form({word:?})");
    }
    let r = run(
        &[],
        "The thing in the string is nothing, so bring the ring.",
    );
    assert!(
        !has_finding(&r, "3.5") && !has_finding(&r, "3.5?"),
        "got {:?}",
        kinds(&r)
    );
}

/// Findings were sorted by the "where" text, so "para 10" came before "para 2".
#[test]
fn lang_regression_finding_order() {
    let mut paras = ["Remove the cover."; 11];
    paras[1] = "Stop the pump; open the valve.";
    paras[9] = "Stop the fan; open the door.";
    paras[10] = "One. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten; eleven.";
    let r = run(&[], &paras.join("\n\n")).render();
    let p2 = r.find("(para 2, sentence 2)");
    let p10 = r.find("(para 10, sentence 10)");
    let p11 = r.find("(para 11, sentence 20)");
    assert!(
        matches!((p2, p10, p11), (Some(a), Some(b), Some(c)) if a < b && b < c),
        "want para 2 < para 10 < para 11 ({p2:?}, {p10:?}, {p11:?}):\n{r}"
    );
}

/// The summary ignored "not approved as a verb" words, and "No findings" was
/// printed with them listed.
#[test]
fn lang_regression_summary_counts_maybe_nouns() {
    let r = run(&[], "The guard fails.");
    assert_eq!(r.word("fail").map(|w| w.kind.as_str()), Some("maybe_noun"));
    let text = r.render();
    assert!(
        !text.contains("No findings") && text.contains("Not approved as a verb: 1"),
        "report:\n{text}"
    );
    let text = run(&[], "Remove the cover.").render();
    assert!(
        text.contains("No findings") && text.contains("Not approved as a verb: 0"),
        "clean report:\n{text}"
    );
}

/// An adverb and a verb were counted as nouns in a noun cluster.
#[test]
fn lang_regression_noun_cluster() {
    for (name, text, want) in [
        (
            "adverb and verb",
            "This may have broken the oracle; the guard probably fails silently when the pin is set.",
            "",
        ),
        ("user example", "The guard probably fails silently.", ""),
        (
            "modal verb ends the run",
            "Examine the cable clamp bolt may fail.",
            "",
        ),
        (
            "noun cluster",
            "Examine the fuel pump pressure sensor connector.",
            "fuel pump pressure sensor connector",
        ),
        (
            "noun ending in -ly",
            "Remove the fuel pump assembly bracket.",
            "fuel pump assembly bracket",
        ),
        (
            "plural noun that is a non-approved verb",
            "Remove the four stainless steel pan head machine screws.",
            "four stainless steel pan head machine screws",
        ),
    ] {
        let r = run(&[], text);
        let got = findings_of(&r, "2.1")
            .last()
            .map(|m| m.split('"').nth(1).unwrap().to_string())
            .unwrap_or_default();
        assert_eq!(got, want, "{name}: {:?}", kinds(&r));
    }
}

// ---------------------------------------------------------------- rival additions

/// For each corpus file, the JSON report equals the reference output byte
/// for byte.
#[test]
fn lang_golden_corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/lang");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 9, "corpus files in {}", dir.display());
    let mut failed = Vec::new();
    for md in &files {
        let text = std::fs::read_to_string(md).unwrap();
        let want = std::fs::read_to_string(md.with_extension("json")).unwrap();
        let got = check(&text).to_json();
        if got != want {
            let at = got
                .bytes()
                .zip(want.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(got.len().min(want.len()));
            let line = want[..at].matches('\n').count() + 1;
            failed.push(format!(
                "{}: first difference at byte {at} (line {line}):\n  got:  {:?}\n  want: {:?}",
                md.display(),
                got[at..].lines().next().unwrap_or(""),
                want[at..].lines().next().unwrap_or("")
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn lang_hard_counts_definite_findings() {
    // "3.6?" asks the writer to confirm; "8.1" is definite.
    let r = run(&[], "The valve is removed; stop the pump.");
    assert!(
        has_finding(&r, "3.6?") && has_finding(&r, "8.1"),
        "{:?}",
        kinds(&r)
    );
    assert_eq!(
        r.hard(),
        r.findings.iter().filter(|f| !f.rule.ends_with('?')).count()
    );
    assert_eq!(r.hard(), 1);
}

#[test]
fn lang_render_word_lines() {
    let r = run(&[], "Ensure that the xyzzy is clean. It may fail.");
    let text = r.render();
    for want in [
        "  ensure (v) x1  ->  MAKE SURE",
        "example: \"Ensure that the xyzzy is clean.\"",
        "  xyzzy x1",
        "  may x1  ->  POSSIBLY",
    ] {
        assert!(text.contains(want), "render lacks {want:?}:\n{text}");
    }
}

#[test]
fn lang_software_glossary_loaded() {
    // "commit" and "merge request" are in the built-in software glossary.
    let r = check("Open the merge request and commit the diff.");
    assert!(
        r.word("commit").is_none() && r.word("merge").is_none(),
        "{:?}",
        word_keys(&r)
    );
    let r = run(&[], "Open the merge request and commit the diff.");
    assert!(r.word("merge").is_some(), "{:?}", word_keys(&r));
}
