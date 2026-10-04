//! Shared `ureq` agent factory that selects native-tls as the TLS
//! provider and lets callers see 4xx response bodies.
//!
//! ureq 3.x changes vs the 2.x setup we used to have:
//!
//!   - TLS backend selection moved from an imperative `tls_connector()`
//!     call (with a hand-built `native_tls::TlsConnector`) to a
//!     declarative `TlsConfig::builder().provider(TlsProvider::NativeTls)`.
//!     The `native-tls` cargo feature on `ureq` wires the real
//!     `native_tls` crate in for us - we just have to tell ureq to
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

use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

/// Build a ready-to-use ureq `Agent` with native-tls selected as the
/// TLS provider and `http_status_as_error` disabled so callers retain
/// access to non-2xx response bodies. Native-tls uses Windows schannel
/// under the hood - no rustls + ring dependency chain.
pub fn https_agent() -> Agent {
    agent(None)
}

/// `https_agent` whose every call gives up after `timeout`, connect to last
/// byte. For calls made from a loop that must keep going: a stalled server
/// would otherwise hold it forever.
pub fn https_agent_with_timeout(timeout: Duration) -> Agent {
    agent(Some(timeout))
}

fn agent(timeout: Option<Duration>) -> Agent {
    Agent::config_builder()
        .timeout_global(timeout)
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                // The OS certificate store (schannel on Windows). ureq's
                // default hands native-tls a bundled root list instead, and
                // schannel then refuses any server whose chain it builds to
                // a root outside that list: Discord's does, so every
                // webhook failed with "unable to find any user-specified
                // roots in the final cert chain".
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        // Non-2xx as Ok(resp): the Twitch proxy needs the 4xx body to
        // surface why allocation failed; webhooks check
        // `resp.status()` directly. Either way, one code path.
        .http_status_as_error(false)
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    /// Network smoke test for the TLS setup; run by hand with `--ignored`.
    #[test]
    #[ignore]
    fn reaches_real_https_hosts() {
        for url in [
            "https://discord.com/api/v10/gateway",
            "https://id.twitch.tv/oauth2/validate",
            "https://ntfy.sh/v1/health",
        ] {
            let r = super::https_agent().get(url).call();
            assert!(r.is_ok(), "{url}: {:?}", r.err());
        }
    }
}
