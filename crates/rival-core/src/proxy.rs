//! CLIProxyAPI: the wire id, the `/v1/models` call, the preflight and the
//! 429 account-limit hint.
//!
//! The model list is fetched once per process for each URL and key, so the
//! models of one plan review share one call. Only a successful list is
//! kept; an error is fetched again on the next call.

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testserver;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use crate::config::{ProxyProvider, Route};

/// The budget of the whole `/v1/models` request.
pub const MODELS_TIMEOUT: Duration = Duration::from_secs(5);

/// The wire model id: `<prefix>/<model>`, or `<model>` when the prefix is
/// empty.
pub fn wire_id(prefix: &str, model: &str) -> String {
    if prefix.is_empty() {
        model.to_string()
    } else {
        format!("{prefix}/{model}")
    }
}

/// The prefix of a listed id: the text before the first `/`; "" when the id
/// has none.
pub fn id_prefix(id: &str) -> &str {
    id.split_once('/').map_or("", |(p, _)| p)
}

/// The bare model of a listed id: the text after the first `/`.
pub fn id_model(id: &str) -> &str {
    id.split_once('/').map_or(id, |(_, m)| m)
}

/// Whether a bare model id belongs to `provider`.
pub fn is_provider_model(provider: ProxyProvider, model: &str) -> bool {
    match provider {
        ProxyProvider::Claude => model.starts_with("claude"),
        ProxyProvider::Codex => model.starts_with("gpt-") || model.contains("codex"),
    }
}

/// Whether the URL's host is the local machine (`127.0.0.1`, `localhost`,
/// `::1`).
pub fn is_loopback_url(url: &str) -> bool {
    host_span(url).is_some_and(|(start, end)| is_loopback_host(&url[start..end]))
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

/// The byte range of the host in an `http(s)://` URL, IPv6 brackets
/// included.
fn host_span(url: &str) -> Option<(usize, usize)> {
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let auth_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_len];
    let host_start = authority.rfind('@').map_or(0, |i| i + 1);
    let host_port = &authority[host_start..];
    let host_len = if host_port.starts_with('[') {
        host_port.find(']').map_or(host_port.len(), |i| i + 1)
    } else {
        host_port.find(':').unwrap_or(host_port.len())
    };
    let start = scheme_end + host_start;
    Some((start, start + host_len))
}

/// `url` with a loopback host replaced by `host`; any other URL as is.
pub fn replace_loopback_host(url: &str, host: &str) -> String {
    match host_span(url) {
        Some((start, end)) if is_loopback_host(&url[start..end]) => {
            format!("{}{host}{}", &url[..start], &url[end..])
        }
        _ => url.to_string(),
    }
}

/// A `/v1/models` failure. The text is the preflight's error line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsError {
    /// Connection refused, timeout, DNS: `proxy unreachable at <url>: <reason>`.
    Unreachable(String),
    /// 401 or 403.
    Rejected(u16),
    /// Any other status, or a body that is not a model list.
    Bad(String),
}

impl std::fmt::Display for ModelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelsError::Unreachable(text) | ModelsError::Bad(text) => f.write_str(text),
            ModelsError::Rejected(status) => write!(
                f,
                "proxy rejected the key ({status}) — run rival config key set"
            ),
        }
    }
}

impl std::error::Error for ModelsError {}

type Cache = Mutex<HashMap<(String, String), Vec<String>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The model ids the proxy serves, in its order. One call per process for
/// each URL and key; a failure is not kept.
pub fn models(route: &Route) -> Result<Vec<String>, ModelsError> {
    let cache_key = (route.url.clone(), route.key().to_string());
    if let Some(ids) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&cache_key)
    {
        return Ok(ids.clone());
    }
    let ids = fetch_models(route)?;
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cache_key, ids.clone());
    Ok(ids)
}

/// `GET <url>/v1/models` with the key as a Bearer token. No redirects (the
/// key must not follow one), and a loopback URL never goes through an
/// `HTTP_PROXY` from the environment.
fn fetch_models(route: &Route) -> Result<Vec<String>, ModelsError> {
    let url = format!("{}/v1/models", route.url);
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(MODELS_TIMEOUT))
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false);
    if is_loopback_url(&route.url) {
        builder = builder.proxy(None);
    }
    let agent: ureq::Agent = builder.build().into();
    let unreachable = |why: String| {
        ModelsError::Unreachable(
            crate::leakguard::scrub(&format!("proxy unreachable at {}: {why}", route.url))
                .into_owned(),
        )
    };
    let mut resp = agent
        .get(&url)
        .header("Authorization", format!("Bearer {}", route.key()))
        .call()
        .map_err(|e| unreachable(e.to_string()))?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return Err(ModelsError::Rejected(status));
    }
    if status != 200 {
        return Err(ModelsError::Bad(format!(
            "proxy answered {status} for {url}"
        )));
    }
    let body = resp
        .body_mut()
        .read_to_vec()
        .map_err(|e| unreachable(e.to_string()))?;
    parse_models(&body).map_err(|e| ModelsError::Bad(format!("proxy sent a bad model list: {e}")))
}

/// The ids of an OpenAI-style list: `{"data":[{"id":"…"},…]}`.
pub fn parse_models(body: &[u8]) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }
    #[derive(Deserialize)]
    struct List {
        data: Vec<Entry>,
    }
    let list: List = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Ok(list.data.into_iter().map(|e| e.id).collect())
}

/// The listed ids grouped by prefix ("" for ids without one), each group in
/// list order.
pub fn group_by_prefix(ids: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in ids {
        groups
            .entry(id_prefix(id).to_string())
            .or_default()
            .push(id_model(id).to_string());
    }
    groups
}

/// The prefixes, in list order without repeats, of the ids that serve
/// `model`.
pub fn prefixes_serving(ids: &[String], model: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if id.contains('/') && id_model(id) == model {
            let p = id_prefix(id).to_string();
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The config key that sets the prefix of `provider`.
fn prefix_key(provider: ProxyProvider) -> String {
    format!("proxy.{}.model_prefix", provider.name())
}

/// Checks that the proxy answers, accepts the key, and serves the wire id
/// of `model`. The error is the line the user sees.
pub fn preflight(route: &Route, provider: ProxyProvider, model: &str) -> Result<(), String> {
    let ids = models(route).map_err(|e| e.to_string())?;
    check_served(&ids, route, provider, model)
}

/// [`preflight`] on a model list already fetched.
pub fn check_served(
    ids: &[String],
    route: &Route,
    provider: ProxyProvider,
    model: &str,
) -> Result<(), String> {
    let wire = wire_id(&route.prefix, model);
    if ids.contains(&wire) {
        return Ok(());
    }
    let name = provider.name();
    if !ids
        .iter()
        .any(|id| is_provider_model(provider, id_model(id)))
    {
        return Err(format!(
            "proxy has no {name} account — log in on the proxy (-{name}-{})",
            match provider {
                ProxyProvider::Claude => "login",
                ProxyProvider::Codex => "device-login",
            }
        ));
    }
    let key = prefix_key(provider);
    if provider == ProxyProvider::Claude && route.prefix.is_empty() {
        let mut has: Vec<String> = Vec::new();
        for id in ids {
            let p = id_prefix(id);
            if !p.is_empty()
                && is_provider_model(provider, id_model(id))
                && !has.iter().any(|h| h == p)
            {
                has.push(p.to_string());
            }
        }
        return Err(format!(
            "the proxy needs a prefix for Claude models; it has: {} — set {key}",
            list_or_none(&has)
        ));
    }
    let serving = prefixes_serving(ids, model);
    if serving.is_empty() {
        return Err(format!(
            "proxy does not serve {wire}; no prefix serves {model} — check the accounts on the proxy"
        ));
    }
    let as_ids: Vec<String> = serving.iter().map(|p| wire_id(p, model)).collect();
    Err(format!(
        "proxy does not serve {wire}; it serves {model} as: {} — set {key}",
        as_ids.join(", ")
    ))
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// The markers, next to `429`, of a proxy account at its limit.
const LIMIT_MARKERS: [&str; 3] = ["cooling down", "monthly spend limit", "rate_limit_error"];

/// The hint for a provider output that shows the proxy account at its
/// limit (a `429` with a limit marker), or `None`. `served` is the proxy's
/// model list; the hint names the other prefixes that serve `model`.
pub fn classify_limit(
    output: &str,
    route: &Route,
    provider: ProxyProvider,
    model: &str,
    served: &[String],
) -> Option<String> {
    if !output.contains("429") || !LIMIT_MARKERS.iter().any(|m| output.contains(m)) {
        return None;
    }
    let account = if route.prefix.is_empty() {
        "(no prefix)"
    } else {
        route.prefix.as_str()
    };
    let others: Vec<String> = prefixes_serving(served, model)
        .into_iter()
        .filter(|p| *p != route.prefix)
        .collect();
    Some(if others.is_empty() {
        format!(
            "proxy account {account} is at its limit (429); no other prefix serves {model} — wait for the limit to reset"
        )
    } else {
        format!(
            "proxy account {account} is at its limit (429); other prefixes that serve {model}: {} — set {}",
            others.join(", "),
            prefix_key(provider)
        )
    })
}
