//! Telemetry behavior. Every client here uses a recording
//! transport: nothing is sent to Sentry, and no test reads the process env.

use super::*;

use std::collections::HashMap;
use std::sync::Mutex;

use sentry::{Envelope, Transport};

#[derive(Default)]
struct Recorder {
    envelopes: Mutex<Vec<Envelope>>,
    flushes: Mutex<Vec<Duration>>,
}

impl Transport for Recorder {
    fn send_envelope(&self, envelope: Envelope) {
        self.envelopes.lock().unwrap().push(envelope);
    }
    fn flush(&self, timeout: Duration) -> bool {
        self.flushes.lock().unwrap().push(timeout);
        true
    }
}

fn recording() -> (Arc<Recorder>, Arc<dyn TransportFactory>) {
    let rec = Arc::new(Recorder::default());
    let r = Arc::clone(&rec);
    let factory: Arc<dyn TransportFactory> =
        Arc::new(move |_: &ClientOptions| -> Arc<dyn Transport> {
            Arc::clone(&r) as Arc<dyn Transport>
        });
    (rec, factory)
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn getter<'a>(m: &'a HashMap<String, String>) -> impl Fn(&str) -> &'a str + Copy + 'a {
    move |k| m.get(k).map_or("", String::as_str)
}

#[test]
fn enabled_only_for_empty_zero_or_false() {
    for (pairs, want) in [
        (&[][..], true),
        (
            &[
                ("DO_NOT_TRACK", "0"),
                ("RIVAL_NO_TELEMETRY", "false"),
                ("CI", ""),
            ],
            true,
        ),
        (&[("DO_NOT_TRACK", "1")], false),
        (&[("RIVAL_NO_TELEMETRY", "yes")], false),
        (&[("CI", "true")], false),
        (&[("CI", "FALSE")], false),
        (&[("CI", " ")], false),
        (&[("SENTRY_DSN", "x")], true),
    ] {
        let m = env(pairs);
        assert_eq!(enabled(getter(&m)), want, "{pairs:?}");
    }
}

#[test]
fn client_options_match_init_defaults() {
    let m = env(&[]);
    let opts = client_options("1.2.3", getter(&m));
    let dsn = opts.dsn.as_ref().unwrap();
    assert_eq!(dsn.public_key(), "4cade01be5cad580635e873f91df96f5");
    assert_eq!(dsn.host(), "o4506162959220736.ingest.us.sentry.io");
    assert_eq!(dsn.project_id().to_string(), "4511041118797825");
    assert_eq!(opts.release.as_deref(), Some("rival@1.2.3"));
    assert_eq!(opts.environment.as_deref(), Some("production"));
    assert!(!opts.send_default_pii);
    assert!(matches!(
        opts.traces_sampling_strategy,
        sentry::TracesSamplingStrategy::FixedRate(r) if r == 0.0
    ));
    assert_eq!(opts.http_proxy, None);
    assert_eq!(opts.https_proxy, None);
    assert_eq!(FLUSH_TIMEOUT, Duration::from_secs(2));
    assert_eq!(
        SENTRY_DSN,
        "https://4cade01be5cad580635e873f91df96f5@o4506162959220736.ingest.us.sentry.io/4511041118797825"
    );
}

#[test]
fn client_options_take_proxies_from_the_given_env() {
    let m = env(&[("http_proxy", "http://p:1")]);
    let opts = client_options("dev", getter(&m));
    assert_eq!(opts.http_proxy.as_deref(), Some("http://p:1"));
    assert_eq!(opts.https_proxy.as_deref(), Some("http://p:1"));
    let m = env(&[("HTTPS_PROXY", "http://s:2"), ("https_proxy", "http://x:3")]);
    let opts = client_options("dev", getter(&m));
    assert_eq!(opts.https_proxy.as_deref(), Some("http://s:2"));
}

#[test]
fn disabled_init_creates_no_client() {
    let (rec, factory) = recording();
    let m = env(&[("CI", "1")]);
    let t = Telemetry::init_with("dev", getter(&m), factory);
    assert!(!t.is_enabled());
    assert!(t.flush());
    assert_eq!(t.capture_message("x", Level::Error), None);
    assert!(rec.envelopes.lock().unwrap().is_empty());
    assert!(rec.flushes.lock().unwrap().is_empty());
}

#[test]
fn explicit_capture_carries_release_and_environment_without_pii() {
    let (rec, factory) = recording();
    let m = env(&[]);
    let t = Telemetry::init_with("dev", getter(&m), factory);
    assert!(t.is_enabled());
    let id = t.capture_message("boom", Level::Error).unwrap();
    assert!(t.flush());
    assert_eq!(*rec.flushes.lock().unwrap(), [FLUSH_TIMEOUT]);
    let envelopes = rec.envelopes.lock().unwrap();
    assert_eq!(envelopes.len(), 1);
    let event = envelopes[0].event().expect("an event");
    assert_eq!(event.event_id, id);
    assert_eq!(event.message.as_deref(), Some("boom"));
    assert_eq!(event.release.as_deref(), Some("rival@dev"));
    assert_eq!(event.environment.as_deref(), Some("production"));
    assert!(event.user.is_none(), "no default PII: {:?}", event.user);
}

/// `recover_panic` never captures. No event follows a call, and no panic
/// hook is installed.
#[test]
fn recover_panic_captures_nothing() {
    let (rec, factory) = recording();
    let m = env(&[]);
    let t = Telemetry::init_with("dev", getter(&m), factory);
    t.recover_panic();
    t.flush();
    assert!(rec.envelopes.lock().unwrap().is_empty());
    assert!(t.client().unwrap().options().integrations.is_empty());
}
