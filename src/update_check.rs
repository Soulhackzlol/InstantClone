//! Lightweight "is there a newer release on GitHub" check.
//!
//! Calls GitHub's public Releases API once per process lifetime (and on
//! demand from the dashboard's `/update-check` endpoint), parses the
//! `tag_name`, and compares it to our compiled-in `CARGO_PKG_VERSION`.
//! No automatic download or install - the dashboard surfaces a small
//! pill that links to the release page if there's a newer build.
//!
//! Failures are non-fatal: if the user is offline, GitHub rate-limits
//! us, or the JSON shape changes, we silently report "couldn't check"
//! and the dashboard hides the pill.

use crate::sync::Mutex;
use std::time::{Duration, Instant};

const RELEASES_URL: &str = "https://api.github.com/repos/Soulhackzlol/InstantClone/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/Soulhackzlol/InstantClone/releases/latest";
const CACHE_TTL: Duration = Duration::from_secs(600); // 10 min

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub error: Option<String>,
}

impl UpdateInfo {
    pub fn to_json(&self) -> String {
        use crate::config::json_str;
        let optional = |v: &Option<String>| v.as_deref().map_or("null".to_string(), json_str);
        format!(
            r#"{{"current":{},"latest":{},"update_available":{},"release_url":{},"error":{}}}"#,
            json_str(&self.current),
            optional(&self.latest),
            self.update_available,
            json_str(RELEASES_PAGE),
            optional(&self.error),
        )
    }
}

static CACHE: Mutex<Option<(Instant, UpdateInfo)>> = Mutex::new(None);

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the cached result if fresh, otherwise fetches GitHub and
/// updates the cache. The fetch happens in-line (no spawn) because the
/// call site is the dashboard endpoint, which already runs on the web
/// worker thread - a 10-second timeout caps the worst case.
pub fn check_update() -> UpdateInfo {
    if let Some((when, info)) = CACHE.lock().as_ref() {
        if when.elapsed() < CACHE_TTL {
            return info.clone();
        }
    }
    let info = fetch_latest();
    *CACHE.lock() = Some((Instant::now(), info.clone()));
    info
}

fn fetch_latest() -> UpdateInfo {
    let current = current_version().to_string();
    let agent = crate::https::https_agent();
    let req = agent
        .get(RELEASES_URL)
        .header("User-Agent", format!("InstantClone/{current}"))
        .header("Accept", "application/vnd.github+json");
    let resp = match req.call() {
        Ok(r) => r,
        Err(e) => {
            return UpdateInfo {
                current: current.clone(),
                latest: None,
                update_available: false,
                error: Some(format!("network: {e}")),
            };
        }
    };
    if resp.status() != 200 {
        return UpdateInfo {
            current: current.clone(),
            latest: None,
            update_available: false,
            error: Some(format!("github http {}", resp.status())),
        };
    }
    let mut body = resp;
    let text = match body.body_mut().read_to_string() {
        Ok(t) => t,
        Err(e) => {
            return UpdateInfo {
                current: current.clone(),
                latest: None,
                update_available: false,
                error: Some(format!("body: {e}")),
            };
        }
    };
    let latest_tag = match extract_tag_name(&text) {
        Some(t) => t,
        None => {
            return UpdateInfo {
                current: current.clone(),
                latest: None,
                update_available: false,
                error: Some("couldn't parse tag_name from GitHub response".to_string()),
            };
        }
    };
    let latest_stripped = strip_tag_prefix(&latest_tag).to_string();
    let update_available = is_newer(&latest_stripped, &current);
    UpdateInfo {
        current,
        latest: Some(latest_stripped),
        update_available,
        error: None,
    }
}

/// Pull the `"tag_name"` value out of the JSON body without a full JSON
/// parse. The GitHub Releases API guarantees a top-level string field
/// here for every published release, so a careful substring + bounded
/// scan is enough; pulling in `serde_json` for one field is overkill.
fn extract_tag_name(body: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let key_pos = body.find(key)?;
    let after_key = &body[key_pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let quote_start = after_colon.find('"')?;
    let value_slice = &after_colon[quote_start + 1..];
    let quote_end = value_slice.find('"')?;
    Some(value_slice[..quote_end].to_string())
}

/// SemVer comparison: base components numerically, then the prerelease
/// per SemVer 11.4 (see `compare_prerelease`). Handles `0.1.3` vs
/// `0.1.3-beta.7` correctly (prerelease loses to release of the same base
/// version). A `v`/`V` tag prefix and `+build` metadata are ignored.
/// Returns true iff `latest` is strictly newer than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let (lat_base, lat_pre) = split_prerelease(strip_build_metadata(strip_tag_prefix(latest)));
    let (cur_base, cur_pre) = split_prerelease(strip_build_metadata(strip_tag_prefix(current)));
    let lat_parts = parse_dotted(lat_base);
    let cur_parts = parse_dotted(cur_base);
    match lat_parts.cmp(&cur_parts) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (lat_pre, cur_pre) {
            // Same base. Release > prerelease > different prerelease.
            (None, None) => false,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => compare_prerelease(a, b).is_gt(),
        },
    }
}

/// `v0.1.15` / `V0.1.15` -> `0.1.15`. Release tags are written by hand, so
/// the prefix case is not guaranteed.
fn strip_tag_prefix(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

/// SemVer build metadata (`+...`) carries no precedence, so it is dropped.
fn strip_build_metadata(v: &str) -> &str {
    v.split_once('+').map_or(v, |(version, _)| version)
}

/// SemVer 11.4 prerelease precedence: dot-separated identifiers compared
/// left to right, numeric ones as numbers (so `beta.10` > `beta.9`),
/// numeric below alphanumeric, and a longer list wins a shared prefix.
fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        let (l, r) = match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => (l, r),
        };
        let order = match (l.parse::<u64>(), r.parse::<u64>()) {
            (Ok(l), Ok(r)) => l.cmp(&r),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => l.cmp(r),
        };
        if order.is_ne() {
            return order;
        }
    }
}

fn split_prerelease(v: &str) -> (&str, Option<&str>) {
    match v.split_once('-') {
        Some((base, pre)) => (base, Some(pre)),
        None => (v, None),
    }
}

fn parse_dotted(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tag_name_handles_typical_github_payload() {
        let json = r#"{"url":"...","tag_name":"v0.1.3","name":"v0.1.3"}"#;
        assert_eq!(extract_tag_name(json).as_deref(), Some("v0.1.3"));
    }

    #[test]
    fn extract_tag_name_tolerates_whitespace_around_colon() {
        let json = r#"{ "tag_name" : "v9.9.9-rc.1" }"#;
        assert_eq!(extract_tag_name(json).as_deref(), Some("v9.9.9-rc.1"));
    }

    #[test]
    fn extract_tag_name_returns_none_when_field_absent() {
        let json = r#"{"name":"oops","prerelease":false}"#;
        assert_eq!(extract_tag_name(json), None);
    }

    #[test]
    fn is_newer_patch_bump() {
        assert!(is_newer("0.1.4", "0.1.3"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.2", "0.1.3"));
    }

    #[test]
    fn is_newer_minor_bump() {
        assert!(is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn is_newer_release_beats_same_base_prerelease() {
        assert!(is_newer("0.1.3", "0.1.3-beta.7"));
        assert!(!is_newer("0.1.3-beta.7", "0.1.3"));
    }

    #[test]
    fn is_newer_prerelease_progression() {
        assert!(is_newer("0.1.3-beta.8", "0.1.3-beta.7"));
        assert!(!is_newer("0.1.3-beta.6", "0.1.3-beta.7"));
    }

    /// Prerelease numbers compare as numbers (SemVer 11.4.1). As text,
    /// "10" sorts before "9", so beta.10 would never be offered to beta.9.
    #[test]
    fn is_newer_prerelease_numbers_compare_numerically() {
        assert!(is_newer("0.1.3-beta.10", "0.1.3-beta.9"));
        assert!(!is_newer("0.1.3-beta.9", "0.1.3-beta.10"));
        // A longer identifier list wins when the shared prefix is equal,
        // and a numeric identifier sorts below an alphanumeric one.
        assert!(is_newer("0.1.3-beta.1.1", "0.1.3-beta.1"));
        assert!(is_newer("0.1.3-rc", "0.1.3-2"));
    }

    /// Two-digit components must not sort as text either.
    #[test]
    fn is_newer_two_digit_patch_beats_one_digit() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.9", "0.1.10"));
    }

    /// Tag prefixes and build metadata are not part of the version: an
    /// uppercase `V` or a `+build` suffix used to parse a component as 0,
    /// so the release looked older than what is installed.
    #[test]
    fn is_newer_ignores_tag_prefix_case_and_build_metadata() {
        assert!(is_newer("V1.0.0", "0.1.14"));
        assert!(is_newer("0.1.15+build.7", "0.1.14"));
        assert!(!is_newer("0.1.14+build.7", "0.1.14"));
        assert_eq!(strip_tag_prefix("V1.0.0"), "1.0.0");
        assert_eq!(strip_tag_prefix("v0.1.15"), "0.1.15");
    }

    /// The error text comes from ureq or the OS and can carry newlines or
    /// control characters, which a raw JSON string cannot contain. The
    /// dashboard would then fail to parse the whole response.
    #[test]
    fn to_json_escapes_control_characters_in_the_error() {
        let info = UpdateInfo {
            current: "0.1.14".into(),
            latest: None,
            update_available: false,
            error: Some("network: line one\nline \"two\"\t\\ end".into()),
        };
        let json = info.to_json();
        assert!(crate::config::is_valid_json(&json), "invalid JSON: {json}");
        assert!(json.contains(r#""error":"network: line one\nline \"two\"\t\\ end""#));
    }
}
