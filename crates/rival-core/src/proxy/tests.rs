//! The proxy module against an in-process HTTP fake on `127.0.0.1:0`. No
//! real proxy and no network.

use std::net::TcpListener;

use super::testserver::serve;
use super::*;

const KEY: &str = "test-proxy-key-0000";

/// The P0 list: Claude under two prefixes, Codex without one.
const P0_LIST: &str = r#"{"object":"list","data":[
  {"id":"emcd_/claude-opus-5-5","owned_by":"anthropic"},
  {"id":"emcd_/claude-fable-5-1","owned_by":"anthropic"},
  {"id":"emcd2_/claude-opus-5-5","owned_by":"anthropic"},
  {"id":"gpt-6-astra","owned_by":"openai"},
  {"id":"gpt-6.1-sol","owned_by":"openai"}
]}"#;

fn route(url: &str, prefix: &str) -> Route {
    Route::new(url, prefix, KEY)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn wire_id_and_prefix_parts() {
    assert_eq!(wire_id("emcd_", "claude-opus-5-5"), "emcd_/claude-opus-5-5");
    assert_eq!(wire_id("", "gpt-6-astra"), "gpt-6-astra");
    assert_eq!(id_prefix("emcd_/claude-opus-5-5"), "emcd_");
    assert_eq!(id_prefix("gpt-6-astra"), "");
    assert_eq!(id_model("a/b/c"), "b/c");
}

#[test]
fn models_sends_the_bearer_key_and_parses_the_list() {
    let (url, seen) = serve(200, P0_LIST);
    let got = models(&route(&url, "emcd_")).unwrap();
    assert_eq!(
        got,
        ids(&[
            "emcd_/claude-opus-5-5",
            "emcd_/claude-fable-5-1",
            "emcd2_/claude-opus-5-5",
            "gpt-6-astra",
            "gpt-6.1-sol"
        ])
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![(
            "GET /v1/models HTTP/1.1".to_string(),
            format!("Bearer {KEY}")
        )]
    );
}

#[test]
fn models_is_fetched_once_per_url_and_key() {
    let (url, seen) = serve(200, P0_LIST);
    let r = route(&url, "emcd_");
    models(&r).unwrap();
    models(&r).unwrap();
    preflight(&r, ProxyProvider::Claude, "claude-opus-5-5").unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn preflight_error_table() {
    // 401 and 403: the key.
    for status in [401u16, 403] {
        let (url, _) = serve(status, "{}");
        assert_eq!(
            preflight(
                &route(&url, "emcd_"),
                ProxyProvider::Claude,
                "claude-opus-5-5"
            ),
            Err(format!(
                "proxy rejected the key ({status}) — run rival config key set"
            ))
        );
    }
    // A failure is not cached: the same URL is asked again.
    let (url, seen) = serve(401, "{}");
    let r = route(&url, "emcd_");
    assert!(models(&r).is_err());
    assert!(models(&r).is_err());
    assert_eq!(seen.lock().unwrap().len(), 2);

    // Another status and a bad body.
    let (url, _) = serve(500, "{}");
    assert_eq!(
        preflight(&route(&url, "x"), ProxyProvider::Claude, "claude-opus-5-5"),
        Err(format!("proxy answered 500 for {url}/v1/models"))
    );
    let (url, _) = serve(200, "[]");
    let err = preflight(&route(&url, "x"), ProxyProvider::Claude, "claude-opus-5-5").unwrap_err();
    assert!(err.starts_with("proxy sent a bad model list: "), "{err}");

    // The served-id rows, on the P0 list.
    let (url, _) = serve(200, P0_LIST);
    let cases = [
        ("emcd_", ProxyProvider::Claude, "claude-opus-5-5", Ok(())),
        ("", ProxyProvider::Codex, "gpt-6.1-sol", Ok(())),
        (
            "emcd2_",
            ProxyProvider::Claude,
            "claude-fable-5-1",
            Err(
                "proxy does not serve emcd2_/claude-fable-5-1; it serves claude-fable-5-1 as: emcd_/claude-fable-5-1 — set proxy.claude.model_prefix",
            ),
        ),
        (
            "emcd3_",
            ProxyProvider::Claude,
            "claude-opus-5-5",
            Err(
                "proxy does not serve emcd3_/claude-opus-5-5; it serves claude-opus-5-5 as: emcd_/claude-opus-5-5, emcd2_/claude-opus-5-5 — set proxy.claude.model_prefix",
            ),
        ),
        (
            "",
            ProxyProvider::Claude,
            "claude-opus-5-5",
            Err(
                "the proxy needs a prefix for Claude models; it has: emcd_, emcd2_ — set proxy.claude.model_prefix",
            ),
        ),
        (
            "",
            ProxyProvider::Codex,
            "gpt-7",
            Err(
                "proxy does not serve gpt-7; no prefix serves gpt-7 — check the accounts on the proxy",
            ),
        ),
    ];
    for (prefix, provider, model, want) in cases {
        assert_eq!(
            preflight(&route(&url, prefix), provider, model),
            want.map_err(str::to_string),
            "{prefix:?} {model}"
        );
    }

    // No model of the provider at all.
    let (url, _) = serve(200, r#"{"data":[{"id":"emcd_/claude-opus-5-5"}]}"#);
    assert_eq!(
        preflight(&route(&url, ""), ProxyProvider::Codex, "gpt-6-astra"),
        Err("proxy has no codex account — log in on the proxy (-codex-device-login)".to_string())
    );
    let (url, _) = serve(200, r#"{"data":[{"id":"gpt-6-astra"}]}"#);
    assert_eq!(
        preflight(
            &route(&url, "emcd_"),
            ProxyProvider::Claude,
            "claude-opus-5-5"
        ),
        Err("proxy has no claude account — log in on the proxy (-claude-login)".to_string())
    );
}

#[test]
fn preflight_names_an_unreachable_proxy() {
    // A port that was free a moment ago: nothing listens there.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let err = preflight(
        &route(&url, "emcd_"),
        ProxyProvider::Claude,
        "claude-opus-5-5",
    )
    .unwrap_err();
    assert!(
        err.starts_with(&format!("proxy unreachable at {url}: ")),
        "{err}"
    );
    assert!(!err.contains(KEY), "{err}");
}

#[test]
fn classify_limit_names_the_other_prefixes() {
    let served = ids(&[
        "emcd_/claude-opus-5-5",
        "emcd2_/claude-opus-5-5",
        "gpt-6-astra",
    ]);
    let r = route("http://127.0.0.1:1", "emcd2_");
    let out = "API Error: 429 {\"error\":{\"type\":\"rate_limit_error\",\"message\":\"All credentials for model emcd2_/claude-opus-5-5 are cooling down (monthly spend limit)\"}}";
    assert_eq!(
        classify_limit(out, &r, ProxyProvider::Claude, "claude-opus-5-5", &served).as_deref(),
        Some(
            "proxy account emcd2_ is at its limit (429); other prefixes that serve claude-opus-5-5: emcd_ — set proxy.claude.model_prefix"
        )
    );
    // Each marker alone counts; 429 is required.
    for marker in ["cooling down", "monthly spend limit", "rate_limit_error"] {
        let text = format!("status 429: {marker}");
        assert!(
            classify_limit(&text, &r, ProxyProvider::Claude, "claude-opus-5-5", &served).is_some(),
            "{marker}"
        );
        assert!(
            classify_limit(
                marker,
                &r,
                ProxyProvider::Claude,
                "claude-opus-5-5",
                &served
            )
            .is_none(),
            "{marker} without 429"
        );
    }
    assert!(
        classify_limit(
            "429 too many",
            &r,
            ProxyProvider::Claude,
            "claude-opus-5-5",
            &served
        )
        .is_none()
    );
    // No other prefix.
    let r = route("http://127.0.0.1:1", "emcd_");
    assert_eq!(
        classify_limit(
            "429 cooling down",
            &r,
            ProxyProvider::Claude,
            "claude-fable-5-1",
            &served
        )
        .as_deref(),
        Some(
            "proxy account emcd_ is at its limit (429); no other prefix serves claude-fable-5-1 — wait for the limit to reset"
        )
    );
}

#[test]
fn group_by_prefix_keeps_list_order() {
    let groups = group_by_prefix(&ids(&[
        "emcd_/claude-opus-5-5",
        "gpt-6-astra",
        "emcd2_/claude-opus-5-5",
        "emcd_/claude-fable-5-1",
    ]));
    let flat: Vec<(String, Vec<String>)> = groups.into_iter().collect();
    assert_eq!(
        flat,
        vec![
            ("".to_string(), ids(&["gpt-6-astra"])),
            ("emcd2_".to_string(), ids(&["claude-opus-5-5"])),
            (
                "emcd_".to_string(),
                ids(&["claude-opus-5-5", "claude-fable-5-1"])
            ),
        ]
    );
}

#[test]
fn loopback_hosts_are_found_and_replaced() {
    for (url, loopback, docker) in [
        (
            "http://127.0.0.1:8317",
            true,
            "http://host.docker.internal:8317",
        ),
        (
            "http://localhost:8317/base",
            true,
            "http://host.docker.internal:8317/base",
        ),
        ("http://LOCALHOST", true, "http://host.docker.internal"),
        (
            "http://[::1]:8317",
            true,
            "http://host.docker.internal:8317",
        ),
        (
            "https://user@127.0.0.1:1",
            true,
            "https://user@host.docker.internal:1",
        ),
        (
            "https://proxy.example:9000",
            false,
            "https://proxy.example:9000",
        ),
        ("http://127.0.0.2:8317", false, "http://127.0.0.2:8317"),
    ] {
        assert_eq!(is_loopback_url(url), loopback, "{url}");
        assert_eq!(
            replace_loopback_host(url, "host.docker.internal"),
            docker,
            "{url}"
        );
    }
}
