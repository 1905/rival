use super::*;

const S: i64 = SECOND as i64;
const M: i64 = MINUTE as i64;
const H: i64 = HOUR as i64;

#[test]
fn parse_accepts_the_duration_grammar() {
    let ok: &[(&str, i64)] = &[
        ("0", 0),
        ("-0", 0),
        ("+0", 0),
        ("0s", 0),
        ("10m", 10 * M),
        ("30m", 30 * M),
        ("-5m", -5 * M),
        ("+5s", 5 * S),
        ("1.5h", 90 * M),
        ("1h30m", 90 * M),
        ("2h45m", 2 * H + 45 * M),
        ("300ms", 300 * MILLISECOND as i64),
        ("1us", 1_000),
        ("1\u{b5}s", 1_000),
        ("1\u{3bc}s", 1_000),
        ("1ns", 1),
        (".5s", S / 2),
        ("1.s", S),
        ("1.0000000001s", S),
        ("9223372036854775807ns", i64::MAX),
        ("-9223372036854775808ns", i64::MIN),
    ];
    for &(input, want) in ok {
        assert_eq!(parse(input), Ok(want), "{input}");
    }
    let bad: &[(&str, &str)] = &[
        ("", r#"time: invalid duration """#),
        ("-", r#"time: invalid duration "-""#),
        ("banana", r#"time: invalid duration "banana""#),
        (".", r#"time: invalid duration ".""#),
        (".s", r#"time: invalid duration ".s""#),
        ("1", r#"time: missing unit in duration "1""#),
        ("1x", r#"time: unknown unit "x" in duration "1x""#),
        ("1d", r#"time: unknown unit "d" in duration "1d""#),
        ("10 m", r#"time: unknown unit " m" in duration "10 m""#),
        (
            "9223372036854775808ns",
            r#"time: invalid duration "9223372036854775808ns""#,
        ),
        ("3000000h", r#"time: invalid duration "3000000h""#),
        (
            "1é",
            r#"time: unknown unit "\xc3\xa9" in duration "1\xc3\xa9""#,
        ),
    ];
    for &(input, want) in bad {
        assert_eq!(parse(input), Err(want.to_string()), "{input:?}");
    }
}

/// Two maximum parts overflow `u64`. The sum must not wrap to 0, because
/// a 0 run timeout turns the timeout off.
#[test]
fn parse_rejects_an_overflow_of_two_maximum_parts() {
    for input in [
        "9223372036854775808ns9223372036854775808ns",
        "-9223372036854775808ns9223372036854775808ns",
    ] {
        assert_eq!(
            parse(input),
            Err(format!("time: invalid duration \"{input}\"")),
            "{input}"
        );
    }
}

#[test]
fn format_prints_the_shortest_unit_text() {
    let cases: &[(&str, i64)] = &[
        ("0s", 0),
        ("1ns", 1),
        ("1.1\u{b5}s", 1_100),
        ("2.2ms", 2_200_000),
        ("3.3s", 3_300_000_000),
        ("4m5s", 4 * M + 5 * S),
        ("4m5.001s", 4 * M + 5001 * MILLISECOND as i64),
        ("5h6m7.001s", 5 * H + 6 * M + 7001 * MILLISECOND as i64),
        ("8m0.000000001s", 8 * M + 1),
        ("2562047h47m16.854775807s", i64::MAX),
        ("-2562047h47m16.854775808s", i64::MIN),
        ("30m0s", 30 * M),
        ("1h35m0s", 95 * M),
        ("-1.5s", -3 * S / 2),
    ];
    for &(want, d) in cases {
        assert_eq!(format(d), want, "{d}");
    }
}
