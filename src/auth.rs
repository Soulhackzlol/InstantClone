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

    /// Reject if `client` is locked out; otherwise count this attempt and arm
    /// the lockout if it crosses the threshold - all under a single lock. Use
    /// this when the check and the verification it guards are separated by an
    /// `.await` (login runs PBKDF2 on a blocking thread): counting at check time
    /// stops a burst of concurrent requests from all clearing the gate before
    /// any failure is recorded. Clear on success with `record_success`; a plain
    /// failure needs no further call.
    pub fn check_and_count(&self, client: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut a = self.attempts.lock();
        if let Some(e) = a.get(client) {
            if now.duration_since(e.last) > self.window {
                a.remove(client);
            } else if let Some(until) = e.locked_until {
                if until > now {
                    return Err(until - now);
                }
            }
        }
        self.bump_failure(&mut a, client, now);
        Ok(())
    }

    /// Record a failed attempt and (re)arm the lockout: exponential in the
    /// number of failures past the threshold, capped at `max_lockout`.
    pub fn record_failure(&self, client: &str) {
        let now = Instant::now();
        let mut a = self.attempts.lock();
        self.bump_failure(&mut a, client, now);
    }

    /// Increment a client's failure count under an already-held lock, arming the
    /// exponential lockout past the threshold and keeping the map bounded.
    fn bump_failure(&self, a: &mut HashMap<String, Attempt>, client: &str, now: Instant) {
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
            // Cap the shift so the doubling can't overflow.
            let over = (e.fails - self.max_fails).min(6);
            // Never lock past the forgiveness window: `check` frees an entry once
            // it's older than `window`, so a lockout longer than that could be
            // wiped early. Clamp to keep the two consistent regardless of config.
            let lock = (self.base_lockout * (1u32 << over))
                .min(self.max_lockout)
                .min(self.window);
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

    // Login limiter, delegated so web.rs keeps its small surface. The gate
    // counts the attempt as it checks (`check_and_count`) so a burst of
    // concurrent logins can't slip past the lockout before the async password
    // hash resolves; a success clears the record.
    pub fn begin_login_attempt(&self, client: &str) -> Result<(), Duration> {
        self.login.check_and_count(client)
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
        // Each attempt counts as it's checked. Five are allowed, the sixth is
        // locked out; a success clears the record.
        for _ in 0..5 {
            assert!(a.begin_login_attempt(ip).is_ok());
        }
        assert!(a.begin_login_attempt(ip).is_err());
        a.record_success(ip);
        assert!(a.begin_login_attempt(ip).is_ok());
    }

    /// The lockout `client` is currently serving, read back through `check`.
    /// Measured just after the failure that armed it, so it sits a hair under
    /// the armed value; `assert_lockout` allows for that.
    fn remaining_lockout(r: &RateLimiter, client: &str) -> Duration {
        r.check(client).expect_err("client should be locked out")
    }

    fn assert_lockout(r: &RateLimiter, client: &str, want_secs: u64) {
        let left = remaining_lockout(r, client);
        let want = Duration::from_secs(want_secs);
        assert!(
            left <= want && left > want - Duration::from_secs(1),
            "expected a ~{want_secs}s lockout, got {left:?}"
        );
    }

    /// Each failure past the threshold doubles the lockout until it reaches
    /// `max_lockout`, where it stays. Read back without sleeping: `check`
    /// reports how long is left, which right after arming is the armed value.
    #[test]
    fn lockout_doubles_past_the_threshold_up_to_the_cap() {
        let r = RateLimiter::new(
            2,
            Duration::from_secs(10),
            Duration::from_secs(45),
            Duration::from_secs(3600),
        );
        let c = "peer";
        r.record_failure(c);
        assert!(r.check(c).is_ok(), "one failure is under the threshold");
        for want in [10, 20, 40, 45, 45] {
            r.record_failure(c);
            assert_lockout(&r, c, want);
        }
    }

    /// A lockout never outlasts the forgiveness window. `check` forgets an
    /// entry once it is older than the window, so a longer lock would be
    /// wiped early and the two settings would disagree about when a client
    /// may try again.
    #[test]
    fn lockout_is_clamped_to_the_forgiveness_window() {
        let r = RateLimiter::new(
            1,
            Duration::from_secs(10),
            Duration::from_secs(3600),
            Duration::from_secs(25),
        );
        let c = "peer";
        for want in [10, 20, 25, 25] {
            r.record_failure(c);
            assert_lockout(&r, c, want);
        }
    }

    /// A client that keeps failing must not overflow the doubling. Without the
    /// cap on the shift, the 33rd failure past the threshold is `1u32 << 32`:
    /// a panic in a debug build, and in release a shift that wraps back to a
    /// short lockout, handing the most persistent guesser the fastest retries.
    #[test]
    fn endless_failures_do_not_overflow_the_doubling() {
        let day = Duration::from_secs(24 * 3600);
        let r = RateLimiter::new(1, Duration::from_secs(1), day, day);
        let c = "peer";
        for _ in 0..100 {
            r.record_failure(c);
        }
        // The shift is capped at 6: base * 64.
        assert_lockout(&r, c, 64);
    }

    /// The tracked-client map is bounded, and when a flood fills it the
    /// oldest entry is the one dropped. `MAX_TRACKED` is private to
    /// `bump_failure`; the 8192 here mirrors it.
    #[test]
    fn a_full_table_evicts_the_oldest_client() {
        const MAX_TRACKED: usize = 8192;
        let r = RateLimiter::new(
            1,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(3600),
        );
        r.record_failure("oldest");
        // Make every later entry strictly newer than "oldest" so it is the
        // unique minimum. A busy-wait on the clock, well under a microsecond,
        // rather than a sleep.
        let armed = Instant::now();
        while Instant::now() <= armed {}
        for i in 1..MAX_TRACKED {
            r.record_failure(&format!("client-{i}"));
        }
        assert_eq!(r.attempts.lock().len(), MAX_TRACKED, "table is full");

        r.record_failure("newcomer");
        assert_eq!(r.attempts.lock().len(), MAX_TRACKED, "table stays bounded");
        assert!(r.check("oldest").is_ok(), "the oldest entry was evicted");
        assert!(r.check("client-1").is_err(), "newer entries are kept");
        assert!(r.check("newcomer").is_err(), "the newcomer is tracked");
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
