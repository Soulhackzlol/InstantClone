//! Why a destination can't stream, in words a streamer can act on.
//!
//! The egress loop keeps the last failed connection of each destination. The
//! dashboard card shows its advice, the scrubbed technical reason and when
//! the next attempt starts, so a wrong stream key is no longer an endless
//! "Connecting...".

use std::io;
use std::time::{Duration, Instant};

/// A publish the platform accepted and then closed within this long is read
/// as a rejected key: Twitch answers a wrong key that way, not with an RTMP
/// error.
pub const QUICK_DROP: Duration = Duration::from_secs(10);

/// Longest technical reason kept for the card, in characters.
const MAX_DETAIL_CHARS: usize = 200;

pub const KEY_REJECTED: &str = "Stream key rejected. Check the key in Edit, and that nothing else \
     is streaming with it.";
pub const CLOSED_AT_START: &str = "The platform closed the stream as soon as it started. This \
     usually means a wrong or expired stream key: check it in Edit.";
pub const REFUSED: &str =
    "The server refused the connection. Check the server address and port in Edit.";
pub const NOT_FOUND: &str =
    "Server address not found. Check your internet connection and the server address in Edit.";
pub const TLS_FAILED: &str = "The secure connection (RTMPS) failed. Check the server address in \
     Edit and that your PC's date and time are right.";
pub const TIMED_OUT: &str = "The server didn't answer in time. Check your internet connection, \
     or pick another server in Edit.";
pub const UNREACHABLE: &str = "No route to the server. Check your internet connection.";
pub const DROPPED: &str = "The connection to the platform dropped.";
pub const BAD_URL: &str = "This server address isn't valid. Fix it in Edit.";
pub const UNKNOWN: &str = "Couldn't connect to the platform.";
/// OBS is publishing Enhanced Broadcasting with a session this run of
/// InstantClone never handed out (it restarted mid-stream): Twitch only
/// takes the session OBS asks for when it starts streaming.
pub const EB_SESSION_LOST: &str = "InstantClone restarted during an Enhanced Broadcasting stream, \
     and Twitch won't take that session back. Stop and start streaming in OBS to begin a new one.";

/// The last failed attempt to stream to one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressFailure {
    /// Plain advice, one of the constants above.
    pub advice: &'static str,
    /// The technical reason, already scrubbed of the stream key.
    pub detail: String,
    pub at: Instant,
    /// When the next attempt starts. None while it is connecting, and for a
    /// failure that retrying can't fix.
    pub retry_at: Option<Instant>,
    /// False for a failure no retry fixes (a malformed address): the card
    /// then asks for an edit instead of counting down.
    pub retries: bool,
}

impl EgressFailure {
    pub fn new(advice: &'static str, detail: &str, retry_in: Option<Duration>) -> Self {
        let at = Instant::now();
        Self {
            advice,
            detail: detail.chars().take(MAX_DETAIL_CHARS).collect(),
            at,
            retry_at: retry_in.map(|d| at + d),
            retries: retry_in.is_some(),
        }
    }
}

/// Plain advice for a failed connection or a dropped session. `quick_drop`
/// is a session the platform ended within `QUICK_DROP` of accepting it.
pub fn advice_for(err: &io::Error, quick_drop: bool) -> &'static str {
    let text = err.to_string().to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    // RTMP status codes and descriptions first: they name the real cause,
    // where the socket error that follows them is generic.
    if has(&[
        "badname",
        "publish.denied",
        "unauthorized",
        "authentication",
        "auth failed",
        "forbidden",
        "stream key",
    ]) {
        return KEY_REJECTED;
    }
    if has(&["tls"]) {
        return TLS_FAILED;
    }
    // Windows' WSAHOST_NOT_FOUND (11001) and WSANO_DATA (11004), and the
    // getaddrinfo wording on Linux.
    if has(&[
        "failed to lookup",
        "no such host",
        "os error 11001",
        "os error 11004",
        "name or service not known",
        "temporary failure in name resolution",
        "nodename nor servname",
    ]) {
        return NOT_FOUND;
    }
    match err.kind() {
        io::ErrorKind::ConnectionRefused => return REFUSED,
        io::ErrorKind::TimedOut => return TIMED_OUT,
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => return UNREACHABLE,
        _ => {}
    }
    // Errors rebuilt from text (the pump's) lose their kind; WSAECONNREFUSED
    // is 10061, WSAETIMEDOUT 10060.
    if has(&["refused", "os error 10061"]) {
        return REFUSED;
    }
    if has(&["timed out", "no answer within", "os error 10060"]) {
        return TIMED_OUT;
    }
    if has(&["unreachable", "os error 10051", "os error 10065"]) {
        return UNREACHABLE;
    }
    if quick_drop {
        return CLOSED_AT_START;
    }
    let dropped = matches!(
        err.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    );
    if dropped || has(&["early eof", "reset", "aborted", "broken pipe", "closed"]) {
        return DROPPED;
    }
    UNKNOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(text: &str) -> io::Error {
        io::Error::other(text.to_string())
    }

    /// A refused publish points at the stream key; network failures say what
    /// failed on the network; anything else is honestly unknown.
    #[test]
    fn each_failure_gets_its_own_advice() {
        // rtmp publish refusals read as a rejected key
        {
            for text in [
                "RTMP NetStream.Publish.BadName: Stream already publishing",
                "RTMP NetStream.Publish.Denied: denied",
                "RTMP _error: Authentication failed",
                "RTMP _error: invalid stream key",
            ] {
                assert_eq!(advice_for(&other(text), false), KEY_REJECTED, "{text}");
            }
        }
        // a publish closed right away points at the key
        {
            let reset = io::Error::from(io::ErrorKind::ConnectionReset);
            assert_eq!(advice_for(&reset, true), CLOSED_AT_START);
            assert_eq!(advice_for(&reset, false), DROPPED);
        }
        // socket failures get their own advice
        {
            let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
            assert_eq!(advice_for(&refused, false), REFUSED);
            let timeout = io::Error::new(io::ErrorKind::TimedOut, "no answer within 20 s");
            assert_eq!(advice_for(&timeout, false), TIMED_OUT);
            let dns = other("No such host is known. (os error 11001)");
            assert_eq!(advice_for(&dns, false), NOT_FOUND);
            let linux_dns =
                other("failed to lookup address information: Name or service not known");
            assert_eq!(advice_for(&linux_dns, false), NOT_FOUND);
            let tls = other("TLS handshake failed: certificate expired");
            assert_eq!(advice_for(&tls, false), TLS_FAILED);
        }
        // errors that lost their kind are read from the text
        {
            let refused = other("No connection could be made (os error 10061)");
            assert_eq!(advice_for(&refused, false), REFUSED);
            let silent = other("operation timed out");
            assert_eq!(advice_for(&silent, false), TIMED_OUT);
        }
        // a dns failure is not mistaken for a rejected key on a quick drop
        {
            let dns = other("No such host is known. (os error 11001)");
            assert_eq!(advice_for(&dns, true), NOT_FOUND);
        }
        // anything else is a plain unknown
        {
            assert_eq!(advice_for(&other("something odd"), false), UNKNOWN);
        }
    }

    #[test]
    fn the_detail_is_capped_on_a_character_boundary() {
        let long = "é".repeat(500);
        let failure = EgressFailure::new(UNKNOWN, &long, None);
        assert_eq!(failure.detail.chars().count(), MAX_DETAIL_CHARS);
        assert!(failure.retry_at.is_none() && !failure.retries);
    }

    #[test]
    fn a_retry_delay_sets_the_next_attempt() {
        let failure = EgressFailure::new(REFUSED, "refused", Some(Duration::from_secs(4)));
        assert_eq!(failure.retry_at, Some(failure.at + Duration::from_secs(4)));
    }
}
