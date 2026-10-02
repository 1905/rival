//! Acceptance and fields derived from Go 1.25.14 `net/url/url.go` and
//! `net/netip/netip.go` (read, not run).

use super::*;

fn ok(raw: &[u8]) -> Url {
    parse(raw).unwrap_or_else(|| panic!("{} did not parse", String::from_utf8_lossy(raw)))
}

#[test]
fn splits_scheme_host_and_decoded_path() {
    let u = ok(b"HTTPS://GitLab.Example.COM:8443/g/%2E%2E/a%2Fb?x=1#note_1");
    assert_eq!(u.scheme, "https");
    assert_eq!(u.host, b"GitLab.Example.COM:8443");
    assert_eq!(u.hostname(), b"GitLab.Example.COM");
    assert_eq!(u.path, b"/g/../a/b");
    assert!(!u.has_user && !u.force_query);

    assert!(ok(b"https://h/a?").force_query);
    assert!(!ok(b"https://h/a??").force_query);
    assert!(ok(b"https://h/a?#x?").force_query);
    assert_eq!(ok(b"https://h/a+b%20c").path, b"/a+b c");
    // The fragment is cut before the CTL check and only its escapes matter.
    assert_eq!(ok(b"https://h/a#\n").path, b"/a");
}

#[test]
fn users_hosts_and_ip_literals() {
    let u = ok(b"ssh://a@b@h:22/x");
    assert!(u.has_user);
    assert_eq!(u.hostname(), b"h");
    assert!(ok(b"https://@h/").has_user);
    assert!(ok(b"https://u:p%40@h/").has_user);

    let u = ok(b"ssh://[::1]:2222/x");
    assert_eq!(u.host, b"[::1]:2222");
    assert_eq!(u.hostname(), b"::1");
    assert_eq!(ok(b"https://[::1]/").hostname(), b"::1");
    assert_eq!(ok(b"https://[fe80::1%25en0]/").host, b"[fe80::1%en0]");
    assert_eq!(ok(b"https://[::ffff:1.2.3.4]/").host, b"[::ffff:1.2.3.4]");
    assert_eq!(ok(b"https://%C3%A4.example/").host, "ä.example".as_bytes());
    assert_eq!(ok(b"https://%FF.example/").host, b"\xff.example");
    assert_eq!(ok(b"https://h:/x").host, b"h:");
}

#[test]
fn opaque_and_relative_forms_have_no_host() {
    let u = ok(b"https:h/x");
    assert!(u.host.is_empty() && u.path.is_empty());
    let u = ok(b"https:///a");
    assert!(u.host.is_empty());
    assert_eq!(u.path, b"/a");
    let u = ok(b"/tmp/repo");
    assert!(u.scheme.is_empty() && u.host.is_empty());
    assert_eq!(u.path, b"/tmp/repo");
}

#[test]
fn go_parse_errors_reject() {
    for raw in [
        &b"://x"[..],     // missing protocol scheme
        b"https://h/\n",  // control character
        b"1a:b",          // colon in the first relative segment
        b"https://h/%zz", // invalid escapes
        b"https://h/a%2",
        b"https://h/%",
        b"https://h/%\xffz",
        b"https://h/#%zz",     // fragment escapes count
        b"https://h:x/",       // non-numeric port
        b"https://a[b]/",      // '[' not at the start
        b"https://[::1/",      // missing ']'
        b"https://[::1]x/",    // junk after ']'
        b"https://[1.2.3.4]/", // IPv4 in brackets
        b"https://[zz]/",
        b"https://[::1%25]/",           // empty zone
        b"https://[fe80::1%25%0A]/",    // zone escape of a control byte
        b"https://a b/",                // invalid host character
        b"https://%41/",                // ASCII escaped in a host
        b"https://u^@h/",               // invalid userinfo
        b"https://u%zz@h/",             // invalid user escape
        b"ssh://u:p%z@h/",              // invalid password escape
        "https://\u{e4}@h/".as_bytes(), // non-ASCII userinfo
    ] {
        assert_eq!(parse(raw), None, "{}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn https_string_escapes_host_and_path_and_keeps_force_query() {
    assert_eq!(
        https_string(
            b"GitLab.Example.com:8443",
            b"/g/app/-/merge_requests/7",
            false
        ),
        "https://GitLab.Example.com:8443/g/app/-/merge_requests/7"
    );
    assert_eq!(https_string(b"h", b"/a", true), "https://h/a?");
    assert_eq!(
        https_string("ä".as_bytes(), "/grün/a b?#%".as_bytes(), false),
        "https://%C3%A4/gr%C3%BCn/a%20b%3F%23%25"
    );
    assert_eq!(
        https_string(b"[fe80::1%en0]", b"/", false),
        "https://[fe80::1%25en0]/"
    );
    // Path mode keeps the sub-delims, ':' and '@'.
    assert_eq!(
        https_string(b"h", b"/a:b@c;d,e=f+g$h&i", false),
        "https://h/a:b@c;d,e=f+g$h&i"
    );
}

#[test]
fn path_escape_is_segment_escaping() {
    assert_eq!(path_escape(b"group/sub/app"), "group%2Fsub%2Fapp");
    assert_eq!(
        path_escape(b"a b;c,d?e:f@g&h=i+j$k"),
        "a%20b%3Bc%2Cd%3Fe:f@g&h=i+j$k"
    );
    assert_eq!(path_escape("ä~._-".as_bytes()), "%C3%A4~._-");
    assert_eq!(path_escape(b"%"), "%25");
}

#[test]
fn ip_literals_follow_netip() {
    for good in [
        "::",
        "::1",
        "1:2:3:4:5:6:7:8",
        "1:2:3:4:5:6:7::",
        "::ffff:1.2.3.4",
        "1:2:3:4:5:6:1.2.3.4",
        "fe80::1%en0",
        "ABCD:ef01::",
    ] {
        assert_eq!(parse_addr(good.as_bytes()), Some(Addr::V6), "{good}");
    }
    assert_eq!(parse_addr(b"1.2.3.4"), Some(Addr::V4));
    for bad in [
        "1:2:3:4:5:6:7:8:9",
        "1::2::3",
        "12345::",
        "1:2",
        "1:2:3:4:5:6:7:8::",
        "1:2:3:4::5:6:7:8",
        "1:1.2.3.4",
        "1:2:3:4:5:6:7:1.2.3.4",
        "::1.2.3.04",
        "::1.2.3.256",
        "::1.2.3",
        "1:",
        ":1",
        "::1%",
        "%x",
        "1.2.3",
        "1..2.3",
        "1.2.3.4.5",
        "zz",
        "",
    ] {
        assert_eq!(parse_addr(bad.as_bytes()), None, "{bad}");
    }
}
