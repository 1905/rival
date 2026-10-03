//! Go's Windows `path/filepath` rules, purely lexical.
//!
//! Go 1.25.14: `internal/filepathlite/path.go` and `path_windows.go`,
//! `path/filepath/path_windows.go`. The functions work on the encoded bytes
//! of a path (WTF-8 on Windows); every separator and volume marker they look
//! at is ASCII, so splitting there keeps the encoding valid.
//!
//! They are compiled on every platform so the rules are tested everywhere.
//! On Windows, [`crate::paths`] dispatches to them; Unix keeps Go's Unix rules,
//! where a backslash is an ordinary file name byte.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Go `filepath.Separator` on Windows.
pub const SEPARATOR: u8 = b'\\';
/// Go `filepath.ListSeparator` on Windows.
pub const LIST_SEPARATOR: u8 = b';';

/// Go `os.IsPathSeparator` on Windows.
pub fn is_sep(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    // SAFETY: every caller builds `bytes` from pieces of valid encoded paths,
    // split and joined only at ASCII bytes, plus ASCII bytes.
    PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(bytes) })
}

fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// `filepathlite.pathHasPrefixFold`: `s` starts with `prefix`, ignoring
/// ASCII case and treating both separators alike; a longer `s` must continue
/// with a separator.
fn has_prefix_fold(s: &[u8], prefix: &[u8]) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    for (i, &p) in prefix.iter().enumerate() {
        if is_sep(p) {
            if !is_sep(s[i]) {
                return false;
            }
        } else if !p.eq_ignore_ascii_case(&s[i]) {
            return false;
        }
    }
    !(s.len() > prefix.len() && !is_sep(s[prefix.len()]))
}

/// `filepathlite.uncLen`: the length of a UNC volume after `prefix_len`
/// leading bytes (host and share).
fn unc_len(path: &[u8], prefix_len: usize) -> usize {
    let mut count = 0;
    for (i, &c) in path.iter().enumerate().skip(prefix_len) {
        if is_sep(c) {
            count += 1;
            if count == 2 {
                return i;
            }
        }
    }
    path.len()
}

/// `filepathlite.cutPath`: splits at the first separator.
fn cut_path(path: &[u8]) -> (&[u8], &[u8], bool) {
    match path.iter().position(|&c| is_sep(c)) {
        Some(i) => (&path[..i], &path[i + 1..], true),
        None => (path, &[], false),
    }
}

/// Go `filepathlite.volumeNameLen` (Windows).
pub fn volume_name_len(path: &[u8]) -> usize {
    if path.len() >= 2 && path[1] == b':' {
        return 2;
    }
    if path.is_empty() || !is_sep(path[0]) {
        return 0;
    }
    if has_prefix_fold(path, br"\\.\UNC") {
        return unc_len(path, br"\\.\UNC\".len());
    }
    if has_prefix_fold(path, br"\\.")
        || has_prefix_fold(path, br"\\?")
        || has_prefix_fold(path, br"\??")
    {
        if path.len() == 3 {
            return 3;
        }
        let (_, rest, ok) = cut_path(&path[4..]);
        if !ok {
            return path.len();
        }
        return path.len() - rest.len() - 1;
    }
    if path.len() >= 2 && is_sep(path[1]) {
        return unc_len(path, 2);
    }
    0
}

/// Go `filepath.FromSlash` on Windows.
pub fn from_slash(path: &[u8]) -> Vec<u8> {
    path.iter()
        .map(|&c| if c == b'/' { SEPARATOR } else { c })
        .collect()
}

/// Go `filepath.VolumeName` on Windows.
pub fn volume_name(path: &[u8]) -> Vec<u8> {
    from_slash(&path[..volume_name_len(path)])
}

/// Go `filepath.IsAbs` on Windows.
pub fn is_abs(path: &[u8]) -> bool {
    let l = volume_name_len(path);
    if l == 0 {
        return false;
    }
    if is_sep(path[0]) && is_sep(path[1]) {
        return true;
    }
    let rest = &path[l..];
    !rest.is_empty() && is_sep(rest[0])
}

/// The output buffer of Go's `lazybuf`. Go only allocates once the output
/// stops being a prefix of the input; `postClean` runs only after that, so
/// the divergence is tracked here.
struct Out<'a> {
    path: &'a [u8],
    buf: Vec<u8>,
    diverged: bool,
}

impl Out<'_> {
    fn append(&mut self, c: u8) {
        let w = self.buf.len();
        if !self.diverged && !(w < self.path.len() && self.path[w] == c) {
            self.diverged = true;
        }
        self.buf.push(c);
    }
}

/// Go `filepath.Clean` on Windows.
pub fn clean(original: &[u8]) -> Vec<u8> {
    let vol_len = volume_name_len(original);
    let path = &original[vol_len..];
    if path.is_empty() {
        if vol_len > 1 && is_sep(original[0]) && is_sep(original[1]) {
            // Should be UNC.
            return from_slash(original);
        }
        let mut out = original.to_vec();
        out.push(b'.');
        return out;
    }
    let rooted = is_sep(path[0]);
    let n = path.len();
    let mut out = Out {
        path,
        buf: Vec::with_capacity(n),
        diverged: false,
    };
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.append(SEPARATOR);
        (r, dotdot) = (1, 1);
    }
    while r < n {
        if is_sep(path[r]) {
            // Empty path element.
            r += 1;
        } else if path[r] == b'.' && (r + 1 == n || is_sep(path[r + 1])) {
            // `.` element.
            r += 1;
        } else if path[r] == b'.'
            && path.get(r + 1) == Some(&b'.')
            && (r + 2 == n || is_sep(path[r + 2]))
        {
            // `..` element: remove to the last separator.
            r += 2;
            if out.buf.len() > dotdot {
                let mut w = out.buf.len() - 1;
                while w > dotdot && !is_sep(out.buf[w]) {
                    w -= 1;
                }
                out.buf.truncate(w);
            } else if !rooted {
                if !out.buf.is_empty() {
                    out.append(SEPARATOR);
                }
                out.append(b'.');
                out.append(b'.');
                dotdot = out.buf.len();
            }
        } else {
            // A real path element; add a separator if needed.
            if (rooted && out.buf.len() != 1) || (!rooted && !out.buf.is_empty()) {
                out.append(SEPARATOR);
            }
            while r < n && !is_sep(path[r]) {
                out.append(path[r]);
                r += 1;
            }
        }
    }
    if out.buf.is_empty() {
        out.append(b'.');
    }
    post_clean(&mut out, vol_len);
    let mut result = original[..vol_len].to_vec();
    result.extend_from_slice(&out.buf);
    from_slash(&result)
}

/// Go `filepathlite.postClean`: keeps a relative path relative.
fn post_clean(out: &mut Out<'_>, vol_len: usize) {
    if vol_len != 0 || !out.diverged {
        return;
    }
    // A ':' in the first element would turn `a/../c:` into the drive `c:`.
    for &c in &out.buf {
        if is_sep(c) {
            break;
        }
        if c == b':' {
            out.buf.splice(0..0, [b'.', SEPARATOR]);
            return;
        }
    }
    // `\a\..\??\c:\x` must not become the Root Local Device path `\??\c:\x`.
    if out.buf.len() >= 3 && is_sep(out.buf[0]) && out.buf[1] == b'?' && out.buf[2] == b'?' {
        out.buf.splice(0..0, [SEPARATOR, b'.']);
    }
}

/// Go `filepath.Join` on Windows.
pub fn join(elems: &[&[u8]]) -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    let mut last: u8 = 0;
    for &e in elems {
        let mut e = e;
        if b.is_empty() {
            // The first non-empty element is added unchanged.
        } else if is_sep(last) {
            // Strip leading separators so non-UNC elements never make `\\`.
            while let Some((&c, rest)) = e.split_first()
                && is_sep(c)
            {
                e = rest;
            }
            // `\` then `??` becomes `\.\??`, not a Root Local Device path.
            if b.len() == 1 && e.starts_with(b"??") && (e.len() == 2 || is_sep(e[2])) {
                b.extend_from_slice(br".\");
            }
        } else if last == b':' {
            // `C:` + `f` stays drive-relative: `C:f`.
        } else {
            b.push(SEPARATOR);
            last = SEPARATOR;
        }
        if let Some(&c) = e.last() {
            b.extend_from_slice(e);
            last = c;
        }
    }
    if b.is_empty() {
        return Vec::new();
    }
    clean(&b)
}

/// Go `filepath.Base` on Windows.
pub fn base(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let mut path = path;
    while let Some((&c, rest)) = path.split_last()
        && is_sep(c)
    {
        path = rest;
    }
    path = &path[volume_name_len(path)..];
    if let Some(i) = path.iter().rposition(|&c| is_sep(c)) {
        path = &path[i + 1..];
    }
    if path.is_empty() {
        return vec![SEPARATOR];
    }
    path.to_vec()
}

/// Go `filepath.Dir` on Windows.
pub fn dir(path: &[u8]) -> Vec<u8> {
    let vol_len = volume_name_len(path);
    let mut i = path.len();
    while i > vol_len && !is_sep(path[i - 1]) {
        i -= 1;
    }
    let d = clean(&path[vol_len..i]);
    let vol = volume_name(path);
    if d == b"." && vol.len() > 2 {
        // Must be UNC.
        return vol;
    }
    let mut out = vol;
    out.extend_from_slice(&d);
    out
}

/// Go `filepath.Ext` on Windows.
pub fn ext(path: &[u8]) -> &[u8] {
    for i in (0..path.len()).rev() {
        if is_sep(path[i]) {
            break;
        }
        if path[i] == b'.' {
            return &path[i..];
        }
    }
    &[]
}

/// Go `filepath.SplitList` on Windows: `;`-separated, where a quoted part
/// may contain `;`. Quotes are removed.
pub fn split_list(path: &[u8]) -> Vec<Vec<u8>> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut list = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, &c) in path.iter().enumerate() {
        if c == b'"' {
            quoted = !quoted;
        } else if c == LIST_SEPARATOR && !quoted {
            list.push(&path[start..i]);
            start = i + 1;
        }
    }
    list.push(&path[start..]);
    list.into_iter()
        .map(|s| s.iter().copied().filter(|&c| c != b'"').collect())
        .collect()
}

/// [`clean`] on a [`Path`].
pub fn clean_path(path: &Path) -> PathBuf {
    bytes_to_path(clean(path_bytes(path)))
}

/// [`join`] of two [`Path`]s.
pub fn join_paths(a: &Path, b: &Path) -> PathBuf {
    bytes_to_path(join(&[path_bytes(a), path_bytes(b)]))
}

/// [`base`] of a [`Path`].
pub fn base_path(path: &Path) -> PathBuf {
    bytes_to_path(base(path_bytes(path)))
}

/// [`is_abs`] of a [`Path`].
pub fn is_abs_path(path: &Path) -> bool {
    is_abs(path_bytes(path))
}

/// Go `filepath.Abs` on Windows with a snapshotted working directory.
///
/// Go calls `GetFullPathNameW`, which reads the process's current
/// directory, and for a path like `D:x` on another drive the per-drive
/// directory. Here `cwd` (Go's `os.Getwd()` snapshot, `None` for its error)
/// stands for the current directory: an absolute path is only cleaned, a
/// relative one is joined onto `cwd`, a root-relative `\x` takes the drive
/// of `cwd`, and a drive-relative `D:x` on `cwd`'s own drive joins `cwd`.
/// A drive-relative path on another drive needs that drive's own directory,
/// which only the OS knows: `full_path` (Go's `syscall.FullPath`) answers it.
pub fn abs_with(
    cwd: Option<&Path>,
    path: &Path,
    full_path: impl FnOnce(&Path) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let p = path_bytes(path);
    if is_abs(p) {
        return Some(clean_path(path));
    }
    let vol_len = volume_name_len(p);
    if vol_len == 0 && p.first().is_some_and(|&c| is_sep(c)) {
        // Rooted on the current drive.
        let cwd = cwd?;
        let mut out = volume_name(path_bytes(cwd));
        out.extend_from_slice(p);
        return Some(bytes_to_path(clean(&out)));
    }
    if vol_len == 2 {
        let cwd = cwd?;
        let c = path_bytes(cwd);
        if volume_name_len(c) == 2 && c[0].eq_ignore_ascii_case(&p[0]) {
            return Some(bytes_to_path(join(&[c, &p[2..]])));
        }
        return full_path(path).map(|full| clean_path(&full));
    }
    let cwd = cwd?;
    Some(bytes_to_path(join(&[path_bytes(cwd), p])))
}

/// Go `syscall.normalizeDir`: the full path of `dir`; a UNC result is
/// `EINVAL`.
fn normalize_dir(
    dir: &[u8],
    full_path: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    let ndir = full_path(dir)?;
    if ndir.len() > 2 && is_sep(ndir[0]) && is_sep(ndir[1]) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(ndir)
}

/// Go `syscall.joinExeDirAndFName` (Windows `StartProcess` with a `Dir`):
/// makes program `p` absolute against the child's directory `dir`, because
/// `CreateProcessW` would resolve it against this process's directory.
///
/// - UNC (`\\server\share\x`) and drive-rooted (`C:\x`) programs are kept.
/// - Drive-relative `C:x`: on `dir`'s drive it joins `dir`; on another
///   drive the OS resolves it from that drive's own directory.
/// - Root-relative `\x` takes `dir`'s drive.
/// - Anything else joins `dir`.
///
/// An empty `p`, a bare `C:` or a UNC `dir` is `EINVAL` (an `InvalidInput`
/// error, Go's `invalid argument`). `full_path` is Go's `syscall.FullPath`
/// (`GetFullPathNameW`); it is injected so the rules test on every platform.
pub fn join_exe_dir_and_fname(
    dir: &[u8],
    p: &[u8],
    full_path: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    let einval = || Err(std::io::ErrorKind::InvalidInput.into());
    if p.is_empty() {
        return einval();
    }
    if p.len() > 2 && is_sep(p[0]) && is_sep(p[1]) {
        return Ok(p.to_vec());
    }
    let concat = |a: &[u8], sep: bool, b: &[u8]| {
        let mut out = a.to_vec();
        if sep {
            out.push(SEPARATOR);
        }
        out.extend_from_slice(b);
        out
    };
    if p.len() > 1 && p[1] == b':' {
        if p.len() == 2 {
            return einval();
        }
        if is_sep(p[2]) {
            return Ok(p.to_vec());
        }
        let d = normalize_dir(dir, full_path)?;
        if d.first().is_some_and(|c| c.eq_ignore_ascii_case(&p[0])) {
            return full_path(&concat(&d, true, &p[2..]));
        }
        return full_path(p);
    }
    let d = normalize_dir(dir, full_path)?;
    if is_sep(p[0]) {
        // Go slices d[:2]; a full path is never shorter.
        return full_path(&concat(d.get(..2).unwrap_or(&d), false, p));
    }
    full_path(&concat(&d, true, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    /// Go `cleantests` (the portable ones), as Windows runs them: results
    /// through `FromSlash`.
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

    /// Go `wincleantests`.
    const CLEAN_WINDOWS: &[(&str, &str)] = &[
        (r"c:", r"c:."),
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
        (r"//host/share/foo/../baz", r"\\host\share\baz"),
        (r"\\host\share\foo\..\..\..\..\bar", r"\\host\share\bar"),
        (r"\\.\C:\a\..\..\..\..\bar", r"\\.\C:\bar"),
        (r"\\.\C:\\\\a", r"\\.\C:\a"),
        (r"\\a\b\..\c", r"\\a\b\c"),
        (r"\\a\b", r"\\a\b"),
        (r".\c:", r".\c:"),
        (r".\c:\foo", r".\c:\foo"),
        (r".\c:foo", r".\c:foo"),
        (r"//abc", r"\\abc"),
        (r"///abc", r"\\\abc"),
        (r"//abc//", r"\\abc\\"),
        (r"\\?\C:\", r"\\?\C:\"),
        (r"\\?\C:\a", r"\\?\C:\a"),
        (r"a/../c:", r".\c:"),
        (r"a\..\c:", r".\c:"),
        (r"a/../c:/a", r".\c:\a"),
        (r"a/../../c:", r"..\c:"),
        (r"foo:bar", r"foo:bar"),
        (r"/a/../??/a", r"\.\??\a"),
    ];

    #[test]
    fn clean_matches_go_windows_clean() {
        for &(input, want) in CLEAN_PORTABLE {
            let want = s(from_slash(want.as_bytes()));
            assert_eq!(s(clean(input.as_bytes())), want, "Clean({input:?})");
            assert_eq!(s(clean(want.as_bytes())), want, "Clean({want:?})");
        }
        for &(input, want) in CLEAN_WINDOWS {
            assert_eq!(s(clean(input.as_bytes())), want, "Clean({input:?})");
            assert_eq!(s(clean(want.as_bytes())), want, "Clean({want:?})");
        }
    }

    #[test]
    fn join_matches_go_windows_join() {
        // Go `jointests` (portable) and `winjointests`, results FromSlash'd.
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
            (&["C:", ""], "C:."),
            (&["C:", "", ""], "C:."),
            (&["C:", r"\a"], r"C:\a"),
            (&["C:", "", r"\a"], r"C:\a"),
            (&["C:.", "a"], "C:a"),
            (&["C:a", "b"], r"C:a\b"),
            (&["C:a", "b", "d"], r"C:a\b\d"),
            (&[r"\\host\share", "foo"], r"\\host\share\foo"),
            (&[r"\\host\share\foo"], r"\\host\share\foo"),
            (&["//host/share", "foo/bar"], r"\\host\share\foo\bar"),
            (&[r"\"], r"\"),
            (&[r"\", ""], r"\"),
            (&[r"\", "a"], r"\a"),
            (&[r"\\", "a"], r"\\a"),
            (&[r"\", "a", "b"], r"\a\b"),
            (&[r"\\", "a", "b"], r"\\a\b"),
            (&[r"\", r"\\a\b", "c"], r"\a\b\c"),
            (&[r"\\a", "b", "c"], r"\\a\b\c"),
            (&[r"\\a\", "b", "c"], r"\\a\b\c"),
            (&["//", "a"], r"\\a"),
            (&[r"a:\b\c", r"x\..\y:\..\..\z"], r"a:\b\z"),
            (&[r"\", r"??\a"], r"\.\??\a"),
        ];
        for &(elems, want) in cases {
            let parts: Vec<&[u8]> = elems.iter().map(|e| e.as_bytes()).collect();
            let want = s(from_slash(want.as_bytes()));
            assert_eq!(s(join(&parts)), want, "Join({elems:?})");
        }
    }

    #[test]
    fn is_abs_matches_go_windows() {
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
            (r"\??\a\b\c", true),
        ];
        for (path, want) in cases {
            assert_eq!(is_abs(path.as_bytes()), want, "IsAbs({path:?})");
        }
        // Go: every Unix case is false without a volume, and keeps its
        // answer with a `c:` prefix.
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
            assert!(!is_abs(path.as_bytes()), "IsAbs({path:?})");
            let prefixed = format!("c:{path}");
            assert_eq!(is_abs(prefixed.as_bytes()), want, "IsAbs({prefixed:?})");
        }
    }

    #[test]
    fn volume_name_matches_go_windows() {
        let cases = [
            (r"c:/foo/bar", r"c:"),
            (r"c:", r"c:"),
            (r"c:\", r"c:"),
            (r"2:", r"2:"),
            (r"", r""),
            (r"\\\host", r"\\\host"),
            (r"\\\host\", r"\\\host"),
            (r"\\\host\share", r"\\\host"),
            (r"\\\host\\share", r"\\\host"),
            (r"\\host", r"\\host"),
            (r"//host", r"\\host"),
            (r"\\host\", r"\\host\"),
            (r"//host/", r"\\host\"),
            (r"\\host\share", r"\\host\share"),
            (r"//host/share", r"\\host\share"),
            (r"\\host\share\", r"\\host\share"),
            (r"//host/share/", r"\\host\share"),
            (r"\\host\share\foo", r"\\host\share"),
            (r"//host/share/foo", r"\\host\share"),
            (r"\\host\share\\foo\\\bar\\\\baz", r"\\host\share"),
            (r"//host/share//foo///bar////baz", r"\\host\share"),
            (r"\\host\share\foo\..\bar", r"\\host\share"),
            (r"//host/share/foo/../bar", r"\\host\share"),
            (r"//.", r"\\."),
            (r"//./", r"\\.\"),
            (r"//./NUL", r"\\.\NUL"),
            (r"//?", r"\\?"),
            (r"//?/", r"\\?\"),
            (r"//?/NUL", r"\\?\NUL"),
            (r"/??", r"\??"),
            (r"/??/", r"\??\"),
            (r"/??/NUL", r"\??\NUL"),
            (r"//./a/b", r"\\.\a"),
            (r"//./C:", r"\\.\C:"),
            (r"//./C:/", r"\\.\C:"),
            (r"//./C:/a/b/c", r"\\.\C:"),
            (r"//./UNC/host/share/a/b/c", r"\\.\UNC\host\share"),
            (r"//./UNC/host", r"\\.\UNC\host"),
            (r"//./UNC/host\", r"\\.\UNC\host\"),
            (r"//./UNC", r"\\.\UNC"),
            (r"//./UNC/", r"\\.\UNC\"),
            (r"\\?\x", r"\\?\x"),
            (r"\??\x", r"\??\x"),
        ];
        for (path, want) in cases {
            assert_eq!(
                s(volume_name(path.as_bytes())),
                want,
                "VolumeName({path:?})"
            );
        }
    }

    #[test]
    fn base_and_dir_match_go_windows() {
        // Go `basetests` with results cleaned, then `winbasetests`.
        let bases = [
            ("", "."),
            (".", "."),
            ("/.", "."),
            ("/", r"\"),
            ("////", r"\"),
            ("x/", "x"),
            ("abc", "abc"),
            ("abc/def", "def"),
            ("a/b/.x", ".x"),
            ("a/b/c.", "c."),
            ("a/b/c.x", "c.x"),
            (r"c:\", r"\"),
            (r"c:.", "."),
            (r"c:\a\b", "b"),
            (r"c:a\b", "b"),
            (r"c:a\b\c", "c"),
            (r"\\host\share\", r"\"),
            (r"\\host\share\a", "a"),
            (r"\\host\share\a\b", "b"),
        ];
        for (path, want) in bases {
            assert_eq!(s(base(path.as_bytes())), want, "Base({path:?})");
        }
        let dirs = [
            ("", "."),
            (".", "."),
            ("/.", r"\"),
            ("/", r"\"),
            ("/foo", r"\"),
            ("x/", "x"),
            ("abc", "."),
            ("abc/def", "abc"),
            ("a/b/.x", r"a\b"),
            ("a/b/c.", r"a\b"),
            ("a/b/c.x", r"a\b"),
            (r"c:\", r"c:\"),
            (r"c:.", r"c:."),
            (r"c:\a\b", r"c:\a"),
            (r"c:a\b", r"c:a"),
            (r"c:a\b\c", r"c:a\b"),
            (r"\\host\share", r"\\host\share"),
            (r"\\host\share\", r"\\host\share\"),
            (r"\\host\share\a", r"\\host\share\"),
            (r"\\host\share\a\b", r"\\host\share\a"),
            (r"\\\\", r"\\\\"),
        ];
        for (path, want) in dirs {
            assert_eq!(s(dir(path.as_bytes())), want, "Dir({path:?})");
        }
    }

    #[test]
    fn ext_and_split_list_match_go_windows() {
        for (path, want) in [
            ("path.go", ".go"),
            ("path.pb.go", ".go"),
            ("a.dir/b", ""),
            ("a.dir/b.go", ".go"),
            ("a.dir/", ""),
            (r"a.dir\b", ""),
            (r"C:\x\codex.CMD", ".CMD"),
        ] {
            assert_eq!(ext(path.as_bytes()), want.as_bytes(), "Ext({path:?})");
        }
        // Go `splitlisttests` (with `;`) and `winsplitlisttests`.
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("a;b", &["a", "b"]),
            (";a;b", &["", "a", "b"]),
            ("\"a\"", &["a"]),
            ("\";\"", &[";"]),
            ("\"a;b\"", &["a;b"]),
            ("\";\";", &[";", ""]),
            (";\";\"", &["", ";"]),
            ("a\";\"b", &["a;b"]),
            ("a; \"\"b", &["a", " b"]),
            ("\"a;b", &["a;b"]),
            ("\"\"a;b", &["a", "b"]),
            ("\"\"\"a;b", &["a;b"]),
            ("\"\"\"\"a;b", &["a", "b"]),
            ("a\";b", &["a;b"]),
            ("a;b\";c", &["a", "b;c"]),
            ("\"a\";b\";c", &["a", "b;c"]),
        ];
        for &(list, want) in cases {
            let got: Vec<String> = split_list(list.as_bytes()).into_iter().map(s).collect();
            assert_eq!(got, want, "SplitList({list:?})");
        }
    }

    #[test]
    fn abs_covers_drive_root_unc_and_relative_paths() {
        let cwd = Path::new(r"C:\work\dir");
        let no_os = |_: &Path| -> Option<PathBuf> { panic!("not an other-drive path") };
        let abs = |p: &str| abs_with(Some(cwd), Path::new(p), no_os).map(|p| p.into_os_string());
        assert_eq!(abs(r"D:\a\.\b\"), Some(r"D:\a\b".into()), "drive-rooted");
        assert_eq!(abs(r"sub\..\x"), Some(r"C:\work\dir\x".into()), "relative");
        assert_eq!(abs(""), Some(r"C:\work\dir".into()), "empty is cwd");
        assert_eq!(abs(r"\top\x"), Some(r"C:\top\x".into()), "root-relative");
        assert_eq!(
            abs("/top/x"),
            Some(r"C:\top\x".into()),
            "slash root-relative"
        );
        assert_eq!(abs(r"c:rel"), Some(r"C:\work\dir\rel".into()), "same drive");
        assert_eq!(
            abs(r"\\host\share\a\..\b"),
            Some(r"\\host\share\b".into()),
            "UNC"
        );
        // Another drive's own directory comes from the OS.
        let got = abs_with(Some(cwd), Path::new(r"D:rel"), |p| {
            assert_eq!(p, Path::new(r"D:rel"));
            Some(PathBuf::from(r"D:\other\.\rel"))
        });
        assert_eq!(
            got.map(PathBuf::into_os_string),
            Some(r"D:\other\rel".into())
        );
        // No working directory: only absolute paths resolve.
        assert_eq!(abs_with(None, Path::new("rel"), no_os), None);
        assert_eq!(abs_with(None, Path::new(r"\x"), no_os), None);
        assert_eq!(
            abs_with(None, Path::new(r"C:\x"), no_os).map(PathBuf::into_os_string),
            Some(r"C:\x".into())
        );
    }

    /// A stand-in for `GetFullPathNameW` with the current directory
    /// `C:\cur` and `D:`'s own directory `D:\dcur`. It records every call.
    fn fake_full_path(
        calls: &std::cell::RefCell<Vec<String>>,
    ) -> impl Fn(&[u8]) -> std::io::Result<Vec<u8>> + '_ {
        move |p: &[u8]| {
            calls.borrow_mut().push(s(p.to_vec()));
            let cur = Path::new(r"C:\cur");
            let other = |q: &Path| {
                let b = path_bytes(q);
                Some(bytes_to_path(join(&[br"D:\dcur", &b[2..]])))
            };
            let full = abs_with(Some(cur), Path::new(std::str::from_utf8(p).unwrap()), other)
                .expect("absolute");
            Ok(path_bytes(&full).to_vec())
        }
    }

    #[test]
    fn join_exe_dir_and_fname_matches_go_start_process() {
        let calls = std::cell::RefCell::new(Vec::new());
        let full = fake_full_path(&calls);
        let join =
            |dir: &str, p: &str| join_exe_dir_and_fname(dir.as_bytes(), p.as_bytes(), &full).map(s);
        // Kept as-is, the directory is never read.
        assert_eq!(
            join(r"C:\work", r"\\srv\share\t.exe").unwrap(),
            r"\\srv\share\t.exe"
        );
        assert_eq!(join(r"C:\work", r"D:\bin\t.exe").unwrap(), r"D:\bin\t.exe");
        assert_eq!(join(r"C:\work", "D:/bin/t.exe").unwrap(), "D:/bin/t.exe");
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
        // Relative: joins the directory.
        assert_eq!(
            join(r"C:\work", r"bin\t.exe").unwrap(),
            r"C:\work\bin\t.exe"
        );
        assert_eq!(join(r"C:\work", r"..\t.exe").unwrap(), r"C:\t.exe");
        // A relative directory is first made full against this process.
        assert_eq!(join("rel", "t.exe").unwrap(), r"C:\cur\rel\t.exe");
        // Root-relative: the directory's drive, not its path.
        assert_eq!(join(r"C:\work", r"\tool.exe").unwrap(), r"C:\tool.exe");
        assert_eq!(join(r"E:\work", "/tool.exe").unwrap(), r"E:\tool.exe");
        // Drive-relative on the directory's drive (any case): joins it.
        assert_eq!(join(r"C:\work", "C:tool.exe").unwrap(), r"C:\work\tool.exe");
        assert_eq!(join(r"c:\work", "C:tool.exe").unwrap(), r"c:\work\tool.exe");
        // Drive-relative on another drive: that drive's own directory.
        calls.borrow_mut().clear();
        assert_eq!(join(r"C:\work", "D:tool.exe").unwrap(), r"D:\dcur\tool.exe");
        assert_eq!(*calls.borrow(), [r"C:\work", "D:tool.exe"]);
        // EINVAL: empty program, a bare drive, a UNC directory.
        for (dir, p) in [
            (r"C:\work", ""),
            (r"C:\work", "C:"),
            (r"\\srv\share\w", "t.exe"),
        ] {
            let err = join(dir, p).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{dir:?} {p:?}"
            );
            assert_eq!(err.raw_os_error(), None);
        }
        // A UNC directory still allows a drive-rooted or UNC program.
        assert_eq!(join(r"\\srv\share\w", r"C:\t.exe").unwrap(), r"C:\t.exe");
        // A FullPath error is returned as-is.
        let failing =
            |_: &[u8]| -> std::io::Result<Vec<u8>> { Err(std::io::Error::from_raw_os_error(123)) };
        let err = join_exe_dir_and_fname(br"C:\w", b"t.exe", &failing).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(123));
    }
}
