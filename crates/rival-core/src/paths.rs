//! Rival's state directory, lexical path handling, and `.env` files.
//!
//! The session and queue directories join `.rival/...` onto the home
//! directory and fall back to a relative `.rival` when the home directory is
//! unknown. `RIVAL_HOME`, when set, is the rival root itself (no `.rival`
//! appended).
//!
//! The `.env` reader is a direct port of `github.com/joho/godotenv` v1.5.1
//! (`parser.go`). It is used both at startup and for API-key lookup.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Captures, Regex};

const ROOT_DIR: &str = ".rival";

/// The variable that names the home directory.
pub const HOME_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// The Rust-port override for the rival root. Process environment only.
pub const STATE_ROOT_VAR: &str = "RIVAL_HOME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// `<home>/.rival`.
    pub fn from_home(home: &Path) -> Self {
        Paths {
            root: clean(&home.join(ROOT_DIR)),
        }
    }

    /// Reads `RIVAL_HOME`, then `HOME` (`USERPROFILE` on Windows).
    pub fn from_env() -> Self {
        Self::from_vars(std::env::var_os(STATE_ROOT_VAR), std::env::var_os(HOME_VAR))
    }

    /// Pure form of [`Paths::from_env`]. Empty values count as unset: an
    /// empty `$HOME` is no home directory.
    pub fn from_vars(rival_home: Option<OsString>, home: Option<OsString>) -> Self {
        if let Some(root) = rival_home.filter(|v| !v.is_empty()) {
            return Paths {
                root: PathBuf::from(root),
            };
        }
        match home.filter(|v| !v.is_empty()) {
            Some(home) => Self::from_home(Path::new(&home)),
            None => Paths {
                root: Path::new(".").join(ROOT_DIR),
            },
        }
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn queue_dir(&self) -> PathBuf {
        self.root.join("queue")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.yaml")
    }

    /// The model check's logs and its empty work directory.
    pub fn check_dir(&self) -> PathBuf {
        self.root.join("check")
    }

    /// The default proxy key file.
    pub fn proxy_key_file(&self) -> PathBuf {
        self.root.join("proxy.key")
    }
}

/// Lexical clean for the host: Unix rules, or on Windows [`clean_windows`].
pub fn clean(path: &Path) -> PathBuf {
    #[cfg(windows)]
    return clean_windows(path);
    #[cfg(not(windows))]
    return clean_unix(path);
}

/// Lexical clean from the [`std::path`] components: drops `.` and empty
/// elements, and removes a name before each `..`. A `..` stays at the start
/// of a relative path and goes away after a root. The empty result is `.`.
///
/// A name that has a `:` (not valid in a Windows file name) can become a
/// drive prefix when it is the first element, for example `a\..\c:`.
#[cfg(windows)]
fn clean_windows(path: &Path) -> PathBuf {
    use std::path::Component;
    let rooted = path.has_root();
    let mut parts: Vec<Component<'_>> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                } else if !rooted {
                    parts.push(c);
                }
            }
            _ => parts.push(c),
        }
    }
    let mut out = PathBuf::new();
    for c in parts {
        match c {
            // The prefix keeps its spelling in std (`//host/share`). Write it
            // with backslashes, as every other separator in the result.
            Component::Prefix(p) => {
                use std::os::windows::ffi::{OsStrExt, OsStringExt};
                let wide: Vec<u16> = p
                    .as_os_str()
                    .encode_wide()
                    .map(|u| {
                        if u == u16::from(b'/') {
                            u16::from(b'\\')
                        } else {
                            u
                        }
                    })
                    .collect();
                out.push(std::ffi::OsString::from_wide(&wide));
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    out
}

/// Joins two elements for the host and cleans the result. Empty elements
/// are dropped. Unlike [`Path::join`], an absolute `b` does not replace `a`.
pub fn join(a: &Path, b: &Path) -> PathBuf {
    match (a.as_os_str().is_empty(), b.as_os_str().is_empty()) {
        (true, true) => PathBuf::new(),
        (true, false) => clean(b),
        (false, true) => clean(a),
        (false, false) => {
            // `a.join("")` adds a separator only where `Path::push` would:
            // a Windows drive alone (`C:`) stays drive-relative. `b` is
            // appended as text, so its root does not replace `a`.
            let mut joined = a.join("").into_os_string();
            joined.push(b);
            clean(Path::new(&joined))
        }
    }
}

/// An absolute path for the host: [`Path::is_absolute`] on Windows (a drive
/// with a root, UNC or `\\?\`), a leading `/` on Unix.
pub fn is_abs(path: &Path) -> bool {
    #[cfg(windows)]
    return path.is_absolute();
    #[cfg(not(windows))]
    return path.as_os_str().as_encoded_bytes().first() == Some(&b'/');
}

/// Lexical clean with Unix rules on every platform. A backslash is an
/// ordinary byte here.
pub fn clean_unix(path: &Path) -> PathBuf {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.is_empty() {
        return PathBuf::from(".");
    }
    let rooted = bytes[0] == b'/';
    let mut parts: Vec<&[u8]> = Vec::new();
    for elem in bytes.split(|&b| b == b'/') {
        match elem {
            b"" | b"." => {}
            b".." => {
                if parts.last().is_some_and(|last| *last != b"..") {
                    parts.pop();
                } else if !rooted {
                    parts.push(elem);
                }
            }
            _ => parts.push(elem),
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    if rooted {
        out.push(b'/');
    }
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    // SAFETY: `out` is pieces of a valid OsStr encoding split and joined only
    // at ASCII '/' bytes, plus ASCII '/' and '.'.
    PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(out) })
}

/// An absolute, cleaned path. `cwd` is the working directory snapshot;
/// `None` stands for a failed lookup, which makes a relative path fail.
///
/// On Windows a relative path joins `cwd` and then [`std::path::absolute`]
/// resolves it. That covers a root-relative `\x` (the drive of `cwd`) and a
/// drive-relative `D:x`, which the OS resolves from that drive's own
/// directory of this process.
pub fn abs(cwd: Option<&Path>, path: &Path) -> Option<PathBuf> {
    if is_abs(path) {
        return Some(clean(path));
    }
    let joined = cwd?.join(path);
    #[cfg(windows)]
    let joined = std::path::absolute(&joined).ok()?;
    Some(clean(&joined))
}

/// The current directory on Unix. An absolute `$PWD` naming the current directory
/// wins (so symlinked paths stay as the shell spelled them); otherwise the
/// kernel's answer.
pub fn getwd(pwd: Option<&OsStr>) -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(pwd) = pwd.filter(|p| p.as_encoded_bytes().first() == Some(&b'/')) {
        use std::os::unix::fs::MetadataExt;
        let dot = std::fs::metadata(".").ok()?;
        if let Ok(d) = std::fs::metadata(pwd)
            && d.dev() == dot.dev()
            && d.ino() == dot.ino()
        {
            return Some(PathBuf::from(pwd));
        }
    }
    #[cfg(not(unix))]
    let _ = pwd;
    std::env::current_dir().ok()
}

/// A `.env` file that could not be read or parsed.
#[derive(Debug)]
pub enum DotenvError {
    Io(std::io::Error),
    Parse(String),
}

impl fmt::Display for DotenvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DotenvError::Io(e) => e.fmt(f),
            DotenvError::Parse(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for DotenvError {}

/// Reads a `.env` file. The whole file parses or nothing is returned.
/// A key assigned twice keeps its last value. `$VAR` expansion sees only
/// keys assigned earlier in the same file, never the process environment.
pub fn read_dotenv(path: &Path) -> Result<HashMap<String, String>, DotenvError> {
    let data = std::fs::read(path).map_err(DotenvError::Io)?;
    parse_dotenv(&String::from_utf8_lossy(&data))
}

/// Parses `.env` text.
pub fn parse_dotenv(src: &str) -> Result<HashMap<String, String>, DotenvError> {
    let mut out = HashMap::new();
    parse_into(src, &mut out).map_err(DotenvError::Parse)?;
    Ok(out)
}

/// Loads `./.env` into the process environment. Reads `./.env` only (no parent search); a
/// missing or unparseable file is silently ignored, and existing process
/// variables win, even when empty.
///
/// `.env` never sets [`STATE_ROOT_VAR`]. The file belongs to the reviewed
/// repository, so it must not move rival's state. Only the process
/// environment can set it.
///
/// # Safety
///
/// Calls `std::env::set_var`. The caller must guarantee that no other thread
/// exists yet and that nothing reads the environment concurrently: call it
/// first thing in `main`.
pub unsafe fn load_dotenv() {
    load_dotenv_with(
        Path::new(".env"),
        |key| std::env::var_os(key).is_some(),
        // SAFETY: the caller upholds this function's single-thread contract.
        |key, value| unsafe { std::env::set_var(key, value) },
    );
}

/// Testable core of [`load_dotenv`]: existing variables are not overridden.
/// `is_set` answers for the environment as it was before loading. Entries the
/// OS would reject (empty key, `=` or NUL in the key, NUL in the value) are
/// skipped silently.
pub fn load_dotenv_with(path: &Path, is_set: impl Fn(&str) -> bool, set: impl FnMut(&str, &str)) {
    load_dotenv_for(path, cfg!(windows), is_set, set);
}

/// [`load_dotenv_with`] with the host's environment-name rules as a
/// parameter, so both are testable everywhere.
fn load_dotenv_for(
    path: &Path,
    case_insensitive: bool,
    is_set: impl Fn(&str) -> bool,
    mut set: impl FnMut(&str, &str),
) {
    let Ok(vars) = read_dotenv(path) else {
        return;
    };
    for (key, value) in &vars {
        if key.is_empty()
            || key.contains(['=', '\0'])
            || value.contains('\0')
            || is_state_root_var(key, case_insensitive)
        {
            continue;
        }
        if !is_set(key) {
            set(key, value);
        }
    }
}

/// Whether `key` names [`STATE_ROOT_VAR`]. Windows environment names are
/// case-insensitive, so every spelling the OS treats as that name matches
/// there (see [`crate::envname`]).
pub(crate) fn is_state_root_var(key: &str, case_insensitive: bool) -> bool {
    crate::envname::eq(case_insensitive, OsStr::new(key), STATE_ROOT_VAR)
}

fn parse_into(src: &str, out: &mut HashMap<String, String>) -> Result<(), String> {
    let src = src.replace("\r\n", "\n");
    let mut cutset: &str = &src;
    while let Some(start) = statement_start(cutset) {
        let (key, left) = locate_key_name(start)?;
        let (value, left) = extract_var_value(left, out)?;
        out.insert(key, value);
        cutset = left;
    }
    Ok(())
}

/// godotenv `isSpace`: a space but not a line break.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\x0b' | '\x0c' | '\r' | ' ' | '\u{85}' | '\u{a0}')
}

/// godotenv `getStatementStart`: skips blank space and comment lines.
fn statement_start(mut src: &str) -> Option<&str> {
    loop {
        let pos = src.find(|c: char| !c.is_whitespace())?;
        src = &src[pos..];
        if !src.starts_with('#') {
            return Some(src);
        }
        src = &src[src.find('\n')?..];
    }
}

/// godotenv `locateKeyName`.
fn locate_key_name(src: &str) -> Result<(String, &str), String> {
    let mut src = src.trim_start_matches(is_space);
    if let Some(trimmed) = src.strip_prefix("export")
        && trimmed.chars().next().is_some_and(is_space)
    {
        src = trimmed.trim_start_matches(is_space);
    }

    let mut key = "";
    let mut offset = 0;
    // Ranges over bytes and widens each byte to a char, not over UTF-8.
    for (i, &b) in src.as_bytes().iter().enumerate() {
        let c = char::from(b);
        if is_space(c) {
            continue;
        }
        match b {
            b'=' | b':' => {
                key = &src[..i];
                offset = i + 1;
                break;
            }
            b'_' => {}
            _ => {
                if c.is_alphabetic() || c.is_numeric() || c == '.' {
                    continue;
                }
                return Err(format!(
                    "unexpected character {:?} in variable name near {:?}",
                    c.to_string(),
                    src
                ));
            }
        }
    }
    if src.is_empty() {
        return Err("zero length string".to_string());
    }
    let key = key.trim_end_matches(char::is_whitespace).to_string();
    Ok((key, src[offset..].trim_start_matches(is_space)))
}

/// godotenv `extractVarValue`.
fn extract_var_value<'a>(
    src: &'a str,
    vars: &HashMap<String, String>,
) -> Result<(String, &'a str), String> {
    let quote = match src.as_bytes().first() {
        Some(&q @ (b'"' | b'\'')) => q,
        _ => {
            let end_of_line = match src.find(['\n', '\r']) {
                Some(end) => end,
                None if src.is_empty() => return Ok((String::new(), "")),
                None => src.len(),
            };
            let line: Vec<char> = src[..end_of_line].chars().collect();
            let mut end_of_var = line.len();
            if end_of_var == 0 {
                return Ok((String::new(), &src[end_of_line..]));
            }
            // The last " #" starts an inline comment.
            for i in (0..end_of_var).rev() {
                if line[i] == '#' && i > 0 && is_space(line[i - 1]) {
                    end_of_var = i;
                    break;
                }
            }
            let value: String = line[..end_of_var].iter().collect();
            let value = expand_variables(value.trim_matches(is_space), vars);
            return Ok((value, &src[end_of_line..]));
        }
    };

    let bytes = src.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] != quote || bytes[i - 1] == b'\\' {
            continue;
        }
        let q = char::from(quote);
        let inner = src[..i].trim_end_matches(q).trim_start_matches(q);
        let value = if quote == b'"' {
            expand_variables(&expand_escapes(inner), vars)
        } else {
            inner.to_string()
        };
        return Ok((value, &src[i + 1..]));
    }

    let end = src.find('\n').unwrap_or(src.len());
    Err(format!("unterminated quoted value {}", &src[..end]))
}

static ESCAPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\.").unwrap());
static UNESCAPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([^$])").unwrap());
static EXPAND_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\\)?(\$)(\()?\{?([A-Z0-9_]+)?\}?").unwrap());

/// godotenv `expandEscapes`: `\n` and `\r` become control characters, then
/// any other backslash pair except `\$` drops its backslash.
fn expand_escapes(s: &str) -> String {
    let out = ESCAPE_RE.replace_all(s, |caps: &Captures| match &caps[0][1..] {
        "n" => "\n".to_string(),
        "r" => "\r".to_string(),
        _ => caps[0].to_string(),
    });
    UNESCAPE_RE.replace_all(&out, "${1}").into_owned()
}

/// godotenv `expandVariables`. The original also tests `submatch[2] == "("`,
/// which can never hold (group 2 is always `$`), so `$(NAME` expands like `$NAME`.
fn expand_variables(v: &str, vars: &HashMap<String, String>) -> String {
    EXPAND_VAR_RE
        .replace_all(v, |caps: &Captures| {
            if caps.get(1).is_some() {
                return caps[0][1..].to_string();
            }
            match caps.get(4) {
                Some(name) => vars.get(name.as_str()).cloned().unwrap_or_default(),
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_vars_picks_root() {
        let cases = [
            (
                "RIVAL_HOME is the root itself",
                Some("/r"),
                Some("/h"),
                "/r",
            ),
            ("HOME gets .rival", None, Some("/h"), "/h/.rival"),
            (
                "empty RIVAL_HOME falls to HOME",
                Some(""),
                Some("/h"),
                "/h/.rival",
            ),
            ("no home: relative .rival", None, None, "./.rival"),
            ("empty HOME: relative .rival", None, Some(""), "./.rival"),
        ];
        for (name, rival_home, home, want) in cases {
            let got = Paths::from_vars(rival_home.map(OsString::from), home.map(OsString::from));
            assert_eq!(got.root, PathBuf::from(want), "{name}");
        }
    }

    #[test]
    fn subpaths_use_the_rival_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::from_home(tmp.path());
        assert_eq!(p.sessions_dir(), tmp.path().join(".rival/sessions"));
        assert_eq!(p.queue_dir(), tmp.path().join(".rival/queue"));
        assert_eq!(p.config_file(), tmp.path().join(".rival/config.yaml"));
    }

    // Lexical clean cases for Unix paths.
    #[test]
    fn clean_resolves_dots_and_separators() {
        let cases = [
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("", "."),
            ("abc/", "abc"),
            ("/abc/def/", "/abc/def"),
            ("//abc", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ];
        for (input, want) in cases {
            assert_eq!(
                clean_unix(Path::new(input)).into_os_string(),
                OsString::from(want),
                "{input:?}"
            );
        }
        // A backslash is an ordinary byte under Unix rules.
        assert_eq!(
            clean_unix(Path::new(r"a\b/../c\.\d")).into_os_string(),
            OsString::from(r"c\.\d")
        );
    }

    /// The host's `clean`, `join` and `is_abs` follow the host's rules.
    #[test]
    fn host_rules_dispatch_by_platform() {
        let got = |p: &str| clean(Path::new(p)).into_os_string();
        let joined = |a: &str, b: &str| join(Path::new(a), Path::new(b)).into_os_string();
        if cfg!(windows) {
            assert_eq!(got(r"C:\a\..\b/c"), OsString::from(r"C:\b\c"));
            assert_eq!(got(r"\\host\share\x\.."), OsString::from(r"\\host\share\"));
            assert_eq!(joined("C:", "f"), OsString::from("C:f"));
            assert!(is_abs(Path::new(r"C:\x")) && !is_abs(Path::new(r"\x")));
        } else {
            // `a\b` is one element.
            assert_eq!(got(r"a\b/../c"), OsString::from("c"));
            assert_eq!(got(r"x/a\b/./c"), OsString::from(r"x/a\b/c"));
            assert_eq!(joined("/a", "../b"), OsString::from("/b"));
            assert_eq!(joined("", ""), OsString::new());
            assert!(is_abs(Path::new("/x")) && !is_abs(Path::new(r"C:\x")));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn abs_joins_cwd_and_cleans() {
        let cwd = Path::new("/work/dir");
        assert_eq!(
            abs(Some(cwd), Path::new("sub/../x")),
            Some("/work/dir/x".into())
        );
        assert_eq!(abs(Some(cwd), Path::new("")), Some("/work/dir".into()));
        assert_eq!(abs(Some(cwd), Path::new("/a/./b/")), Some("/a/b".into()));
        assert_eq!(abs(None, Path::new("/a")), Some("/a".into()));
        assert_eq!(abs(None, Path::new("rel")), None);
    }

    /// The portable clean cases, as Windows runs them: `/` becomes `\`.
    #[cfg(windows)]
    const CLEAN_PORTABLE: &[(&str, &str)] = &[
        ("abc", "abc"),
        ("abc/def", "abc/def"),
        ("a/b/c", "a/b/c"),
        (".", "."),
        ("..", ".."),
        ("../..", "../.."),
        ("../../abc", "../../abc"),
        ("/abc", "/abc"),
        ("/", "/"),
        ("", "."),
        ("abc/", "abc"),
        ("abc/def/", "abc/def"),
        ("a/b/c/", "a/b/c"),
        ("./", "."),
        ("../", ".."),
        ("../../", "../.."),
        ("/abc/", "/abc"),
        ("abc//def//ghi", "abc/def/ghi"),
        ("abc//", "abc"),
        ("abc/./def", "abc/def"),
        ("/./abc/def", "/abc/def"),
        ("abc/.", "abc"),
        ("abc/def/ghi/../jkl", "abc/def/jkl"),
        ("abc/def/../ghi/../jkl", "abc/jkl"),
        ("abc/def/..", "abc"),
        ("abc/def/../..", "."),
        ("/abc/def/../..", "/"),
        ("abc/def/../../..", ".."),
        ("/abc/def/../../..", "/"),
        ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
        ("/../abc", "/abc"),
        ("a/../b:/../../c", "../c"),
        ("abc/./../def", "def"),
        ("abc//./../def", "def"),
        ("abc/../../././../def", "../../def"),
    ];

    /// Windows clean cases: drives, UNC shares and device paths.
    #[cfg(windows)]
    const CLEAN_WINDOWS: &[(&str, &str)] = &[
        // A drive alone stays as it is (was `c:.`).
        (r"c:", r"c:"),
        (r"c:\", r"c:\"),
        (r"c:\abc", r"c:\abc"),
        (r"c:abc\..\..\.\.\..\def", r"c:..\..\def"),
        (r"c:\abc\def\..\..", r"c:\"),
        (r"c:\..\abc", r"c:\abc"),
        (r"c:..\abc", r"c:..\abc"),
        (r"c:\b:\..\..\..\d", r"c:\d"),
        (r"\", r"\"),
        (r"/", r"\"),
        (r"\\i\..\c$", r"\\i\..\c$"),
        (r"\\i\..\i\c$", r"\\i\..\i\c$"),
        (r"\\i\..\I\c$", r"\\i\..\I\c$"),
        (r"\\host\share\foo\..\bar", r"\\host\share\bar"),
        // The prefix is written with backslashes.
        (r"//host/share/foo/../baz", r"\\host\share\baz"),
        (r"\\host\share\foo\..\..\..\..\bar", r"\\host\share\bar"),
        (r"\\.\C:\a\..\..\..\..\bar", r"\\.\C:\bar"),
        (r"\\.\C:\\\\a", r"\\.\C:\a"),
        (r"\\a\b\..\c", r"\\a\b\c"),
        // std reads a UNC share as a prefix plus its root, so the clean form
        // ends with the root separator.
        (r"\\a\b", r"\\a\b\"),
        // A first element with a `:` is a drive for std (were `.\c:`,
        // `.\c:\foo` and `.\c:foo`).
        (r".\c:", r"c:"),
        (r".\c:\foo", r"c:foo"),
        (r".\c:foo", r"c:foo"),
        // Without a share, `//abc` is not UNC for std: one root (were
        // `\\abc`, `\\\abc` and `\\abc\\`).
        (r"//abc", r"\abc"),
        (r"///abc", r"\abc"),
        (r"//abc//", r"\abc"),
        (r"\\?\C:\", r"\\?\C:\"),
        (r"\\?\C:\a", r"\\?\C:\a"),
        // A first element with a `:` is a drive for std (were `.\c:`,
        // `.\c:`, `.\c:\a` and `..\c:`).
        (r"a/../c:", r"c:"),
        (r"a\..\c:", r"c:"),
        (r"a/../c:/a", r"c:a"),
        (r"a/../../c:", r"c:"),
        (r"foo:bar", r"foo:bar"),
        // std has no `\??\` prefix, so nothing is inserted (was `\.\??\a`).
        (r"/a/../??/a", r"\??\a"),
    ];

    #[cfg(windows)]
    #[test]
    fn windows_clean_folds_std_components() {
        let got = |p: &str| clean(Path::new(p)).into_os_string();
        // Collect every mismatch, so one run shows all of them.
        let mut bad = Vec::new();
        let mut check = |input: &str, want: &str| {
            for i in [input, want] {
                let g = got(i);
                if g.as_os_str() != std::ffi::OsStr::new(want) {
                    bad.push(format!("clean({i:?}) = {g:?}, want {want:?}"));
                }
            }
        };
        for &(input, want) in CLEAN_PORTABLE {
            check(input, &want.replace('/', r"\"));
        }
        for &(input, want) in CLEAN_WINDOWS {
            check(input, want);
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Several elements join two at a time, as callers do.
    #[cfg(windows)]
    #[test]
    fn windows_join_appends_and_cleans() {
        let cases: &[(&[&str], &str)] = &[
            (&[], ""),
            (&[""], ""),
            (&["/"], "/"),
            (&["a"], "a"),
            (&["a", "b"], "a/b"),
            (&["a", ""], "a"),
            (&["", "b"], "b"),
            (&["/", "a"], "/a"),
            (&["/", "a/b"], "/a/b"),
            (&["/", ""], "/"),
            (&["/a", "b"], "/a/b"),
            (&["a", "/b"], "a/b"),
            (&["/a", "/b"], "/a/b"),
            (&["a/", "b"], "a/b"),
            (&["a/", ""], "a"),
            (&["", ""], ""),
            (&["/", "a", "b"], "/a/b"),
            (&["directory", "file"], r"directory\file"),
            (&[r"C:\Windows\", "System32"], r"C:\Windows\System32"),
            (&[r"C:\Windows\", ""], r"C:\Windows"),
            (&[r"C:\", "Windows"], r"C:\Windows"),
            (&["C:", "a"], "C:a"),
            (&["C:", r"a\b"], r"C:a\b"),
            (&["C:", "a", "b"], r"C:a\b"),
            (&["C:", "", "b"], "C:b"),
            (&["C:", "", "", "b"], "C:b"),
            // A drive alone stays as it is (were `C:.`).
            (&["C:", ""], "C:"),
            (&["C:", "", ""], "C:"),
            (&["C:", r"\a"], r"C:\a"),
            (&["C:", "", r"\a"], r"C:\a"),
            (&["C:.", "a"], "C:a"),
            (&["C:a", "b"], r"C:a\b"),
            (&["C:a", "b", "d"], r"C:a\b\d"),
            (&[r"\\host\share", "foo"], r"\\host\share\foo"),
            (&[r"\\host\share\foo"], r"\\host\share\foo"),
            // std keeps the spelling of the UNC prefix (was
            // `\\host\share\foo\bar`).
            (&["//host/share", "foo/bar"], r"//host/share\foo\bar"),
            (&[r"\"], r"\"),
            (&[r"\", ""], r"\"),
            (&[r"\", "a"], r"\a"),
            // `\\` and `\\a` without a share are not UNC for std, and each
            // pair is cleaned (were `\\a`, `\\a\b`, `\\a\b\c`, `\\a\b\c`
            // and `\\a`).
            (&[r"\\", "a"], r"\a"),
            (&[r"\", "a", "b"], r"\a\b"),
            (&[r"\\", "a", "b"], r"\a\b"),
            (&[r"\", r"\\a\b", "c"], r"\a\b\c"),
            (&[r"\\a", "b", "c"], r"\a\b\c"),
            (&[r"\\a\", "b", "c"], r"\a\b\c"),
            (&["//", "a"], r"\a"),
            (&[r"a:\b\c", r"x\..\y:\..\..\z"], r"a:\b\z"),
            // std has no `\??\` prefix, so nothing is inserted (was
            // `\.\??\a`).
            (&[r"\", r"??\a"], r"\??\a"),
        ];
        let mut bad = Vec::new();
        for &(elems, want) in cases {
            let got = elems
                .iter()
                .fold(PathBuf::new(), |acc, e| join(&acc, Path::new(e)))
                .into_os_string();
            let want = want.replace('/', r"\");
            if got.as_os_str() != std::ffi::OsStr::new(&want) {
                bad.push(format!("join({elems:?}) = {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_is_abs_is_std_is_absolute() {
        let cases = [
            (r"C:\", true),
            (r"c\", false),
            (r"c::", false),
            (r"c:", false),
            (r"/", false),
            (r"\", false),
            (r"\Windows", false),
            (r"c:a\b", false),
            (r"c:\a\b", true),
            (r"c:/a/b", true),
            (r"\\host\share", true),
            (r"\\host\share\", true),
            (r"\\host\share\foo", true),
            (r"//host/share/foo/bar", true),
            (r"\\?\a\b\c", true),
            // std has no `\??\` prefix: root-relative (was true).
            (r"\??\a\b\c", false),
        ];
        for (path, want) in cases {
            assert_eq!(is_abs(Path::new(path)), want, "is_abs({path:?})");
        }
        // Unix cases are never absolute without a drive, and keep their
        // Unix answer with a `c:` prefix.
        let unix = [
            ("", false),
            ("/", true),
            ("/usr/bin/gcc", true),
            ("..", false),
            ("/a/../bb", true),
            (".", false),
            ("./", false),
            ("lala", false),
        ];
        for (path, want) in unix {
            assert!(!is_abs(Path::new(path)), "is_abs({path:?})");
            let prefixed = format!("c:{path}");
            assert_eq!(is_abs(Path::new(&prefixed)), want, "is_abs({prefixed:?})");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_abs_covers_drive_root_unc_and_relative_paths() {
        let cwd = Path::new(r"C:\work\dir");
        let abs = |p: &str| abs(Some(cwd), Path::new(p)).map(PathBuf::into_os_string);
        assert_eq!(abs(r"D:\a\.\b\"), Some(r"D:\a\b".into()), "drive-rooted");
        assert_eq!(abs(r"sub\..\x"), Some(r"C:\work\dir\x".into()), "relative");
        assert_eq!(abs(""), Some(r"C:\work\dir".into()), "empty is cwd");
        assert_eq!(abs(r"\top\x"), Some(r"C:\top\x".into()), "root-relative");
        assert_eq!(
            abs("/top/x"),
            Some(r"C:\top\x".into()),
            "slash root-relative"
        );
        assert_eq!(
            abs(r"\\host\share\a\..\b"),
            Some(r"\\host\share\b".into()),
            "UNC"
        );
        // A drive-relative path takes the drive's own directory of this
        // process from the OS, also on the drive of `cwd` (was
        // `C:\work\dir\rel` for `c:rel`).
        for p in [r"c:rel", r"D:rel"] {
            let os = clean(&std::path::absolute(p).unwrap()).into_os_string();
            assert_eq!(abs(p), Some(os), "drive-relative {p:?}");
        }
        // No working directory: only absolute paths resolve.
        assert_eq!(super::abs(None, Path::new("rel")), None);
        assert_eq!(super::abs(None, Path::new(r"\x")), None);
        assert_eq!(super::abs(None, Path::new(r"c:rel")), None);
        assert_eq!(
            super::abs(None, Path::new(r"C:\x")).map(PathBuf::into_os_string),
            Some(r"C:\x".into())
        );
    }

    #[test]
    fn getwd_prefers_a_matching_pwd() {
        let real = std::env::current_dir().unwrap();
        assert_eq!(getwd(Some(real.as_os_str())), Some(real.clone()));
        // A relative or wrong PWD falls back to the kernel's answer.
        assert_eq!(getwd(Some(OsStr::new("relative"))), Some(real.clone()));
        let other = tempfile::tempdir().unwrap();
        assert_eq!(getwd(Some(other.path().as_os_str())), Some(real.clone()));
        assert_eq!(getwd(None), Some(real));
    }

    fn parsed(src: &str) -> HashMap<String, String> {
        parse_dotenv(src).unwrap_or_else(|e| panic!("parse {src:?}: {e}"))
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // godotenv_test.go TestParsing, TestTrailingNewlines and the fixtures.
    #[test]
    fn dotenv_parses_like_godotenv() {
        let cases: &[(&str, &str, &str)] = &[
            ("FOO=bar", "FOO", "bar"),
            ("FOO =bar", "FOO", "bar"),
            ("FOO= bar", "FOO", "bar"),
            (r#"FOO="bar""#, "FOO", "bar"),
            ("FOO='bar'", "FOO", "bar"),
            (r#"FOO="escaped\"bar""#, "FOO", r#"escaped"bar"#),
            (r#"FOO="'d'""#, "FOO", "'d'"),
            ("OPTION_A: 1", "OPTION_A", "1"),
            ("OPTION_A: Foo=bar", "OPTION_A", "Foo=bar"),
            ("OPTION_A=1:B", "OPTION_A", "1:B"),
            ("export OPTION_A=2", "OPTION_A", "2"),
            (r"export OPTION_B='\n'", "OPTION_B", r"\n"),
            ("export exportFoo=2", "exportFoo", "2"),
            ("exportFOO=2", "exportFOO", "2"),
            ("export_FOO =2", "export_FOO", "2"),
            ("export.FOO= 2", "export.FOO", "2"),
            ("export\tOPTION_A=2", "OPTION_A", "2"),
            ("  export OPTION_A=2", "OPTION_A", "2"),
            ("\texport OPTION_A=2", "OPTION_A", "2"),
            (r#"FOO="bar\nbaz""#, "FOO", "bar\nbaz"),
            ("FOO.BAR=foobar", "FOO.BAR", "foobar"),
            ("FOO=foobar=", "FOO", "foobar="),
            ("FOO=bar ", "FOO", "bar"),
            ("KEY=value value", "KEY", "value value"),
            ("KEY=value value\n", "KEY", "value value"),
            ("KEY: value value", "KEY", "value value"),
            ("KEY: value value\n", "KEY", "value value"),
            ("FOO=bar # this is foo", "FOO", "bar"),
            (r#"FOO="bar#baz" # comment"#, "FOO", "bar#baz"),
            ("FOO='bar#baz' # comment", "FOO", "bar#baz"),
            (r#"FOO="bar#baz#bang" # comment"#, "FOO", "bar#baz#bang"),
            (r#"FOO="ba#r""#, "FOO", "ba#r"),
            ("FOO='ba#r'", "FOO", "ba#r"),
            (r#"FOO="bar\n\ b\az""#, "FOO", "bar\n baz"),
            (r#"FOO="bar\\\n\ b\az""#, "FOO", "bar\\\n baz"),
            (r#"FOO="bar\\r\ b\az""#, "FOO", "bar\\r baz"),
            (r#"="value""#, "", "value"),
            (" KEY =value", "KEY", "value"),
            ("   KEY=value", "KEY", "value"),
            ("\tKEY=value", "KEY", "value"),
            // Unquoted spaces inside a key are skipped, not an error.
            ("MY KEY=v", "MY KEY", "v"),
            // Only a space before '#' starts a comment; the last one wins.
            ("bar=foo#baz", "bar", "foo#baz"),
            ("A=x #y #z", "A", "x #y"),
            (r##"baz="foo"#bar"##, "baz", "foo"),
            ("WIN=crlf\r\n", "WIN", "crlf"),
        ];
        for &(input, key, want) in cases {
            let got = parsed(input);
            assert_eq!(
                got.get(key).map(String::as_str),
                Some(want),
                "{input:?} -> {got:?}"
            );
        }
        assert!(parse_dotenv("lol$wut").is_err());
    }

    #[test]
    fn dotenv_fixtures_match_godotenv() {
        let plain = "OPTION_A=1\nOPTION_B=2\nOPTION_C= 3\nOPTION_D =4\nOPTION_E = 5\nOPTION_F = \nOPTION_G=\nOPTION_H=1 2";
        assert_eq!(
            parsed(plain),
            map(&[
                ("OPTION_A", "1"),
                ("OPTION_B", "2"),
                ("OPTION_C", "3"),
                ("OPTION_D", "4"),
                ("OPTION_E", "5"),
                ("OPTION_F", ""),
                ("OPTION_G", ""),
                ("OPTION_H", "1 2"),
            ])
        );
        let quoted = "OPTION_A='1'\nOPTION_B='2'\nOPTION_C=''\nOPTION_D='\\n'\nOPTION_E=\"1\"\nOPTION_F=\"2\"\nOPTION_G=\"\"\nOPTION_H=\"\\n\"\nOPTION_I = \"echo 'asd'\"\nOPTION_J='line 1\nline 2'\nOPTION_K='line one\nthis is \\'quoted\\'\none more line'\nOPTION_L=\"line 1\nline 2\"\nOPTION_M=\"line one\nthis is \\\"quoted\\\"\none more line\"\n";
        assert_eq!(
            parsed(quoted),
            map(&[
                ("OPTION_A", "1"),
                ("OPTION_B", "2"),
                ("OPTION_C", ""),
                ("OPTION_D", "\\n"),
                ("OPTION_E", "1"),
                ("OPTION_F", "2"),
                ("OPTION_G", ""),
                ("OPTION_H", "\n"),
                ("OPTION_I", "echo 'asd'"),
                ("OPTION_J", "line 1\nline 2"),
                ("OPTION_K", "line one\nthis is \\'quoted\\'\none more line"),
                ("OPTION_L", "line 1\nline 2"),
                ("OPTION_M", "line one\nthis is \"quoted\"\none more line"),
            ])
        );
        let comments = "# Full line comment\nfoo=bar # baz\nbar=foo#baz\nbaz=\"foo\"#bar\n";
        assert_eq!(
            parsed(comments),
            map(&[("foo", "bar"), ("bar", "foo#baz"), ("baz", "foo")])
        );
        let equals = "export OPTION_A='postgres://localhost:5432/database?sslmode=disable'\n";
        assert_eq!(
            parsed(equals),
            map(&[(
                "OPTION_A",
                "postgres://localhost:5432/database?sslmode=disable"
            )])
        );
        let url = "TEST_URLS=\"stratum+tcp://stratum.antpool.com:3333\nstratum+tcp://stratum.antpool.com:443\"";
        assert_eq!(
            parsed(url),
            map(&[(
                "TEST_URLS",
                "stratum+tcp://stratum.antpool.com:3333\nstratum+tcp://stratum.antpool.com:443"
            )])
        );
        assert!(parse_dotenv("INVALID LINE\nfoo=bar\n").is_err());
    }

    // godotenv_test.go TestSubstitutions and TestExpanding.
    #[test]
    fn dotenv_expands_only_earlier_file_keys() {
        let subst = "OPTION_A=1\nOPTION_B=${OPTION_A}\nOPTION_C=$OPTION_B\nOPTION_D=${OPTION_A}${OPTION_B}\nOPTION_E=${OPTION_NOT_DEFINED}\n";
        assert_eq!(
            parsed(subst),
            map(&[
                ("OPTION_A", "1"),
                ("OPTION_B", "1"),
                ("OPTION_C", "1"),
                ("OPTION_D", "11"),
                ("OPTION_E", ""),
            ])
        );
        let cases: &[(&str, &[(&str, &str)])] = &[
            ("FOO=test\nBAR=$FOO", &[("FOO", "test"), ("BAR", "test")]),
            (
                "FOO=test\nBAR=${FOO}bar",
                &[("FOO", "test"), ("BAR", "testbar")],
            ),
            ("BAR=$FOO", &[("BAR", "")]),
            (
                "FOO=test\nBAR=\"quote $FOO\"",
                &[("FOO", "test"), ("BAR", "quote test")],
            ),
            ("BAR='quote $FOO'", &[("BAR", "quote $FOO")]),
            (r#"FOO="foo\$BAR""#, &[("FOO", "foo$BAR")]),
            (r#"FOO="foo\${BAR}""#, &[("FOO", "foo${BAR}")]),
            (
                "FOO=test\nBAR=\"foo\\${FOO} ${FOO}\"",
                &[("FOO", "test"), ("BAR", "foo${FOO} test")],
            ),
            // Only [A-Z0-9_] names expand; an underscore name is one name.
            ("A_B=1\nX=$A_B", &[("A_B", "1"), ("X", "1")]),
            ("lower=1\nX=$lower", &[("lower", "1"), ("X", "$lower")]),
            ("X=${lower}", &[("X", "${lower}")]),
            ("X=$", &[("X", "$")]),
            // Later keys are not visible yet; a later assignment wins.
            ("X=$Y\nY=1", &[("X", ""), ("Y", "1")]),
            (
                "A=1\nB=$A\nA=2\nC=$A",
                &[("A", "2"), ("B", "1"), ("C", "2")],
            ),
            // The dead `submatch[2] == "("` check: `$(NAME` expands.
            ("A=1\nB=$(A)", &[("A", "1"), ("B", "1)")]),
        ];
        for &(input, want) in cases {
            assert_eq!(parsed(input), map(want), "{input:?}");
        }
        // The process environment is never consulted: PATH is set in every
        // test process but expands to empty here.
        assert!(std::env::var_os("PATH").is_some());
        assert_eq!(parsed("X=$PATH")["X"], "");
    }

    #[test]
    fn dotenv_duplicate_keys_last_wins() {
        assert_eq!(parsed("A=1\nA=2\n"), map(&[("A", "2")]));
        assert_eq!(parsed("A=1\nexport A='3'\n"), map(&[("A", "3")]));
    }

    // godotenv_test.go TestLinesToIgnore.
    #[test]
    fn dotenv_ignores_blank_and_comment_lines() {
        for input in ["\n", "\r\n", "\t\t ", "# Comment", "\t # comment"] {
            assert_eq!(statement_start(input), None, "{input:?}");
            assert!(parsed(input).is_empty(), "{input:?}");
        }
        assert_eq!(
            statement_start(r"export OPTION_B='\n'"),
            Some(r"export OPTION_B='\n'")
        );
    }

    #[test]
    fn dotenv_rejects_bad_files_whole() {
        for input in [
            "A=1\nB='unterminated\n",
            "A=1\nB=\"unterminated",
            "A=1\nnot a statement\n",
            "A=1\nexport ",
            "A=\"x\"junk\n",
        ] {
            assert!(parse_dotenv(input).is_err(), "{input:?}");
        }
        // A trailing key with no '=' parses under the empty key.
        assert_eq!(parsed("A=1\nFOO"), map(&[("A", "1"), ("", "FOO")]));
    }

    fn load(contents: Option<&str>, existing: &[(&str, &str)]) -> HashMap<String, String> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".env");
        if let Some(text) = contents {
            std::fs::write(&path, text).unwrap();
        }
        let before = map(existing);
        let mut env = before.clone();
        load_dotenv_with(
            &path,
            |k| before.contains_key(k),
            |k, v| {
                env.insert(k.to_string(), v.to_string());
            },
        );
        env
    }

    /// [`load`] under the given platform's name rules. On Windows, names
    /// compare case-insensitively, like the real environment there.
    fn load_on(
        windows: bool,
        contents: &str,
        existing: &[(&str, &str)],
    ) -> HashMap<String, String> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".env");
        std::fs::write(&path, contents).unwrap();
        let before = map(existing);
        let mut env = before.clone();
        load_dotenv_for(
            &path,
            windows,
            |k| {
                before.keys().any(|b| {
                    if windows {
                        b.eq_ignore_ascii_case(k)
                    } else {
                        b == k
                    }
                })
            },
            |k, v| {
                env.insert(k.to_string(), v.to_string());
            },
        );
        env
    }

    fn root_of(env: &HashMap<String, String>) -> PathBuf {
        let get = |k: &str| env.get(k).map(OsString::from);
        Paths::from_vars(get(STATE_ROOT_VAR), get("HOME")).root
    }

    #[test]
    fn dotenv_never_sets_state_root() {
        let file = "RIVAL_HOME=./.rival-dev\nA=1\nHOME=/repo-home\n";
        for windows in [false, true] {
            // Unset: the file's value is dropped; other keys, HOME included,
            // still load.
            let env = load_on(windows, file, &[]);
            assert_eq!(
                env,
                map(&[("A", "1"), ("HOME", "/repo-home")]),
                "windows={windows}"
            );
            assert_eq!(root_of(&env), PathBuf::from("/repo-home/.rival"));

            // Exported, even empty: the process value stays.
            let env = load_on(windows, file, &[("RIVAL_HOME", ""), ("HOME", "/h")]);
            assert_eq!(env["RIVAL_HOME"], "", "windows={windows}");
            assert_eq!(root_of(&env), PathBuf::from("/h/.rival"));
            let env = load_on(windows, file, &[("RIVAL_HOME", "/custom"), ("HOME", "/h")]);
            assert_eq!(env["RIVAL_HOME"], "/custom", "windows={windows}");
            assert_eq!(env["A"], "1", "windows={windows}");
            assert_eq!(root_of(&env), PathBuf::from("/custom"));
        }
    }

    #[test]
    fn dotenv_state_root_spelling_follows_platform() {
        let file =
            "rival_home=a\nRival_Home=b\nRIVAL_HOME=c\nOK=1\nRIVAL_HOM\u{f3}=d\nRIVAL_HOMEX=e\n";
        // Near names (another letter, a longer name) are ordinary keys.
        let near = [("OK", "1"), ("RIVAL_HOM\u{f3}", "d"), ("RIVAL_HOMEX", "e")];
        // Windows names are case-insensitive: no spelling gets through.
        assert_eq!(load_on(true, file, &[]), map(&near));
        // Unix names are exact: other spellings are ordinary variables.
        let mut unix = vec![("rival_home", "a"), ("Rival_Home", "b")];
        unix.extend(near);
        assert_eq!(load_on(false, file, &[]), map(&unix));
        // The host loader applies the host's rule.
        let env = load(Some(file), &[]);
        assert!(!env.contains_key(STATE_ROOT_VAR));
        assert_eq!(env.contains_key("rival_home"), !cfg!(windows));
    }

    /// godotenv widens each UTF-8 byte of a key to a rune. The second byte
    /// of U+0131 (dotless i) widens to `±`, which is no letter, so a file
    /// spelling `RIVAL_HOME` with it sets nothing on any platform.
    #[test]
    fn dotenv_rejects_unicode_state_root_spelling() {
        let file = "R\u{131}VAL_HOME=repo\nOK=1\n";
        assert!(parse_dotenv(file).is_err());
        for windows in [false, true] {
            assert!(load_on(windows, file, &[]).is_empty(), "windows={windows}");
        }
    }

    /// The Windows host loader, with the OS name comparison: no spelling of
    /// `RIVAL_HOME` loads, exported values (empty ones included) stay, and
    /// near names and ordinary keys load. The native child test in
    /// `subprocess::windows_tests` checks the rule against the OS lookup.
    #[cfg(windows)]
    #[test]
    fn dotenv_state_root_windows_rule_is_the_os_rule() {
        let file = "rival_home=a\nRival_Home=b\nRIVAL_HOM\u{f3}=d\nRIVAL_HOMEX=e\nOK=1\n";
        for existing in [
            &[][..],
            &[("RIVAL_HOME", "")][..],
            &[("RIVAL_HOME", "/custom")][..],
        ] {
            let mut want = map(existing);
            want.extend(map(&[
                ("RIVAL_HOM\u{f3}", "d"),
                ("RIVAL_HOMEX", "e"),
                ("OK", "1"),
            ]));
            assert_eq!(load(Some(file), existing), want, "existing {existing:?}");
        }
    }

    #[test]
    fn dotenv_missing_file_is_silent() {
        assert!(load(None, &[]).is_empty());
    }

    #[test]
    fn dotenv_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(read_dotenv(tmp.path()), Err(DotenvError::Io(_))));
    }

    #[test]
    fn dotenv_sets_new_vars_and_existing_win() {
        let env = load(
            Some("A=1\nB=\"two\"\n# c\nEXISTING=file\nEMPTY=file\nA=last\n"),
            &[("EXISTING", "process"), ("EMPTY", "")],
        );
        assert_eq!(
            env,
            map(&[
                ("A", "last"),
                ("B", "two"),
                ("EXISTING", "process"),
                ("EMPTY", ""),
            ])
        );
    }

    #[test]
    fn dotenv_expansion_ignores_process_env() {
        let env = load(Some("A=$EXISTING\nB=${A}x\n"), &[("EXISTING", "process")]);
        assert_eq!(env, map(&[("EXISTING", "process"), ("A", ""), ("B", "x")]));
    }

    #[test]
    fn dotenv_skips_entries_setenv_would_reject() {
        let env = load(Some("=v\nOK=1\n"), &[]);
        assert_eq!(env.get("OK").map(String::as_str), Some("1"));
        assert!(!env.contains_key(""));
        let env = load(Some("Z=\"a\0b\"\nY=2\n"), &[]);
        assert_eq!(env, map(&[("Y", "2")]));
    }

    #[test]
    fn dotenv_parse_error_sets_nothing() {
        let env = load(Some("A=1\nB='unterminated\n"), &[]);
        assert!(env.is_empty(), "{env:?}");
    }
}
