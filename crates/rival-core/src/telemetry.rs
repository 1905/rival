//! Crash telemetry through Sentry. Go: `internal/telemetry/telemetry.go`.
//!
//! The Go wiring never sends an event in practice: nothing calls a capture
//! API, and `RecoverPanic` cannot recover (see [`Telemetry::recover_panic`]).
//! The port keeps that: no panic hook, no automatic error events. Sentry's
//! built-in ureq transport sends on one background thread; [`Telemetry::flush`]
//! waits at most [`FLUSH_TIMEOUT`] for it.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use sentry::types::Uuid;
use sentry::{Client, ClientOptions, Hub, Level, Scope, TransportFactory};

#[cfg(test)]
mod tests;

pub const SENTRY_DSN: &str = "https://4cade01be5cad580635e873f91df96f5@o4506162959220736.ingest.us.sentry.io/4511041118797825";
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// Any of these set to anything but empty, `0` or `false`
/// turns telemetry off.
pub const OPT_OUT_VARS: [&str; 3] = ["DO_NOT_TRACK", "RIVAL_NO_TELEMETRY", "CI"];

pub fn enabled<'a>(getenv: impl Fn(&str) -> &'a str) -> bool {
    OPT_OUT_VARS
        .iter()
        .all(|key| matches!(getenv(key), "" | "0" | "false"))
}

/// Go `sentry.ClientOptions` of `Init`, without the transport. Proxies come
/// from the environment, as Go's transport uses `http.ProxyFromEnvironment`.
pub fn client_options<'a>(version: &str, getenv: impl Fn(&str) -> &'a str) -> ClientOptions {
    let mut opts = ClientOptions::new();
    opts.dsn = Some(SENTRY_DSN.parse().expect("valid DSN"));
    opts.release = Some(Cow::Owned(format!("rival@{version}")));
    opts.environment = Some(Cow::Borrowed("production"));
    opts.send_default_pii = false;
    opts.traces_sampling_strategy = sentry::TracesSamplingStrategy::FixedRate(0.0);
    opts.shutdown_timeout = FLUSH_TIMEOUT;
    let first = |keys: [&str; 2]| {
        keys.iter()
            .map(|k| getenv(k))
            .find(|v| !v.is_empty())
            .map(|v| Cow::Owned(v.to_string()))
    };
    opts.http_proxy = first(["HTTP_PROXY", "http_proxy"]);
    opts.https_proxy = first(["HTTPS_PROXY", "https_proxy"]).or_else(|| opts.http_proxy.clone());
    opts
}

/// The initialized client, if telemetry is enabled. Go keeps it in the
/// global hub plus an `enabled` flag.
#[derive(Default)]
pub struct Telemetry {
    client: Option<Arc<Client>>,
}

impl Telemetry {
    /// Go `Init(version)` with the real transport. Binds the client to the
    /// main hub, as `sentry.Init` sets the global hub.
    pub fn init<'a>(version: &str, getenv: impl Fn(&str) -> &'a str + Copy) -> Telemetry {
        let t = Self::init_with(
            version,
            getenv,
            Arc::new(sentry::transports::DefaultTransportFactory),
        );
        if let Some(client) = &t.client {
            Hub::main().bind_client(Some(Arc::clone(client)));
        }
        t
    }

    /// [`Telemetry::init`] with an injected transport and no global hub.
    pub fn init_with<'a>(
        version: &str,
        getenv: impl Fn(&str) -> &'a str + Copy,
        transport: Arc<dyn TransportFactory>,
    ) -> Telemetry {
        if !enabled(getenv) {
            return Telemetry::default();
        }
        let mut opts = client_options(version, getenv);
        opts.transport = Some(transport);
        Telemetry {
            client: Some(Arc::new(Client::with_options(opts))),
        }
    }

    /// Go's `enabled` flag: `Init` ran past the opt-out check.
    pub fn is_enabled(&self) -> bool {
        self.client.is_some()
    }

    pub fn client(&self) -> Option<&Arc<Client>> {
        self.client.as_ref()
    }

    /// Waits at most [`FLUSH_TIMEOUT`] when enabled.
    pub fn flush(&self) -> bool {
        match &self.client {
            Some(client) => client.flush(Some(FLUSH_TIMEOUT)),
            None => true,
        }
    }

    /// Go `RecoverPanic`, a deferred call to `sentry.Recover()`. Go's
    /// `recover()` only stops a panic when the deferred function calls it
    /// directly; `sentry.Recover` is one call deeper, so it returns nil and
    /// nothing is captured or recovered. This port keeps the no-op: a panic
    /// is never reported automatically.
    pub fn recover_panic(&self) {}

    /// An explicit capture through this client (tests and future callers).
    /// Returns `None` when telemetry is disabled.
    pub fn capture_message(&self, message: &str, level: Level) -> Option<Uuid> {
        let client = self.client.as_ref()?;
        let hub = Hub::new(Some(Arc::clone(client)), Arc::new(Scope::default()));
        Some(hub.capture_message(message, level))
    }
}
