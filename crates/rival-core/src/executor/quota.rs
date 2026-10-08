/// High-precision substrings that indicate a reviewer CLI hit a provider
/// quota/rate limit. Some providers report these failures only in captured
/// output, so matching is case-insensitive against the combined
/// stdout+stderr log.
///
/// These are deliberately specific to the provider error envelopes (not bare
/// tokens like "429" or "rate limit") so a reviewer legitimately *describing*
/// such a bug in its findings does not get misclassified as quota-exhausted.
const QUOTA_SIGNATURES: [&str; 8] = [
    "resource_exhausted (code 429)",
    "individual quota reached",
    "quota reached. contact your administrator",
    "enable overages",
    "insufficient_quota",
    "rate_limit_exceeded",
    "error 429 (too many requests)",
    "usage limit reached. upgrade",
];

/// Reports whether the captured CLI output indicates the provider rejected
/// the request due to a quota/rate limit. The match ignores case.
pub fn is_quota_exhausted(output: &str) -> bool {
    let lower = output.to_lowercase();
    QUOTA_SIGNATURES.iter().any(|sig| lower.contains(sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_quota_exhausted_cases() {
        let cases = [
            (
                "resource exhausted envelope",
                "E0601 23:12:44 log.go:398] agent executor error: RESOURCE_EXHAUSTED (code 429): Individual quota reached. Contact your administrator to enable overages. Resets in 154h30m51s.",
                true,
            ),
            (
                "individual quota reached",
                "Individual quota reached. Contact your administrator.",
                true,
            ),
            (
                "enable overages hint",
                "please enable overages to continue",
                true,
            ),
            (
                "openai insufficient_quota",
                r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#,
                true,
            ),
            (
                "rate_limit_exceeded",
                "error: rate_limit_exceeded, retry later",
                true,
            ),
            ("case insensitive", "resource_exhausted (CODE 429)", true),
            ("empty output is not quota", "", false),
            (
                "normal review output is not quota",
                "[HIGH] handler.go:42 returns 429 to the client when the rate limit is hit; consider backoff.",
                false,
            ),
            (
                "bare 429 in findings is not misclassified",
                "The endpoint should return HTTP 429 (Too Many Requests) on rate limit, but currently returns 500.",
                false,
            ),
        ];
        for (name, input, want) in cases {
            assert_eq!(is_quota_exhausted(input), want, "{name}: {input:?}");
        }
    }

    #[test]
    fn remaining_signatures_and_unicode_lowercasing() {
        for sig in QUOTA_SIGNATURES {
            assert!(is_quota_exhausted(&sig.to_uppercase()), "{sig}");
        }
        // The full lowercase mapping turns U+0130 into 'i' plus U+0307, so
        // the text does not match.
        assert!(!is_quota_exhausted("\u{130}NSUFFICIENT_QUOTA"));
    }
}
