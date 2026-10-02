//! Go: `internal/gitscope/env.go`.

use std::ffi::OsString;

/// Variables that point Git at a specific repository instead of the one in
/// the working directory (`git rev-parse --local-env-vars`).
const REPOSITORY_VARS: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Clears Git's repository-local overrides before changing workdir.
/// See <https://git-scm.com/docs/githooks> and `git rev-parse --local-env-vars`.
/// Keeps host configuration and SSH authentication available for fetch.
///
/// Entries are Go `os.Environ()` items (`KEY=VALUE`). The key is everything
/// before the first `=` (Go `strings.Cut`); an entry without `=` is all key.
pub fn repository_env(env: &[OsString]) -> Vec<OsString> {
    env.iter()
        .filter(|item| {
            let bytes = item.as_encoded_bytes();
            let key = bytes
                .iter()
                .position(|&b| b == b'=')
                .map_or(bytes, |i| &bytes[..i]);
            !REPOSITORY_VARS.iter().any(|v| v.as_bytes() == key)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn clears_every_repository_override_and_keeps_the_rest_in_order() {
        let mut input: Vec<String> = REPOSITORY_VARS
            .iter()
            .map(|k| format!("{k}=caller-repository"))
            .collect();
        input.insert(0, "PATH=/usr/bin".into());
        input.insert(3, "GIT_SSH_COMMAND=ssh -p 2222".into());
        input.push("GIT_CONFIG_GLOBAL=/home/me/.gitconfig".into());
        input.push("GIT_DIRX=kept".into());
        input.push("GIT_DIR".into()); // no "=": the whole entry is the key
        input.push("HOME=/home/me".into());
        let input: Vec<&str> = input.iter().map(String::as_str).collect();

        let got = repository_env(&entries(&input));
        assert_eq!(
            got,
            entries(&[
                "PATH=/usr/bin",
                "GIT_SSH_COMMAND=ssh -p 2222",
                "GIT_CONFIG_GLOBAL=/home/me/.gitconfig",
                "GIT_DIRX=kept",
                "HOME=/home/me",
            ])
        );
    }

    #[test]
    fn value_with_equals_and_empty_input() {
        assert_eq!(
            repository_env(&entries(&["A=b=c", "GIT_DIR==x", "=C:=C:\\"])),
            entries(&["A=b=c", "=C:=C:\\"])
        );
        assert!(repository_env(&[]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_entries_are_kept_or_dropped_by_key() {
        use std::os::unix::ffi::OsStringExt;
        let keep = OsString::from_vec(b"X=\xff".to_vec());
        let drop = OsString::from_vec(b"GIT_DIR=\xff".to_vec());
        assert_eq!(repository_env(&[keep.clone(), drop]), vec![keep]);
    }
}
