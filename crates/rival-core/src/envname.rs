//! Environment-variable name comparisons for the security filters.
//!
//! `case_insensitive = false` is the Unix exact comparison on the
//! raw encoded bytes. `case_insensitive = true` is the host's
//! case-insensitive rule. On Windows it is the operating system's ordinal
//! ignore-case comparison (`CompareStringOrdinal`), which Microsoft names for
//! environment-variable names. Elsewhere it is ASCII folding: an injectable
//! model of the Windows rule for tests, not proof of the Windows behavior.

use std::ffi::OsStr;

/// Whether `name` is `want` under the rule.
pub(crate) fn eq(case_insensitive: bool, name: &OsStr, want: &str) -> bool {
    if case_insensitive {
        fold::eq(name, want)
    } else {
        name.as_encoded_bytes() == want.as_bytes()
    }
}

/// Whether `s` starts with `prefix` under the rule. A `prefix` ending in
/// `=` matches exactly one variable name.
pub(crate) fn has_prefix(case_insensitive: bool, s: &OsStr, prefix: &str) -> bool {
    if case_insensitive {
        fold::has_prefix(s, prefix)
    } else {
        s.as_encoded_bytes().starts_with(prefix.as_bytes())
    }
}

/// Windows: compares UTF-16 code units through the OS. Lengths are counted
/// in code units, so a name whose UTF-8 length differs still lines up.
/// Unpaired surrogates are compared as they are.
#[cfg(windows)]
mod fold {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    fn ordinal_eq(a: &[u16], b: &[u16]) -> bool {
        let (Ok(a_len), Ok(b_len)) = (i32::try_from(a.len()), i32::try_from(b.len())) else {
            return false;
        };
        // SAFETY: both pointers are valid for the explicit lengths given.
        unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) == CSTR_EQUAL }
    }

    pub(super) fn eq(name: &OsStr, want: &str) -> bool {
        let name: Vec<u16> = name.encode_wide().collect();
        let want: Vec<u16> = want.encode_utf16().collect();
        ordinal_eq(&name, &want)
    }

    pub(super) fn has_prefix(s: &OsStr, prefix: &str) -> bool {
        let prefix: Vec<u16> = prefix.encode_utf16().collect();
        let head: Vec<u16> = s.encode_wide().take(prefix.len()).collect();
        head.len() == prefix.len() && ordinal_eq(&head, &prefix)
    }
}

/// Other hosts: ASCII case folding on the encoded bytes. Non-ASCII bytes
/// and invalid encodings only match themselves.
#[cfg(not(windows))]
mod fold {
    use std::ffi::OsStr;

    pub(super) fn eq(name: &OsStr, want: &str) -> bool {
        name.as_encoded_bytes()
            .eq_ignore_ascii_case(want.as_bytes())
    }

    pub(super) fn has_prefix(s: &OsStr, prefix: &str) -> bool {
        s.as_encoded_bytes()
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    #[test]
    fn exact_rule_compares_raw_bytes() {
        assert!(eq(false, os("NODE_OPTIONS"), "NODE_OPTIONS"));
        assert!(!eq(false, os("Node_Options"), "NODE_OPTIONS"));
        assert!(!eq(false, os("NODE_OPTIONS_X"), "NODE_OPTIONS"));
        assert!(has_prefix(false, os("NODE_OPTIONS=x"), "NODE_OPTIONS"));
        assert!(has_prefix(false, os("AWS_REGION=x"), "AWS_"));
        assert!(!has_prefix(false, os("aws_region=x"), "AWS_"));
        assert!(has_prefix(false, os("KEY=v"), "KEY="));
        assert!(!has_prefix(false, os("KEY_2=v"), "KEY="));
        assert!(!has_prefix(false, os("KEY"), "KEY="));
        assert!(!has_prefix(false, os("\u{e9}cole=1"), "\u{c9}COLE="));
    }

    #[test]
    fn case_insensitive_rule_folds_ascii_everywhere() {
        assert!(eq(true, os("Node_Options"), "NODE_OPTIONS"));
        assert!(eq(true, os("rival_home"), "RIVAL_HOME"));
        assert!(!eq(true, os("RIVAL_HOMEX"), "RIVAL_HOME"));
        assert!(!eq(true, os("RIVAL_HOM"), "RIVAL_HOME"));
        assert!(has_prefix(true, os("aws_session_token=x"), "AWS_"));
        let key = "ANTHROPIC_API_KEY=";
        assert!(has_prefix(true, os("Anthropic_Api_Key=k"), key));
        assert!(!has_prefix(true, os("Anthropic_Api_Key_2=k"), key));
        assert!(!has_prefix(true, os("Anthropic_Api_Key"), key));
        assert!(!has_prefix(true, os("AWS"), "AWS_"));
        assert!(has_prefix(true, os("=C:=C:\\"), "="));
        // A letter never equals another letter: E with an accent is not E.
        assert!(!eq(true, os("\u{c9}COLE"), "ECOLE"));
        assert!(!has_prefix(true, os("\u{e9}cole_x=1"), "ECOLE_"));
        assert!(!eq(true, os("R\u{cd}VAL_HOME"), "RIVAL_HOME"));
    }

    /// The ASCII model: non-ASCII letters match only themselves, and the
    /// prefix length is in bytes.
    #[cfg(not(windows))]
    #[test]
    fn ascii_model_keeps_non_ascii_letters_distinct() {
        assert!(!eq(true, os("\u{e9}cole"), "\u{c9}COLE"));
        assert!(!has_prefix(true, os("\u{e9}cole_x=1"), "\u{c9}COLE_"));
        assert!(!eq(true, os("NODE_OPT\u{131}ONS"), "NODE_OPTIONS"));
        let long_s = os("NODE_OPTION\u{17f}=x");
        assert!(!has_prefix(true, long_s, "NODE_OPTIONS"));
    }

    /// The OS rule folds non-ASCII letters too, and counts the prefix in
    /// UTF-16 code units.
    #[cfg(windows)]
    #[test]
    fn windows_rule_uses_the_os_uppercase_table() {
        assert!(eq(true, os("\u{e9}cole"), "\u{c9}COLE"));
        assert!(has_prefix(true, os("\u{e9}cole_x=1"), "\u{c9}COLE_"));
        assert!(!has_prefix(true, os("\u{e9}cole"), "\u{c9}COLE_"));
        // Unpaired surrogates compare as raw code units.
        use std::os::windows::ffi::OsStringExt;
        let bad = std::ffi::OsString::from_wide(&[0x41, 0xD800, 0x3D, 0x78]);
        assert!(has_prefix(true, &bad, "a"));
        assert!(!has_prefix(true, &bad, "a="));
    }
}
