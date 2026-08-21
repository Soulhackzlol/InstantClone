//! Optional dashboard-auth runtime state: live session tokens plus a reusable
//! failed-attempt rate limiter. Only exercised when a password / ingest key is
//! set; a default install never constructs a session or records an attempt.
//!
//! Held as an `Arc<AuthState>` created in `main`, so sessions survive a web
//! supervisor restart (port change). The password hash and dock token live in
//! `Settings`; this holds only the ephemeral state.

use crate::crypto;
use crate::sync::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Session lifetime. Long enough that a streamer is not re-prompted mid-stream,
/// short enough that a stolen cookie does not live forever.
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

struct Attempt {
    fails: u32,
    last: Instant,
    locked_until: Option<Instant>,
}

/// Per-client failed-attempt limiter with exponential lockout. Shared by the
/// dashboard login and the RTMP ingest-key check so both throttle guessing the
/// same way. `check` is O(1) and must be called BEFORE any expensive work
/// (a password hash, a wire round-trip) so a locked-out client costs nothing.
pub struct RateLimiter {
    attempts: Mutex<HashMap<String, Attempt>>,
    max_fails: u32,
    base_lockout: Duration,
    max_lockout: Duration,
    window: Duration,
}

impl RateLimiter {
    pub fn new(
        max_fails: u32,
        base_lockout: Duration,
        max_lockout: Duration,
        window: Duration,
    ) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_fails,
            base_lockout,
            max_lockout,
            window,
        }
    }

    /// `Err(remaining)` when `client` is currently locked out, else `Ok`.
    pub fn check(&self, client: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut a = self.attempts.lock();
        if let Some(e) = a.get(client) {
            // A quiet window forgives an honest user who fumbled earlier.
            if now.duration_since(e.last) > self.window {
                a.remove(client);
                return Ok(());
            }
            if let Some(until) = e.locked_until {
                if until > now {
                    return Err(until - now);
                }
            }
        }
        Ok(())
    }

    /// Record a failed attempt and (re)arm the lockout: exponential in the
    /// number of failures past the threshold, capped at `max_lockout`.
    pub fn record_failure(&self, client: &str) {
        let now = Instant::now();
        let mut a = self.attempts.lock();
        // Bound the map so an attacker cycling source IPs (a whole IPv6 /64 is
        // routable to one host) can't grow it without limit. Drop entries past
        // the forgiveness window first; if a genuine flood keeps it full, evict
        // the single oldest to make room. Caps memory at ~MAX_TRACKED entries.
        const MAX_TRACKED: usize = 8192;
        if a.len() >= MAX_TRACKED {
            a.retain(|_, e| now.duration_since(e.last) <= self.window);
            if a.len() >= MAX_TRACKED {
                if let Some(oldest) = a.iter().min_by_key(|(_, e)| e.last).map(|(k, _)| k.clone()) {
                    a.remove(&oldest);
                }
            }
        }
        let e = a.entry(client.to_string()).or_insert(Attempt {
            fails: 0,
            last: now,
            locked_until: None,
        });
        e.fails = e.fails.saturating_add(1);
        e.last = now;
        if e.fails >= self.max_fails {
            let over = (e.fails - self.max_fails).min(6); // cap the shift
            let lock = (self.base_lockout * (1u32 << over)).min(self.max_lockout);
            e.locked_until = Some(now + lock);
        }
    }

    /// Clear a client's record after a success.
    pub fn record_success(&self, client: &str) {
        self.attempts.lock().remove(client);
    }
}

pub struct AuthState {
    sessions: Mutex<HashMap<String, Instant>>, // token -> expiry
    login: RateLimiter,
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            // 5 failures then an exponential lockout, 15s base up to 15min,
            // forgiven after a 15min quiet window. Paired with the ~230ms
            // PBKDF2 cost this throttles online guessing to a crawl.
            login: RateLimiter::new(
                5,
                Duration::from_secs(15),
                Duration::from_secs(15 * 60),
                Duration::from_secs(15 * 60),
            ),
        }
    }

    /// Mint a fresh 256-bit session token valid for `SESSION_TTL`.
    pub fn create_session(&self) -> String {
        let token = crypto::random_token();
        let mut s = self.sessions.lock();
        s.insert(token.clone(), Instant::now() + SESSION_TTL);
        // Opportunistic prune so the map cannot grow without bound.
        let now = Instant::now();
        s.retain(|_, exp| *exp > now);
        token
    }

    /// True only if `token` names a live, unexpired session. Tokens are 256-bit
    /// CSPRNG output, so a HashMap lookup here is not a meaningful timing oracle
    /// (there is nothing to guess byte-by-byte); constant-time would only cost
    /// the O(1) lookup for no security gain.
    pub fn validate_session(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        match self.sessions.lock().get(token) {
            Some(exp) => *exp > Instant::now(),
            None => false,
        }
    }

    pub fn revoke_session(&self, token: &str) {
        self.sessions.lock().remove(token);
    }

    /// Drop every session. Used on password change / disable so old cookies
    /// stop working immediately.
    pub fn revoke_all(&self) {
        self.sessions.lock().clear();
    }

    // Login limiter, delegated so web.rs keeps its small surface.
    pub fn check_login_allowed(&self, client: &str) -> Result<(), Duration> {
        self.login.check(client)
    }
    pub fn record_failure(&self, client: &str) {
        self.login.record_failure(client)
    }
    pub fn record_success(&self, client: &str) {
        self.login.record_success(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lifecycle() {
        let a = AuthState::new();
        let t = a.create_session();
        assert!(a.validate_session(&t));
        assert!(!a.validate_session("not-a-token"));
        assert!(!a.validate_session(""));
        a.revoke_session(&t);
        assert!(!a.validate_session(&t));
    }

    #[test]
    fn revoke_all_drops_every_session() {
        let a = AuthState::new();
        let t1 = a.create_session();
        let t2 = a.create_session();
        a.revoke_all();
        assert!(!a.validate_session(&t1));
        assert!(!a.validate_session(&t2));
    }

    #[test]
    fn lockout_after_threshold_and_cleared_on_success() {
        let a = AuthState::new();
        let ip = "10.0.0.1";
        assert!(a.check_login_allowed(ip).is_ok());
        for _ in 0..5 {
            a.record_failure(ip);
        }
        assert!(a.check_login_allowed(ip).is_err());
        a.record_success(ip);
        assert!(a.check_login_allowed(ip).is_ok());
    }

    #[test]
    fn rate_limiter_locks_and_forgives_on_success() {
        let r = RateLimiter::new(
            3,
            Duration::from_secs(30),
            Duration::from_secs(600),
            Duration::from_secs(600),
        );
        let c = "peer";
        assert!(r.check(c).is_ok());
        r.record_failure(c);
        r.record_failure(c);
        assert!(r.check(c).is_ok()); // 2 < 3, still allowed
        r.record_failure(c);
        assert!(r.check(c).is_err()); // 3rd failure locks
        r.record_success(c);
        assert!(r.check(c).is_ok()); // success clears the lock
    }
}
