//! The leak guard: secrets this process holds, and the scrub that removes
//! them from text before it reaches a log, an error or a session file.
//!
//! A secret is registered when rival loads it (the proxy key, at
//! [`crate::config::Config::proxy_route`]). The provider log, the stdout
//! mirror, rival's own log lines, session records and printed errors all
//! pass through [`scrub`] or [`scrub_bytes`].

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::sync::RwLock;

/// What a scrubbed secret becomes.
pub const REDACTED: &str = "<redacted>";

/// Shorter values are not registered: they would match ordinary text.
pub const MIN_SECRET_LEN: usize = 8;

static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Adds `secret` to the scrub list. Values shorter than
/// [`MIN_SECRET_LEN`] (after trimming) and repeats are ignored.
pub fn register(secret: &str) {
    let secret = secret.trim();
    if secret.len() < MIN_SECRET_LEN {
        return;
    }
    let mut list = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    if !list.iter().any(|s| s == secret) {
        list.push(secret.to_string());
        // Longest first, so a secret that contains another goes whole.
        list.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }
}

/// Whether any secret is registered.
pub fn active() -> bool {
    !SECRETS.read().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// The length of the longest registered secret; 0 when none.
pub fn max_len() -> usize {
    SECRETS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .first()
        .map_or(0, String::len)
}

/// `text` with every registered secret replaced by [`REDACTED`].
pub fn scrub(text: &str) -> Cow<'_, str> {
    let list = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    if !list.iter().any(|s| text.contains(s.as_str())) {
        return Cow::Borrowed(text);
    }
    let mut out = text.to_string();
    for secret in list.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    Cow::Owned(out)
}

/// [`scrub`] over raw bytes (provider output need not be UTF-8).
pub fn scrub_bytes(data: &[u8]) -> Cow<'_, [u8]> {
    let list = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    if list.is_empty() || !list.iter().any(|s| find(data, s.as_bytes()).is_some()) {
        return Cow::Borrowed(data);
    }
    let mut out = data.to_vec();
    for secret in list.iter() {
        out = replace_bytes(&out, secret.as_bytes(), REDACTED.as_bytes());
    }
    Cow::Owned(out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn replace_bytes(data: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut rest = data;
    while let Some(i) = find(rest, needle) {
        out.extend_from_slice(&rest[..i]);
        out.extend_from_slice(with);
        rest = &rest[i + needle.len()..];
    }
    out.extend_from_slice(rest);
    out
}
