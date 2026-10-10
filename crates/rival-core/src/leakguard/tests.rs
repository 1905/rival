//! The scrub list is process-global, so each test uses its own fake
//! secret and never asserts that the list is empty.

use super::*;

#[test]
fn scrub_replaces_registered_secrets_only() {
    register("test-secret-scrub-0001");
    assert_eq!(
        scrub("key=test-secret-scrub-0001; again test-secret-scrub-0001"),
        "key=<redacted>; again <redacted>"
    );
    assert!(matches!(scrub("nothing here"), Cow::Borrowed(_)));
    assert!(active());
    assert!(max_len() >= "test-secret-scrub-0001".len());
}

#[test]
fn short_values_are_not_registered() {
    register("  short  ");
    assert_eq!(scrub("a short word"), "a short word");
}

#[test]
fn scrub_bytes_handles_non_utf8_text() {
    register("test-secret-scrub-0002");
    let mut data = b"\xff before test-secret-scrub-0002 after".to_vec();
    data.push(0xfe);
    let want = {
        let mut w = b"\xff before <redacted> after".to_vec();
        w.push(0xfe);
        w
    };
    assert_eq!(scrub_bytes(&data).as_ref(), want.as_slice());
    assert!(matches!(scrub_bytes(b"clean"), Cow::Borrowed(_)));
}

#[test]
fn a_longer_secret_goes_whole() {
    register("test-secret-scrub-0003");
    register("test-secret-scrub-0003-longer");
    assert_eq!(scrub("x test-secret-scrub-0003-longer y"), "x <redacted> y");
}

/// Session records and log lines are JSON: a secret with a quote, a
/// backslash or a control character appears there escaped, and the
/// escaped form is scrubbed too.
#[test]
fn json_escaped_secret_is_scrubbed() {
    let secret = "test-\"secret\\scrub\t0004";
    register(secret);
    let encoded = serde_json::to_string(&format!("key {secret} end")).unwrap();
    assert!(!encoded.contains(secret), "the test needs an escaped form");
    assert_eq!(scrub(&encoded), "\"key <redacted> end\"");
    assert_eq!(
        scrub_bytes(encoded.as_bytes()).as_ref(),
        b"\"key <redacted> end\"".as_slice()
    );
    assert_eq!(scrub(&format!("raw {secret}")), "raw <redacted>");
}
