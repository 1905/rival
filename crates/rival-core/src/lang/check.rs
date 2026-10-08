//! The checks. A port of the reference checker with the same logic and the
//! same order of findings, messages and "where" strings.
//!
//! Character classes follow the reference checker, not Rust's defaults:
//! - A letter is general category L (`char::is_alphabetic` also takes
//!   marks and letter numbers). Lower, upper and title case are Ll, Lu, Lt.
//! - `\b`, `\w`, `\d` and `\S` in the reference patterns are ASCII only, so
//!   the patterns here spell them as ASCII classes.
//! - Lower case is the simple per-character mapping: no final sigma, and
//!   U+0130 maps to "i".
//! - Byte offsets and character offsets stay as the reference has them.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::Mode;
use super::dict::{DEINFLECT, Dict, NotApprovedEntry, hedge_hint, plural_forms};
use super::report::{AutoHit, Finding, Report, WordHit};

// ---------------------------------------------------------------- characters

static LETTER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").unwrap());
static LOWER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Ll}$").unwrap());
static UPPER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Lu}$").unwrap());
static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Lt}$").unwrap());

fn class_has(re: &Regex, c: char) -> bool {
    let mut buf = [0u8; 4];
    re.is_match(c.encode_utf8(&mut buf))
}

/// General category L.
pub(super) fn is_letter(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic()
    } else {
        class_has(&LETTER_RE, c)
    }
}

/// General category Ll.
fn is_lower(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_lowercase()
    } else {
        class_has(&LOWER_RE, c)
    }
}

/// General category Lu.
fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_uppercase()
    } else {
        class_has(&UPPER_RE, c)
    }
}

/// General category Lt.
fn is_title(c: char) -> bool {
    !c.is_ascii() && class_has(&TITLE_RE, c)
}

/// General category N (Nd, Nl, No).
fn is_number(c: char) -> bool {
    c.is_numeric()
}

/// The simple lower-case mapping of one character.
fn lower_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    // Only U+0130 has a longer full mapping ("i" + U+0307); its simple
    // mapping is "i".
    c.to_lowercase().next().unwrap_or(c)
}

/// The simple title-case mapping of one character.
fn title_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    match c {
        '\u{01C4}'..='\u{01C6}' => return '\u{01C5}',
        '\u{01C7}'..='\u{01C9}' => return '\u{01C8}',
        '\u{01CA}'..='\u{01CC}' => return '\u{01CB}',
        '\u{01F1}'..='\u{01F3}' => return '\u{01F2}',
        // Georgian Mkhedruli has upper-case letters but is its own title case.
        '\u{10D0}'..='\u{10FA}' | '\u{10FD}'..='\u{10FF}' => return c,
        _ => {}
    }
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u,
        // A full mapping of two or more characters: the simple mapping is
        // the title-case letter with the iota subscript, or none.
        _ => greek_iota_title(c).unwrap_or(c),
    }
}

/// The simple title-case mapping of a Greek letter with an iota subscript
/// (U+1F80..U+1FFC): the full mapping is two characters, the simple one is
/// one letter.
fn greek_iota_title(c: char) -> Option<char> {
    let u = c as u32;
    let t = match u {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => u + 8,
        0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF => u,
        0x1FB3 | 0x1FC3 | 0x1FF3 => u + 9,
        0x1FBC | 0x1FCC | 0x1FFC => u,
        _ => return None,
    };
    char::from_u32(t)
}

/// Lower case, one character at a time.
pub(crate) fn lower(s: &str) -> String {
    s.chars().map(lower_char).collect()
}

/// Simple case-insensitive equality of two characters.
fn equal_fold(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(&b);
    }
    // Dotted capital I and dotless i have case mappings but no simple case
    // folding.
    if ['\u{130}', '\u{131}'].contains(&a) || ['\u{130}', '\u{131}'].contains(&b) {
        return false;
    }
    let up = |c: char| {
        let mut u = c.to_uppercase();
        match (u.next(), u.next()) {
            (Some(x), None) => x,
            _ => c,
        }
    };
    lower_char(a) == lower_char(b) || up(a) == up(b)
}

pub(super) fn is_word_rune(c: char) -> bool {
    c == '_' || is_letter(c) || is_number(c)
}

pub(crate) fn rune_len(s: &str) -> usize {
    s.chars().count()
}

/// A word with capitals and no lower-case letters ("ABC", "A-B").
pub(super) fn is_upper_word(s: &str) -> bool {
    let mut cased = false;
    for r in s.chars() {
        if is_lower(r) || is_title(r) {
            return false;
        }
        cased = cased || is_upper(r);
    }
    cased
}

/// Text where each word starts with a capital and continues in lower case.
pub(super) fn is_title_case(s: &str) -> bool {
    let mut prev_cased = false;
    for r in s.chars() {
        let cased = is_upper(r) || is_lower(r) || is_title(r);
        if cased && ((prev_cased && lower_char(r) != r) || (!prev_cased && title_char(r) != r)) {
            return false;
        }
        prev_cased = cased;
    }
    true
}

pub(crate) fn word_set(s: &'static str) -> HashSet<&'static str> {
    s.split_whitespace().collect()
}

pub(crate) fn has_any_suffix(s: &str, sufs: &[&str]) -> bool {
    sufs.iter().any(|x| s.ends_with(x))
}

/// `\b` with Unicode word characters.
fn boundary(rs: &[char], i: usize) -> bool {
    (i > 0 && is_word_rune(rs[i - 1])) != (i < rs.len() && is_word_rune(rs[i]))
}

/// Matches `lit` (case-insensitive) at `rs[i..]` after a word boundary, and
/// before one if `tail`. Returns the end of the match.
fn match_at(rs: &[char], i: usize, lit: &str, tail: bool) -> Option<usize> {
    let l: Vec<char> = lit.chars().collect();
    if !boundary(rs, i) || i + l.len() > rs.len() {
        return None;
    }
    for (k, &r) in l.iter().enumerate() {
        if !equal_fold(rs[i + k], r) {
            return None;
        }
    }
    let e = i + l.len();
    (!tail || boundary(rs, e)).then_some(e)
}

/// The first match of one of `alts`, trying them in order at each position.
fn find_bounded(s: &str, alts: &[&str], tail: bool) -> String {
    let rs: Vec<char> = s.chars().collect();
    for i in 0..=rs.len() {
        for a in alts {
            if let Some(e) = match_at(&rs, i, a, tail) {
                return rs[i..e].iter().collect();
            }
        }
    }
    String::new()
}

/// Replaces `lit`, as a whole word or phrase in any case, with `repl`.
fn replace_bounded(s: &str, lit: &str, repl: &str) -> String {
    let rs: Vec<char> = s.chars().collect();
    let mut b = String::with_capacity(s.len());
    let mut i = 0;
    while i < rs.len() {
        if let Some(e) = match_at(&rs, i, lit, true).filter(|&e| e > i) {
            b.push_str(repl);
            i = e;
            continue;
        }
        b.push(rs[i]);
        i += 1;
    }
    b
}

// ---------------------------------------------------------------- patterns

/// White space: the Unicode `White_Space` property.
pub(crate) const WS: &str = r"[\s\v\x{85}\p{Z}]";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

/// A list marker: 1. 1) 10a. (a) a. iv. - • * –; never a short word such
/// as "it." in wrapped prose.
pub(super) static LIST_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"^{WS}*(\(?([0-9]{{1,3}}[a-z]?|[A-Za-z]|[ivxIVX]{{2,4}})[.)]|[-•*–]|\p{{Nd}}+\.){WS}+"
    ))
});
static LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?i)^(WARNING|CAUTION|NOTE|DANGER|NOTICE){WS}*:{WS}*"
    ))
});
static SAFETY_LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"(?i)^(WARNING|CAUTION|DANGER){WS}*:{WS}*")));
static NOTE_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"(?i)^NOTE{WS}*:")));
static HEADING_HASH_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{WS}*#{{1,6}}{WS}")));
static PAREN_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\([^()]*\)"));
static QUOTE_RE: LazyLock<Regex> = LazyLock::new(|| re(r#""[^"]+"|“[^”]+”"#));
static DIGIT_DOT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(\p{Nd})\.(\p{Nd})"));
static ABBR_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(^|[^\pL\pN_])(e\.g\.|i\.e\.|etc\.|No\.|Fig\.|fig\.|a\.m\.|p\.m\.|vs\.|approx\.|max\.|min\.)",
    )
});
static RAW_TOK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z][A-Za-z'\-_]*"));
static HEAD_WORDS_RE: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z']+"));
static COMMA_WORD_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r",{WS}*([A-Za-z']+)")));
static CHUNK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"[^\s\v\x{85}\p{Z}]+"));
static CODE_PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| re(r"CODESPAN[A-Z]{3}"));

/// The units that count as one word (8.4–8.7): parenthesis (8.5), quoted
/// text, number with unit, identifier, abbreviation (8.6), hyphenated word
/// (8.7), word. It runs on `ascii_words(s)`, with ASCII `\b` and `\w`.
static ONE_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    let units: Vec<String> = "°C °F % mm cm m km in ft kg g lb lbs oz psi kpa mpa bar nm n v kv mv a ma ka w kw mw hz khz mhz ghz
		ohm ohms Ω s ms min h hr hrs l ml gal rpm db c f k"
        .split_whitespace()
        .map(regex::escape)
        .collect();
    let b = r"(?-u:\b)";
    let w = "[0-9A-Za-z_]";
    let num = r"[+\-−]?[0-9][0-9,.:/]*";
    re(&format!(
        r#"\([^()]*\)|"[^"]+"|“[^”]+”|{num}{WS}?(?:{units}){b}|{num}|{b}[A-Z0-9][A-Z0-9\-/.]*[0-9][A-Z0-9\-/.]*{b}|{b}[A-Z]{{2,}}(?:[-/][A-Z0-9]+)*{b}|{b}{w}+(?:-{w}+)+{b}|{b}[0-9A-Za-z_']+{b}"#,
        units = units.join("|"),
    ))
});

// auto-glossary
static FLAG_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^--?[A-Za-z][A-Za-z0-9_-]*(=[^\t\n\f\r ]*)?$"));
static URL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9+.-]*://[^\t\n\f\r ]+$"));
static PATH_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[0-9A-Za-z_./~-]*\.[A-Za-z0-9]+(:[0-9]+)+$"));
static PATH_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9A-Za-z_.~/@:+-]+$"));
static FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*\.(go|mod|sum|py|ts|tsx|js|jsx|mjs|cjs|json|ya?ml|toml|ini|cfg|conf|env|md|txt|csv|sql|sh|bash|zsh|rs|java|kt|rb|php|c|h|cc|cpp|hpp|proto|html|css|xml|lock|log|tf)$",
    )
});
static DOTFILE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\.[A-Za-z][0-9A-Za-z_.-]*$"));
static DOTTED_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)+$"));
static SNAKE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^_*[A-Za-z0-9]+(_+[A-Za-z0-9]+)*_*$"));
static MIXED_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9]*$"));
static CALL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*$"));
static ACRONYM_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Z][A-Z0-9]{1,5}$"));

// ---------------------------------------------------------------- word lists

macro_rules! word_list {
    ($name:ident, $words:expr) => {
        static $name: LazyLock<HashSet<&'static str>> = LazyLock::new(|| word_set($words));
    };
}

// The only approved -ing forms (Rule 3.5).
word_list!(
    APPROVED_ING,
    "lighting opening routing servicing mating missing remaining something during"
);
word_list!(
    IRREGULAR_PARTICIPLES,
    "been done given gone made put set shown seen held kept known taken written broken cut let read sent
	spent left found got worn torn bent built lost run won begun become come drawn driven fallen flown frozen hidden hit led lit met paid
	said sold spoken stuck thrown understood hung shut spread split"
);
word_list!(BE_FORMS, "is are was were be been being am");
word_list!(HAVE_FORMS, "have has had");
word_list!(
    NUMBER_WORDS,
    "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen
	seventeen eighteen nineteen twenty thirty forty fifty sixty seventy eighty ninety hundred thousand million half quarter third fourth fifth"
);
word_list!(
    DETERMINERS,
    "the a an this these its their your our each all some many no of front rear top bottom left right new old
	primary applicable correct two three four five six"
);
// Modals and common verbs with no noun sense.
word_list!(
    ALWAYS_VERB,
    "may might should would could shall ought ensure take perform provide obtain require allow enable indicate
	occur avoid achieve verify confirm assure determine establish maintain prevent proceed remain become seem appear consist contain
	possess acquire utilize utilise employ conduct carry commence cease terminate initiate activate deactivate locate relocate
	accomplish facilitate eliminate exceed persist arise happen depend rely refer reinstall retighten"
);
word_list!(
    VERB_SLOT,
    "to you we they it can will must not do does did and then or always never carefully slowly immediately also only"
);
word_list!(
    FUNCTION_WORDS,
    "a an the and or of to in on at for with by from as is are was were be not no if when that this these it
	you we they than then thus but also only each all its their"
);
word_list!(CONDITION_HEADS, "if when before after while until unless");
word_list!(
    IMPERATIVE_HEADS,
    "do make put set remove install connect disconnect turn press push pull open close start stop use apply
	examine measure tighten loosen attach hold keep wait read refer record replace discard clean fill drain lift lower move operate
	energize de-energize obey get go let cut lubricate tag torque adjust align release engage disengage select touch enter type click
	tap swipe scroll save download upload reboot restart update insert mount unscrew screw supply send give show write add count
	identify compare continue repeat always never only carefully slowly immediately then first also"
);
word_list!(
    CONTRACTION_SET,
    "it's let's that's there's what's here's who's"
);
word_list!(LABEL_WORDS, "WARNING CAUTION NOTE DANGER NOTICE");
// Suffixes named in prose about grammar, not command-line flags.
word_list!(SUFFIX_MENTIONS, "-ing -ed -er -est -ly -s -es -ies");

const MULTI_WORD_PHRASES: [&str; 22] = [
    "make sure",
    "makes sure",
    "made sure",
    "away from",
    "out of",
    "in front of",
    "because of",
    "adjacent to",
    "each other",
    "in progress",
    "put on",
    "come on",
    "go off",
    "aft of",
    "forward of",
    "inboard of",
    "outboard of",
    "upstream of",
    "downstream of",
    "for example",
    "as a result",
    "at the same time",
];

const LATIN_ALTS: [&str; 8] = [
    "e.g.", "i.e.", "etc.", "etc", "et al.", "et al", "vs.", "viz.",
];
const GENDER_ALTS: [&str; 8] = [
    "he", "she", "him", "her", "his", "hers", "himself", "herself",
];

// ---------------------------------------------------------------- word count (rules 8.4–8.7)

/// Changes each non-ASCII letter or digit to "a", so that the ASCII `\w`
/// of the word pattern sees a word character.
fn ascii_words(s: &str) -> String {
    s.chars()
        .map(|r| {
            if !r.is_ascii() && (is_letter(r) || is_number(r)) {
                'a'
            } else {
                r
            }
        })
        .collect()
}

/// The words of a sentence. List markers and safety labels do not count
/// (8.4).
pub(super) fn count_words(sentence: &str) -> usize {
    let s = LIST_MARKER_RE.replace(sentence.trim(), "");
    let s = LABEL_RE.replace(&s, "");
    ONE_WORD_RE
        .find_iter(&ascii_words(&s))
        .filter(|t| t.as_str().chars().any(is_word_rune))
        .count()
}

// ---------------------------------------------------------------- segmentation

fn split_paragraphs(text: &str) -> Vec<Vec<String>> {
    let mut paras = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for line in text.split('\n').chain(std::iter::once("")) {
        if !line.trim().is_empty() {
            cur.push(line.to_string());
        } else if !cur.is_empty() {
            paras.push(std::mem::take(&mut cur));
        }
    }
    paras
}

fn is_heading(line: &str) -> bool {
    let st = line.trim();
    HEADING_HASH_RE.is_match(line)
        || (line.split_whitespace().count() <= 8 && is_title_case(st) && !st.ends_with('.'))
}

/// Splits a block after . ! ? or : when a capital, a digit, or a quote or
/// parenthesis before one comes next. A colon that ends a list intro ends a
/// sentence (8.4).
pub(super) fn split_sentences(block: &str) -> Vec<String> {
    let t = ABBR_RE.replace_all(block, |c: &Captures| {
        format!("{}{}", &c[1], c[2].replace('.', "<DOT>"))
    });
    let rs: Vec<char> = DIGIT_DOT_RE
        .replace_all(&t, "${1}<DOT>${2}")
        .chars()
        .collect();
    let is_az09 = |k: usize| k < rs.len() && (rs[k].is_ascii_uppercase() || rs[k].is_ascii_digit());
    let mut out = Vec::new();
    let mut emit = |p: &[char]| {
        let p: String = p.iter().collect();
        let p = p.replace("<DOT>", ".");
        let p = p.trim();
        if !p.is_empty() {
            out.push(p.to_string());
        }
    };
    let mut last = 0;
    let mut p = 1;
    while p < rs.len() {
        if ".!?:".contains(rs[p - 1]) && rs[p].is_whitespace() {
            let mut q = p;
            while q < rs.len() && rs[q].is_whitespace() {
                q += 1;
            }
            if is_az09(q) || (q < rs.len() && "\"“(".contains(rs[q]) && is_az09(q + 1)) {
                emit(&rs[last..p]);
                last = q;
                p = q;
            }
        }
        p += 1;
    }
    emit(&rs[last..]);
    out
}

// ---------------------------------------------------------------- code skip

/// An all-caps word for the text it stands for; records the text in `m`.
/// The word count makes it one word (8.6), and the word checks skip it.
fn placeholder(prefix: &str, m: &mut HashMap<String, String>, text: &str) -> String {
    let k = m.len();
    let letter = |n: usize| char::from(b'A' + (n % 26) as u8);
    let p = format!("{prefix}{}{}{}", letter(k / 676), letter(k / 26), letter(k));
    m.insert(p.clone(), text.to_string());
    p
}

/// Puts the original `code` spans back, for the examples in the report.
pub(super) fn restore_code(s: &str, spans: &HashMap<String, String>) -> String {
    CODE_PLACEHOLDER_RE
        .replace_all(s, |c: &Captures| {
            spans
                .get(&c[0])
                .cloned()
                .unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

fn run_len(s: &[u8], c: u8) -> usize {
    s.iter().take_while(|&&b| b == c).count()
}

/// Replaces each `code` span with one word. Unclosed backticks stay.
fn strip_inline_code(line: &str, spans: &mut HashMap<String, String>) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut lit = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        out.push_str(&line[lit..i]);
        let n = run_len(&b[i..], b'`');
        let mut end = None;
        let mut k = i + n;
        while k < b.len() && end.is_none() {
            let m = run_len(&b[k..], b'`');
            if m == n {
                end = Some(k);
            } else {
                k += m.max(1);
            }
        }
        match end {
            None => {
                out.push_str(&line[i..i + n]);
                i += n;
            }
            Some(e) => {
                out.push_str(&placeholder("CODESPAN", spans, &line[i..e + n]));
                i = e + n;
            }
        }
        lit = i;
    }
    out.push_str(&line[lit..]);
    out
}

/// Blanks code blocks (so they also end the paragraph) and collapses inline
/// spans. A fence can have any indent (a fence in a nested list). An
/// indented block (4 spaces or a tab) is code after a blank line, but not in
/// a list, where the indent continues a list item. Returns the new text and
/// the original spans.
pub(super) fn strip_code(text: &str) -> (String, HashMap<String, String>) {
    let mut spans = HashMap::new();
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let mut fence_ch = 0u8;
    let mut fence_len = 0;
    let (mut indented, mut in_list, mut after_blank) = (false, false, true);
    for line in &mut lines {
        let l = std::mem::take(line);
        let body = l.trim_start_matches([' ', '\t']);
        let is_indented = l.starts_with("    ") || l.starts_with('\t');
        if fence_len > 0 {
            let n = run_len(body.as_bytes(), fence_ch);
            if n >= fence_len && body[n..].trim().is_empty() {
                fence_len = 0;
            }
        } else if body.trim().is_empty() {
            after_blank = true;
            *line = l;
            continue;
        } else if is_indented && (indented || (after_blank && !in_list)) {
            indented = true;
        } else {
            indented = false;
            for ch in *b"`~" {
                let n = run_len(body.as_bytes(), ch);
                if n >= 3 && !(ch == b'`' && body[n..].contains('`')) {
                    fence_ch = ch;
                    fence_len = n;
                    break;
                }
            }
            if fence_len == 0 {
                if LIST_MARKER_RE.is_match(&l) {
                    in_list = true;
                } else if after_blank && !is_indented {
                    in_list = false;
                }
                *line = strip_inline_code(&l, &mut spans);
            }
        }
        after_blank = false;
    }
    (lines.join("\n"), spans)
}

// ---------------------------------------------------------------- auto-glossary

/// A sentence written in capitals (as the dictionary examples are). Its
/// capitalized words are ordinary words, not acronyms.
fn shouted(s: &str) -> bool {
    let (mut caps, mut n) = (0, 0);
    for w in s.split(|r: char| !is_letter(r)).filter(|w| !w.is_empty()) {
        if w.len() >= 2 {
            n += 1;
            if is_upper_word(w) {
                caps += 1;
            }
        }
    }
    n >= 3 && caps * 2 > n
}

/// Separates leading and trailing prose punctuation (and a possessive 's)
/// from a chunk.
pub(super) fn split_punct(ch: &str) -> (&str, &str, &str) {
    let core = ch.trim_start_matches(|r: char| "(\"'“‘<[{".contains(r));
    let pre = &ch[..ch.len() - core.len()];
    let mut end = core.len();
    while let Some(r) = core[..end].chars().next_back() {
        // keep the ")" of a call such as fmt.Println()
        if !".,;:!?)]}\"'>”’".contains(r)
            || (r == ')' && core[..end].matches('(').count() >= core[..end].matches(')').count())
        {
            break;
        }
        end -= r.len_utf8();
    }
    for p in ["'s", "’s"] {
        if core[..end].ends_with(p) && end > p.len() {
            end -= p.len();
            break;
        }
    }
    (pre, &core[..end], &core[end..])
}

/// The auto-glossary kind of a token, or "".
pub(super) fn classify_code(d: &Dict, core: &str) -> &'static str {
    if core.len() < 2 || !core.chars().any(is_letter) {
        return "";
    }
    if FLAG_RE.is_match(core) && !SUFFIX_MENTIONS.contains(core) {
        return "flag";
    }
    if URL_RE.is_match(core) {
        return "url";
    }
    if PATH_LINE_RE.is_match(core) {
        return "path";
    }
    // a path has a dot, a leading "/", "./", "../" or "~/", or a trailing
    // "/"; "and/or" and "may/might/should" are word lists, not paths
    if core.contains('/')
        && PATH_RE.is_match(core)
        && (core.as_bytes()[1..].contains(&b'.')
            || core.starts_with('/')
            || core.starts_with('.')
            || core.starts_with("~/")
            || core.ends_with('/'))
    {
        return "path";
    }
    if FILE_RE.is_match(core) || DOTFILE_RE.is_match(core) {
        return "file";
    }
    let base = core.strip_suffix("()").unwrap_or(core);
    if DOTTED_RE.is_match(base) {
        // e.g / i.e / a.m / Ph.D are not identifiers. A one-letter part is
        // accepted only next to a part of 3+ characters or before "()":
        // t.Run, r.Body, w.Write(), t.Do().
        let short = base.split('.').any(|seg| seg.len() < 2);
        let long = base.split('.').any(|seg| seg.len() >= 3);
        if short && !long && base == core {
            return "";
        }
        return "dotted";
    }
    if base.contains('_') && SNAKE_RE.is_match(base) {
        return "snake_case";
    }
    if MIXED_RE.is_match(base) && base.chars().any(is_lower) && base[1..].chars().any(is_upper) {
        return "mixed_case";
    }
    if base != core && CALL_RE.is_match(base) {
        return "call";
    }
    if ACRONYM_RE.is_match(core) && !LABEL_WORDS.contains(core) {
        let low = lower(core);
        if !d.forms.contains_key(&low) && !d.non_approved.contains_key(&low) {
            return "acronym";
        }
    }
    ""
}

// ---------------------------------------------------------------- checker

/// The glossary terms: every term and its plurals, and the multi-word terms
/// longest first.
#[derive(Debug, Default)]
pub(crate) struct Glossary {
    pub(super) terms: HashSet<String>,
    pub(super) multi: Vec<String>,
}

impl Glossary {
    /// Builds a glossary from the lines of a glossary file: one term per
    /// line, "#" starts a comment line.
    pub(crate) fn new<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut g = Glossary::default();
        for line in lines {
            let t = lower(line.trim());
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let mut forms = vec![t.clone()];
            forms.extend(plural_forms(&t));
            if t.contains(' ') {
                g.multi.extend(forms.iter().cloned());
            }
            g.terms.extend(forms);
        }
        g.multi.sort_by_key(|m| std::cmp::Reverse(rune_len(m)));
        g
    }
}

/// The state of one check run.
pub(crate) struct Checker<'g> {
    d: &'static Dict,
    mode: Mode,
    glossary: &'g Glossary,
    findings: Vec<Finding>,
    words: Vec<WordHit>,
    word_index: HashMap<String, usize>,
    autos: Vec<AutoHit>,
    auto_index: HashMap<String, usize>,
    code_spans: HashMap<String, String>,
}

/// Where a finding is: its paragraph, sentence and "where" text.
struct At {
    para: usize,
    sent: usize,
    text: String,
}

impl<'g> Checker<'g> {
    pub(crate) fn new(d: &'static Dict, mode: Mode, glossary: &'g Glossary) -> Self {
        Checker {
            d,
            mode,
            glossary,
            findings: Vec::new(),
            words: Vec::new(),
            word_index: HashMap::new(),
            autos: Vec::new(),
            auto_index: HashMap::new(),
            code_spans: HashMap::new(),
        }
    }

    pub(crate) fn into_report(self) -> Report {
        Report {
            findings: self.findings,
            words: self.words,
            auto: self.autos,
        }
    }

    fn add(&mut self, at: &At, rule: &str, kind: &str, message: String) {
        self.findings.push(Finding {
            rule: rule.to_string(),
            kind: kind.to_string(),
            message,
            r#where: at.text.clone(),
            para: at.para,
            sent: at.sent,
        });
    }

    fn hit(&mut self, key: &str, kind: &str, alts: String, example: &str, pos: String) {
        let i = match self.word_index.get(key) {
            Some(&i) => i,
            None => {
                self.words.push(WordHit {
                    word: key.to_string(),
                    kind: kind.to_string(),
                    alts,
                    count: 0,
                    example: example.to_string(),
                    pos,
                    hint: hedge_hint(key).to_string(),
                });
                self.word_index
                    .insert(key.to_string(), self.words.len() - 1);
                self.words.len() - 1
            }
        };
        self.words[i].count += 1;
    }

    fn non_approved(&self, w: &str) -> &'static [NotApprovedEntry] {
        self.d.non_approved.get(w).map_or(&[], Vec::as_slice)
    }

    /// Returns ("approved"|"approved-inflected"|"non_approved"|"glossary"|"unknown", base).
    pub(super) fn lookup_base(&self, t: &str) -> (&'static str, String) {
        if self.glossary.terms.contains(t) {
            return ("glossary", t.to_string());
        }
        if let Some(f) = self.d.forms.get(t) {
            return ("approved", f.clone());
        }
        if self.d.non_approved.contains_key(t) {
            return ("non_approved", t.to_string());
        }
        for r in &DEINFLECT {
            if t.ends_with(r.suf) && rune_len(t) > r.suf.len() + 2 {
                let b = format!("{}{}", &t[..t.len() - r.suf.len()], r.repl);
                if let Some(f) = self.d.forms.get(&b) {
                    return ("approved-inflected", f.clone());
                }
                if self.d.non_approved.contains_key(&b) {
                    return ("non_approved", b);
                }
            }
        }
        ("unknown", t.to_string())
    }

    fn approved_has_pos(&self, base: &str, pos: &[&str]) -> bool {
        self.d
            .approved
            .get(base)
            .is_some_and(|es| es.iter().any(|e| pos.contains(&e.pos.as_str())))
    }

    fn is_verb(&self, w: &str) -> bool {
        let w = lower(w);
        let base = self.d.forms.get(&w).unwrap_or(&w);
        self.approved_has_pos(base, &["v"])
    }

    fn is_function(&self, w: &str) -> bool {
        FUNCTION_WORDS.contains(w)
            || self.approved_has_pos(w, &["prep", "conj", "art", "pron", "adv"])
    }

    /// An -ly word that the dictionary does not give as a noun or adjective
    /// ("silently", not "assembly" or "early").
    fn is_adverb(&self, t: &str) -> bool {
        let noun_adj = |pos: &str| pos == "n" || pos == "adj";
        let base = self.d.forms.get(t).map_or("", String::as_str);
        t.ends_with("ly")
            && rune_len(t) > 3
            && !self.glossary.terms.contains(t)
            && !self
                .d
                .approved
                .get(base)
                .is_some_and(|es| es.iter().any(|e| noun_adj(&e.pos)))
            && !self.non_approved(t).iter().any(|e| noun_adj(&e.pos))
    }

    fn sentence_mode(&self, sent: &str) -> Mode {
        if self.mode != Mode::Auto {
            return self.mode;
        }
        // auto: imperative opener, list step, or safety word -> procedural
        let first = LIST_MARKER_RE.replace(sent, "");
        let first = SAFETY_LABEL_RE.replace(&first, "");
        let head = match HEAD_WORDS_RE.find(&first) {
            Some(m) if !NOTE_RE.is_match(sent) => lower(m.as_str()),
            _ => return Mode::Descriptive,
        };
        // condition-first imperative: "If/When/Before ..., do X"
        if CONDITION_HEADS.contains(head.as_str())
            && let Some(m) = COMMA_WORD_RE.captures(&first)
            && self.is_verb(&m[1])
        {
            return Mode::Procedural;
        }
        if IMPERATIVE_HEADS.contains(head.as_str())
            || (self.is_verb(&head) && head != "it" && head != "this" && head != "there")
        {
            return Mode::Procedural;
        }
        Mode::Descriptive
    }

    /// Runs every check on `text` and fills the findings and word hits.
    pub(crate) fn check(&mut self, text: &str) {
        let (text, spans) = strip_code(text);
        self.code_spans = spans;
        let mut sent_no = 0;
        for (i, mut lines) in split_paragraphs(&text).into_iter().enumerate() {
            let pi = i + 1;
            if lines.len() == 1 && is_heading(&lines[0]) {
                continue;
            }
            let n_list = lines.iter().filter(|l| LIST_MARKER_RE.is_match(l)).count();
            let mut sentences = Vec::new();
            if n_list >= (lines.len() / 2).max(1) {
                for l in &lines {
                    sentences.extend(split_sentences(l));
                }
            } else {
                for l in &mut lines {
                    *l = l.trim().to_string();
                }
                sentences = split_sentences(&lines.join(" "));
                if sentences.len() > 6 {
                    let at = At {
                        para: pi,
                        sent: 0,
                        text: format!("para {pi}"),
                    };
                    let msg = format!(
                        "Paragraph {pi} has {} sentences (max 6). Divide it.",
                        sentences.len()
                    );
                    self.add(&at, "6.6", "paragraph", msg);
                }
            }
            for s in &sentences {
                sent_no += 1;
                self.check_sentence(s, pi, sent_no);
            }
        }
    }

    /// Replaces code-like tokens with a one-word placeholder and records
    /// them. Acronyms are only recorded: the word checks already skip
    /// all-caps words.
    pub(super) fn mark_auto(&mut self, s: &str) -> (String, HashMap<String, String>) {
        let mut autos = HashMap::new();
        let loud = shouted(s);
        let d = self.d;
        let out = CHUNK_RE
            .replace_all(s, |c: &Captures| {
                let ch = &c[0];
                let (pre, core, suf) = split_punct(ch);
                let kind = classify_code(d, core);
                if kind.is_empty() || (kind == "acronym" && loud) {
                    return ch.to_string();
                }
                let i = match self.auto_index.get(core) {
                    Some(&i) => i,
                    None => {
                        self.autos.push(AutoHit {
                            term: core.to_string(),
                            kind: kind.to_string(),
                            count: 0,
                        });
                        self.auto_index
                            .insert(core.to_string(), self.autos.len() - 1);
                        self.autos.len() - 1
                    }
                };
                self.autos[i].count += 1;
                if kind == "acronym" {
                    return ch.to_string();
                }
                format!(
                    "{pre}{}{suf}",
                    placeholder("GLOSSARYAUTO", &mut autos, core)
                )
            })
            .into_owned();
        (out, autos)
    }

    /// The words for the word-level checks, in lower case. A token that
    /// starts with "\0" joins noun clusters under its own text: a multi-word
    /// glossary term ("<term>"), an auto-glossary token, or a capitalized
    /// word mid-sentence. Multi-word approved phrases are joined with "_".
    fn tokenize(&self, s: &str, autos: &HashMap<String, String>) -> Vec<String> {
        let work = PAREN_RE.replace_all(s, " ");
        let work = QUOTE_RE.replace_all(&work, " ");
        let work = LIST_MARKER_RE.replace(&work, "");
        let mut work = LABEL_RE.replace(&work, "").into_owned();
        for g in &self.glossary.multi {
            work = replace_bounded(&work, g, "GLOSSARYTERM");
        }
        for mw in MULTI_WORD_PHRASES {
            work = replace_bounded(&work, mw, &mw.replace(' ', "_"));
        }
        let mut tokens = Vec::new();
        for (idx, m) in RAW_TOK_RE.find_iter(&work).enumerate() {
            let mut tok = m.as_str();
            if tok == "GLOSSARYTERM" {
                tokens.push("\0<term>".to_string());
                continue;
            }
            if let Some(auto) = autos.get(tok) {
                tokens.push(format!("\0{auto}"));
                continue;
            }
            // contractions are reported separately (4.2); possessives are
            // fine (GR-8)
            if tok.contains('\'') {
                let lt = lower(tok);
                if lt.ends_with("n't") || CONTRACTION_SET.contains(lt.as_str()) {
                    continue;
                }
                tok = tok.split('\'').next().unwrap_or("");
                if tok.is_empty() {
                    continue;
                }
            }
            let upper = is_upper_word(tok);
            if tok.len() > 1 && upper {
                // abbreviation / quoted label (8.6)
            } else if idx > 0
                && tok.as_bytes()[0].is_ascii_uppercase()
                && !upper
                && !tok.contains('_')
            {
                // capitalized mid-sentence: proper noun or product name
                // (TN category 11)
                tokens.push(format!("\0{}", lower(tok)));
            } else {
                tokens.push(lower(tok));
            }
        }
        tokens
    }

    fn check_sentence(&mut self, s: &str, pi: usize, n: usize) {
        let at = At {
            para: pi,
            sent: n,
            text: format!("para {pi}, sentence {n}"),
        };
        let mut short = restore_code(s, &self.code_spans);
        if rune_len(&short) > 90 {
            short = short.chars().take(87).collect::<String>() + "...";
        }
        // a typographic apostrophe is an apostrophe
        let (s, autos) = self.mark_auto(&s.replace('’', "'"));
        let (mode, limit, rule) = match self.sentence_mode(&s) {
            Mode::Procedural => ("procedural", 20, "5.1"),
            _ => ("descriptive", 25, "6.3"),
        };
        let wc = count_words(&s);
        if wc > limit {
            self.add(
                &at,
                rule,
                "length",
                format!("{wc} words ({mode}, max {limit}): \"{short}\""),
            );
        }
        if s.contains(';') {
            self.add(&at, "8.1", "punctuation", format!("Semicolon: \"{short}\""));
        }
        for m in contractions(&s) {
            // possessive 's is allowed (GR-8)
            let lm = lower(&m);
            if !lm.ends_with("'s") || CONTRACTION_SET.contains(lm.as_str()) {
                self.add(
                    &at,
                    "4.2",
                    "contraction",
                    format!("Contraction \"{m}\": write the words in full."),
                );
            }
        }
        let m = find_bounded(&s, &LATIN_ALTS, false);
        if !m.is_empty() {
            self.add(
                &at,
                "GR-6",
                "latin",
                format!(
                    "Latin abbreviation \"{m}\": use \"for example\", \"that is\", or rewrite."
                ),
            );
        }
        let m = find_bounded(&s, &GENDER_ALTS, true);
        if !m.is_empty() {
            self.add(&at, "GR-7", "pronoun", format!("Gender-specific pronoun \"{m}\": the rules do not permit he/she. Use \"the operator\", \"you\", \"they\"."));
        }

        let tokens = self.tokenize(&s, &autos);
        let mut prev = String::new();
        let mut run_nouns: Vec<String> = Vec::new();
        for (i, tok) in tokens.iter().enumerate() {
            let next = tokens.get(i + 1).map_or("", String::as_str);
            if let Some(text) = tok.strip_prefix('\0') {
                run_nouns.push(text.to_string());
                prev = lower(text);
                continue;
            }
            let t = tok.replace('_', " ");
            if t.contains(' ') {
                // multi-word approved phrase
                run_nouns.clear();
            } else if t.len() < 2 {
            } else if NUMBER_WORDS.contains(t.as_str()) {
                run_nouns.push(t.clone());
            } else if t == "been" || t == "being" {
                self.add(&at, "3.4", "tense", format!("\"{t}\": not an approved form of BE (only be, is, are, was, were). Rewrite without it."));
            } else {
                self.check_word(&t, &prev, next, i == 0, &short, &at, &mut run_nouns);
            }
            prev = t;
        }
        self.flush_nouns(&at, &mut run_nouns);
    }

    fn flush_nouns(&mut self, at: &At, run_nouns: &mut Vec<String>) {
        if run_nouns.len() >= 4 {
            let msg = format!(
                "Possible multi-word noun of {} words: \"{}\". Max 3 — use prepositions or hyphens (2.2).",
                run_nouns.len(),
                run_nouns.join(" ")
            );
            self.add(at, "2.1", "noun-cluster", msg);
        }
        run_nouns.clear();
    }

    /// The word-level checks on one token: approval, -ing, tenses, passive,
    /// noun clusters.
    #[allow(clippy::too_many_arguments)]
    fn check_word(
        &mut self,
        t: &str,
        prev: &str,
        next: &str,
        first: bool,
        example: &str,
        at: &At,
        run_nouns: &mut Vec<String>,
    ) {
        let (mut kind, base) = self.lookup_base(t);
        let entries = self.non_approved(&base);
        // part-of-speech heuristic: a word after a determiner/adjective is
        // used as a noun.
        let mut verb_only = false;
        if kind == "non_approved" {
            let nounish = self.approved_has_pos(&base, &["n"]) || listed_tn(entries);
            if DETERMINERS.contains(prev) {
                if all_verb(entries) {
                    kind = if nounish { "approved" } else { "unknown" };
                }
            } else if !first && !VERB_SLOT.contains(prev) {
                let mut looks_verb = ALWAYS_VERB.contains(base.as_str())
                    || has_any_suffix(&base, &["ize", "ise", "ify", "ate", "en"]);
                if !looks_verb && let Some(rest) = base.strip_prefix("re") {
                    looks_verb = self.d.approved.contains_key(rest);
                }
                verb_only =
                    all_verb(entries) && !looks_verb && (nounish || !self.is_function(prev));
            }
        }

        // -ing forms (3.5)
        if kind != "glossary" && ing_form(t) {
            if DETERMINERS.contains(prev)
                || (!next.is_empty()
                    && !self.is_function(next)
                    && !self.is_verb(next)
                    && !BE_FORMS.contains(next))
            {
                self.add(at, "3.5?", "ing", format!("\"-ing\" word \"{t}\": permitted only if it is a technical noun or a modifier in one (\"wiring diagram\"). Confirm, else rewrite."));
            } else {
                self.add(at, "3.5", "ing", format!("\"-ing\" verb form \"{t}\": not permitted. Rewrite with a simple tense (\"when you do\", not \"when doing\")."));
            }
        }

        // complex tenses (3.4) and passive (3.6)
        let participle = t.ends_with("ed") || IRREGULAR_PARTICIPLES.contains(t);
        if HAVE_FORMS.contains(prev) && participle && t != "need" {
            self.add(
                at,
                "3.4",
                "tense",
                format!(
                    "\"{prev} {t}\": perfect tense is not approved. Use the simple past or present."
                ),
            );
        }
        if BE_FORMS.contains(prev) && participle {
            if next == "by" {
                self.add(
                    at,
                    "3.6",
                    "passive",
                    format!("Passive voice \"{prev} {t} by\": make the agent the subject."),
                );
            } else {
                self.add(at, "3.6?", "passive", format!("\"{prev} {t}\": passive voice, or a past participle used as an adjective (permitted, 3.3)? Confirm. In procedures, use the imperative."));
            }
        }

        // word approval
        match kind {
            "non_approved" => {
                let alts: Vec<String> = entries
                    .iter()
                    .map(|e| {
                        let a = e.use_instead.join(", ");
                        if a.is_empty() { e.note.clone() } else { a }
                    })
                    .collect();
                let poses: Vec<&str> = entries.iter().map(|e| e.pos.as_str()).collect();
                let hk = if verb_only {
                    "maybe_noun"
                } else {
                    "non_approved"
                };
                self.hit(&base, hk, alts.join("; "), example, poses.join("/"));
            }
            "unknown" => self.hit(&base, "unknown", String::new(), example, String::new()),
            _ => {}
        }

        // multi-word noun heuristic (2.1): 4+ consecutive noun or adjective
        // words. A function word, a verb (approved, or non-approved and not
        // a possible noun here), or an -ly adverb ends the run.
        if self.is_function(&base)
            || self.is_verb(&base)
            || (kind == "non_approved" && all_verb(entries) && !verb_only)
            || self.is_adverb(t)
        {
            self.flush_nouns(at, run_nouns);
        } else {
            run_nouns.push(t.to_string());
        }
    }
}

/// An -ing word that can be a verb form: not "thing", "nothing", "string",
/// "bring", "ring".
pub(super) fn ing_form(t: &str) -> bool {
    t.ends_with("ing")
        && !APPROVED_ING.contains(t)
        && !t.ends_with("thing")
        && t.strip_suffix("ing")
            .unwrap_or(t)
            .contains(['a', 'e', 'i', 'o', 'u', 'y'])
}

fn all_verb(entries: &[NotApprovedEntry]) -> bool {
    entries.iter().all(|e| e.pos == "v")
}

fn listed_tn(entries: &[NotApprovedEntry]) -> bool {
    entries
        .iter()
        .any(|e| e.use_instead.iter().any(|a| a.ends_with("(TN)")))
}

/// Every word'suffix in `s` ("don't", "it's", "user's").
pub(super) fn contractions(s: &str) -> Vec<String> {
    let rs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < rs.len() {
        if !is_word_rune(rs[i]) || !boundary(&rs, i) {
            i += 1;
            continue;
        }
        let mut e = i;
        while e < rs.len() && is_word_rune(rs[e]) {
            e += 1;
        }
        for suf in ["t", "s", "re", "ve", "ll", "d", "m"] {
            if e < rs.len()
                && rs[e] == '\''
                && let Some(end) = match_at(&rs, e + 1, suf, true)
            {
                out.push(rs[i..end].iter().collect());
                break;
            }
        }
        i = e + 1;
    }
    out
}
