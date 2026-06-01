//! Shared `ureq` agent factory that selects native-tls as the TLS
//! provider and lets callers see 4xx response bodies.
//!
//! ureq 3.x changes vs the 2.x setup we used to have:
//!
//!   - TLS backend selection moved from an imperative `tls_connector()`
//!     call (with a hand-built `native_tls::TlsConnector`) to a
//!     declarative `TlsConfig::builder().provider(TlsProvider::NativeTls)`.
//!     The `native-tls` cargo feature on `ureq` wires the real
//!     `native_tls` crate in for us — we just have to tell ureq to
//!     prefer it over the rustls default.
//!
//!   - 4xx / 5xx responses default to `Err(Error::StatusCode(u16))`,
//!     which DROPS the response body. The Twitch
//!     `GetClientConfiguration` proxy needs the body in the 4xx case
//!     to surface why the EB allocation failed; flipping
//!     `http_status_as_error(false)` on the agent moves non-2xx into
//!     the Ok branch so we can read the body before deciding.
//!
//!   - Timeouts moved from per-builder methods (`.timeout_connect(d)`,
//!     `.timeout(d)`) to an `Option<Duration>` config field on either
//!     the agent or, per-request, via `req.config().timeout_*().build()`.
//!
//! Use [`https_agent`] everywhere we need to call out to an HTTPS URL.

use ureq::tls::{TlsConfig, TlsProvider};
use ureq::Agent;

/// Build a ready-to-use ureq `Agent` with native-tls selected as the
/// TLS provider and `http_status_as_error` disabled so callers retain
/// access to non-2xx response bodies. Native-tls uses Windows schannel
/// under the hood — no rustls + ring dependency chain.
pub fn https_agent() -> Agent {
    Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .build(),
        )
        // Non-2xx as Ok(resp): the Twitch proxy needs the 4xx body to
        // surface why allocation failed; webhooks check
        // `resp.status()` directly. Either way, one code path.
        .http_status_as_error(false)
        .build()
        .into()
}
