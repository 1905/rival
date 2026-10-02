use super::*;
use chrono::TimeZone;

#[allow(clippy::too_many_arguments)]
fn at(
    offset_secs: i32,
    y: i32,
    mo: u32,
    d: u32,
    h: u32,
    mi: u32,
    s: u32,
    ns: u32,
) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(offset_secs)
        .unwrap()
        .with_ymd_and_hms(y, mo, d, h, mi, s)
        .unwrap()
        .with_nanosecond(ns)
        .unwrap()
}

fn raw(text: &str) -> Box<RawValue> {
    RawValue::from_string(text.to_string()).unwrap()
}

// ---- encoding ----

#[test]
fn format_time_trims_every_trailing_fraction_zero() {
    let cases = [
        (
            at(0, 2026, 1, 2, 3, 4, 5, 123_400_000),
            "2026-01-02T03:04:05.1234Z",
        ),
        (
            at(0, 2026, 1, 2, 3, 4, 5, 1),
            "2026-01-02T03:04:05.000000001Z",
        ),
        (at(0, 2026, 1, 2, 3, 4, 5, 0), "2026-01-02T03:04:05Z"),
        (
            at(0, 2026, 1, 2, 3, 4, 5, 120_000_000),
            "2026-01-02T03:04:05.12Z",
        ),
        (
            at(0, 2026, 1, 2, 3, 4, 5, 123_456_000),
            "2026-01-02T03:04:05.123456Z",
        ),
        (
            at(3 * 3600 + 1800, 2026, 1, 2, 3, 4, 5, 123_400_000),
            "2026-01-02T03:04:05.1234+03:30",
        ),
        (
            at(-5 * 3600, 2026, 1, 2, 3, 4, 5, 0),
            "2026-01-02T03:04:05-05:00",
        ),
        (zero_time(), "0001-01-01T00:00:00Z"),
    ];
    for (t, want) in cases {
        assert_eq!(format_time(&t).unwrap(), want);
    }
}

#[test]
fn format_time_rejects_years_go_rejects() {
    let t = at(0, 10000, 1, 1, 0, 0, 0, 0);
    assert_eq!(
        format_time(&t).unwrap_err(),
        "Time.MarshalJSON: year outside of range [0,9999]"
    );
}

#[test]
fn marshal_indent_escapes_like_go() {
    let s = "a<b>&\u{2028}\u{2029}\u{8}\u{c}\u{1}\u{7f}\"\\\n\r\t日\u{FFFD}";
    let got = String::from_utf8(marshal_indent(&s).unwrap()).unwrap();
    assert_eq!(
        got,
        "\"a\\u003cb\\u003e\\u0026\\u2028\\u2029\\b\\f\\u0001\u{7f}\\\"\\\\\\n\\r\\t日\u{FFFD}\""
    );
    // The byte quoter agrees with the serde path on valid UTF-8.
    assert_eq!(quote_bytes(s.as_bytes()), got);
}

#[test]
fn quote_bytes_escapes_each_invalid_byte_and_keeps_literal_replacement_chars() {
    let cases: [(&[u8], &str); 6] = [
        (b"a\xc3", r#""a\ufffd""#),
        (b"\xe6\x97", r#""\ufffd\ufffd""#),
        (b"\xf0\x9f\x98", r#""\ufffd\ufffd\ufffd""#),
        (b"\xff\xfe", r#""\ufffd\ufffd""#),
        // A UTF-8-encoded surrogate is three invalid bytes in Go.
        (b"\xed\xa0\x80", r#""\ufffd\ufffd\ufffd""#),
        ("\u{FFFD}\u{1F600}".as_bytes(), "\"\u{FFFD}\u{1F600}\""),
    ];
    for (input, want) in cases {
        assert_eq!(quote_bytes(input), want, "{input:?}");
    }
}

#[test]
fn lossy_utf8_replaces_each_invalid_byte() {
    let s = "日本語".as_bytes();
    assert_eq!(lossy_utf8(&s[..3]), "日");
    assert_eq!(lossy_utf8(&s[..4]), "日\u{FFFD}");
    assert_eq!(lossy_utf8(&s[..5]), "日\u{FFFD}\u{FFFD}");
    assert_eq!(lossy_utf8(b"a\xff\xfeb"), "a\u{FFFD}\u{FFFD}b");
    assert_eq!(lossy_utf8(b""), "");
}

#[test]
fn go_string_serializes_valid_text_normally_and_invalid_bytes_as_escapes() {
    #[derive(Serialize)]
    struct Wrap {
        a: GoString,
        b: GoString,
    }
    let w = Wrap {
        a: GoString::from("x<\u{FFFD}"),
        b: GoString::from_bytes(b"x<\xe6\x97".to_vec()),
    };
    let got = String::from_utf8(marshal_indent(&w).unwrap()).unwrap();
    assert_eq!(
        got,
        "{\n  \"a\": \"x\\u003c\u{FFFD}\",\n  \"b\": \"x\\u003c\\ufffd\\ufffd\"\n}"
    );
    assert_eq!(w.b.to_str_lossy(), "x<\u{FFFD}\u{FFFD}");
    assert_eq!(w.a, "x<\u{FFFD}");
    assert_ne!(w.b, GoString::from("x<\u{FFFD}\u{FFFD}"));
}

// ---- decoding ----

#[test]
fn decode_object_keeps_order_and_duplicates() {
    let got = decode_object(br#" {"a":1, "b":"x", "a":null} "#, "T").unwrap();
    let flat: Vec<_> = got.iter().map(|(k, v)| (k.as_str(), v.get())).collect();
    assert_eq!(flat, [("a", "1"), ("b", "\"x\""), ("a", "null")]);
}

#[test]
fn decode_object_top_level_null_and_mismatches() {
    assert!(decode_object(b"null", "pkg.T").unwrap().is_empty());
    for (doc, want) in [
        ("[]", "array"),
        ("\"s\"", "string"),
        ("12", "number"),
        ("true", "bool"),
    ] {
        assert_eq!(
            decode_object(doc.as_bytes(), "pkg.T").unwrap_err(),
            format!("json: cannot unmarshal {want} into Go value of type pkg.T")
        );
    }
    assert!(decode_object(b"{} x", "T").is_err(), "trailing data");
    assert!(decode_object(b"{", "T").is_err());
    assert!(decode_object(b"", "T").is_err());
}

#[test]
fn decode_object_replaces_invalid_utf8_and_lone_surrogates_like_go() {
    let doc = b"{\"a\":\"x\xffy\",\"b\":\"\xe6\x97\",\"c\":\"\\ud800x\",\"d\":\"\\udc00\",\
\"e\":\"\\ud83d\\ude00\",\"f\":\"\\ud800\\ud800\\udc00\",\"g\":\"\\ud800\\u0041\",\
\"h\":\"\xed\xa0\x80\",\"i\":\"\\\\ud800\"}";
    let got: Vec<(String, String)> = decode_object(doc, "T")
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k, decode_string(&v).unwrap().unwrap()))
        .collect();
    let want = [
        ("a", "x\u{FFFD}y"),
        ("b", "\u{FFFD}\u{FFFD}"),
        ("c", "\u{FFFD}x"),
        ("d", "\u{FFFD}"),
        ("e", "\u{1F600}"),
        ("f", "\u{FFFD}\u{10000}"),
        ("g", "\u{FFFD}A"),
        ("h", "\u{FFFD}\u{FFFD}\u{FFFD}"),
        // An escaped backslash is not the start of an escape.
        ("i", "\\ud800"),
    ];
    let got: Vec<(&str, &str)> = got.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(got, want);

    // Invalid bytes in a key are replaced too; outside strings they stay errors.
    let keys = decode_object(b"{\"k\xff\":1}", "T").unwrap();
    assert_eq!(keys[0].0, "k\u{FFFD}");
    assert!(decode_object(b"{\"a\":1\xff}", "T").is_err());
}

#[test]
fn decode_object_enforces_go_max_depth() {
    let nested = |depth: usize| {
        format!(
            "{{\"x\":{}{}}}",
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        )
    };
    assert!(decode_object(nested(10_000).as_bytes(), "T").is_ok());
    assert_eq!(
        decode_object(nested(10_001).as_bytes(), "T").unwrap_err(),
        "exceeded max depth"
    );
    // Brackets inside strings do not count.
    let in_string = format!("{{\"x\":\"{}\"}}", "[".repeat(20_000));
    assert!(decode_object(in_string.as_bytes(), "T").is_ok());
}

#[test]
fn match_field_prefers_exact_then_folds_like_go() {
    let names = ["id", "work_dir", "status", "error"];
    assert_eq!(match_field(&names, "id"), Some("id"));
    assert_eq!(match_field(&names, "ID"), Some("id"));
    assert_eq!(match_field(&names, "Work_Dir"), Some("work_dir"));
    // U+212A KELVIN SIGN folds to k; U+017F LONG S folds to s.
    assert_eq!(match_field(&names, "wor\u{212A}_dir"), Some("work_dir"));
    assert_eq!(match_field(&names, "\u{17F}tatu\u{17F}"), Some("status"));
    // Dotted and dotless I are not in i's fold orbit.
    assert_eq!(match_field(&names, "\u{130}d"), None);
    assert_eq!(match_field(&names, "\u{131}d"), None);
    assert_eq!(match_field(&names, "workdir"), None);
    assert_eq!(match_field(&names, "msg"), None);
}

#[test]
fn decode_string_and_int_follow_go_literal_rules() {
    assert_eq!(decode_string(&raw("null")), Ok(None));
    assert_eq!(decode_string(&raw(r#""a\u003cb""#)), Ok(Some("a<b".into())));
    for (text, want) in [
        ("5", "number"),
        ("true", "bool"),
        ("[1]", "array"),
        ("{}", "object"),
    ] {
        assert_eq!(decode_string(&raw(text)), Err(TypeMismatch(want.into())));
    }

    assert_eq!(decode_int(&raw("null")), Ok(None));
    assert_eq!(decode_int(&raw("-0")), Ok(Some(0)));
    assert_eq!(decode_int(&raw("-12")), Ok(Some(-12)));
    assert_eq!(decode_int(&raw("9223372036854775807")), Ok(Some(i64::MAX)));
    for text in ["1.5", "1e3", "1.0", "9223372036854775808"] {
        assert_eq!(
            decode_int(&raw(text)),
            Err(TypeMismatch(format!("number {text}")))
        );
    }
    assert_eq!(
        decode_int(&raw("\"1\"")),
        Err(TypeMismatch("string".into()))
    );
    assert_eq!(decode_int(&raw("false")), Err(TypeMismatch("bool".into())));
}

#[test]
fn decode_time_reads_raw_quoted_text() {
    assert_eq!(decode_time(&raw("null")), Ok(None));
    assert_eq!(
        decode_time(&raw(r#""2026-01-02T03:04:05Z""#)),
        Ok(Some(at(0, 2026, 1, 2, 3, 4, 5, 0)))
    );
    for text in ["5", "true", "{}", "[]"] {
        assert_eq!(
            decode_time(&raw(text)).unwrap_err(),
            "Time.UnmarshalJSON: input is not a JSON string"
        );
    }
    // Go does not unescape the string before parsing it.
    assert_eq!(
        decode_time(&raw(r#""\u0032026-01-02T03:04:05Z""#)).unwrap_err(),
        r#"parsing time "\\u0032026-01-02T03:04:05Z" as "2006-01-02T15:04:05Z07:00": cannot parse "\\u0032026-01-02T03:04:05Z" as "2006""#
    );
}

// ---- time parsing ----

#[test]
fn parse_time_round_trips_and_normalizes_zero_offset() {
    for text in [
        "2026-01-02T03:04:05.1234+03:30",
        "2026-01-02T03:04:05.000000001Z",
        "0001-01-01T00:00:00Z",
        "2026-01-02T03:04:05-05:00",
        "0000-02-29T00:00:00Z",
        "9999-12-31T23:59:59.999999999Z",
    ] {
        assert_eq!(
            format_time(&parse_time(text.as_bytes()).unwrap()).unwrap(),
            text
        );
    }
    // Go parses +00:00 and -00:00 into a zero offset and writes them back as Z.
    for text in ["2026-01-02T03:04:05+00:00", "2026-01-02T03:04:05-00:00"] {
        let t = parse_time(text.as_bytes()).unwrap();
        assert_eq!(format_time(&t).unwrap(), "2026-01-02T03:04:05Z");
    }
}

#[test]
fn parse_time_accepts_what_go_strict_rfc3339_lets_through() {
    // Go's fallback time.Parse(RFC3339) runs with its strict checks disabled.
    let cases = [
        ("2026-01-02T3:04:05Z", "2026-01-02T03:04:05Z"),
        ("2026-01-02T03:04:05,5Z", "2026-01-02T03:04:05.5Z"),
        (
            "2026-01-02T03:04:05.1234567891Z",
            "2026-01-02T03:04:05.123456789Z",
        ),
        ("2026-01-02T03:04:05+05:60", "2026-01-02T03:04:05+06:00"),
        ("2026-01-02T03:04:05-23:59", "2026-01-02T03:04:05-23:59"),
    ];
    for (text, want) in cases {
        let t = parse_time(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(format_time(&t).unwrap(), want, "{text}");
    }
}

#[test]
fn parse_time_rejects_like_go_with_go_messages() {
    let layout = r#""2006-01-02T15:04:05Z07:00""#;
    let cannot = |text: &str, elem: &str, as_: &str| {
        format!("parsing time \"{text}\" as {layout}: cannot parse \"{elem}\" as \"{as_}\"")
    };
    let range = |text: &str, what: &str| format!("parsing time \"{text}\": {what} out of range");
    let cases = [
        // chrono accepts these three; Go does not.
        (
            "2026-01-02t03:04:05Z",
            cannot("2026-01-02t03:04:05Z", "t03:04:05Z", "T"),
        ),
        (
            "2026-01-02 03:04:05Z",
            cannot("2026-01-02 03:04:05Z", " 03:04:05Z", "T"),
        ),
        (
            "2026-01-02T03:04:05z",
            cannot("2026-01-02T03:04:05z", "z", "Z07:00"),
        ),
        ("2026-01-02T03:04:60Z", range("2026-01-02T03:04:60Z", "second")),
        ("2026-01-02T24:00:00Z", range("2026-01-02T24:00:00Z", "hour")),
        ("2026-01-02T03:60:00Z", range("2026-01-02T03:60:00Z", "minute")),
        ("2026-13-02T03:04:05Z", range("2026-13-02T03:04:05Z", "month")),
        ("2026-02-29T03:04:05Z", range("2026-02-29T03:04:05Z", "day")),
        ("2026-01-00T03:04:05Z", range("2026-01-00T03:04:05Z", "day")),
        (
            "2026-01-02T03:04:05+25:00",
            range("2026-01-02T03:04:05+25:00", "time zone offset hour"),
        ),
        (
            "2026-01-02T03:04:05+05:61",
            range("2026-01-02T03:04:05+05:61", "time zone offset minute"),
        ),
        (
            "2026-01-02T03:04:05Zjunk",
            "parsing time \"2026-01-02T03:04:05Zjunk\": extra text: \"junk\"".to_string(),
        ),
        (
            "2026-01-02T03:04:05",
            cannot("2026-01-02T03:04:05", "", "Z07:00"),
        ),
        (
            "2026-01-02T03:04:05.Z",
            cannot("2026-01-02T03:04:05.Z", ".Z", "Z07:00"),
        ),
        (
            "2026-01-02T03:04:05*05:00",
            cannot("2026-01-02T03:04:05*05:00", "*05:00", "Z07:00"),
        ),
        ("26-01-02T03:04:05Z", cannot("26-01-02T03:04:05Z", "26-01-02T03:04:05Z", "2006")),
        ("2026-1-02T03:04:05Z", cannot("2026-1-02T03:04:05Z", "1-02T03:04:05Z", "01")),
        ("yesterday", cannot("yesterday", "yesterday", "2006")),
        ("", cannot("", "", "2006")),
        (
            "2026-01-02T03:04:05\u{FFFD}",
            "parsing time \"2026-01-02T03:04:05\\xef\\xbf\\xbd\" as \"2006-01-02T15:04:05Z07:00\": \
             cannot parse \"\\xef\\xbf\\xbd\" as \"Z07:00\""
                .to_string(),
        ),
    ];
    for (text, want) in cases {
        assert_eq!(parse_time(text.as_bytes()).unwrap_err(), want, "{text}");
    }
    let invalid = parse_time(b"2026-01-02T03:04:05\xff").unwrap_err();
    assert!(
        invalid.starts_with("parsing time \"2026-01-02T03:04:05\\xff\" as "),
        "{invalid}"
    );
}

#[test]
fn parse_time_rejects_offsets_fixed_offset_cannot_hold() {
    // Go accepts these (offset >= 24h) but then cannot marshal them again.
    for text in ["2026-01-02T03:04:05+24:00", "2026-01-02T03:04:05-23:60"] {
        assert_eq!(
            parse_time(text.as_bytes()).unwrap_err(),
            format!("parsing time \"{text}\": time zone offset not supported")
        );
    }
}
