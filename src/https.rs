//! Shared `ureq::AgentBuilder` factory with a native-tls connector
//! pre-installed.
//!
//! `ureq` v2 with `default-features = false, features = ["native-tls"]`
//! exposes the API to use a `native_tls::TlsConnector` but does NOT
//! auto-install one — the AgentBuilder still says "no TLS backend
//! configured" the moment you try to make an HTTPS request unless you
//! explicitly call `.tls_connector(...)`. The default behaviour seems
//! useful for the `rustls` story but it silently broke every HTTPS
//! call from this binary (the Discord webhook, the test-webhook
//! endpoint, and — most visibly — the Twitch `GetClientConfiguration`
//! proxy).
//!
//! The error message at runtime is exactly:
//!     `Unknown Scheme: cannot make HTTPS request because no TLS
//!     backend is configured`
//!
//! Use [`https_agent_builder`] everywhere we need to call out to an
//! HTTPS URL. Webhooks and the multi-track config proxy share this so
//! a future TLS-backend swap (e.g. moving to rustls + webpki-roots) is
//! a one-line change.

use std::sync::Arc;

/// Build an `AgentBuilder` with native-tls wired in as the TLS
/// connector. The native-tls constructor only fails on truly broken
/// systems (no platform TLS library available); on a fresh Windows
/// install this always succeeds, so we expect rather than wrap-in-Result
/// the call site. If it ever does fail the panic is the right signal
/// — every HTTPS call from this process would be unable to negotiate
/// a session anyway.
pub fn https_agent_builder() -> ureq::AgentBuilder {
    let connector = native_tls::TlsConnector::new()
        .expect("native-tls TlsConnector::new() failed — platform TLS unavailable");
    ureq::AgentBuilder::new().tls_connector(Arc::new(connector))
}
