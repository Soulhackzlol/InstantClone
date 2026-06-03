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

use std::sync::Mutex;
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
        let latest_field = match &self.latest {
            Some(v) => format!("\"{}\"", v.replace('"', "'")),
            None => "null".to_string(),
        };
        let error_field = match &self.error {
            Some(e) => format!("\"{}\"", e.replace('"', "'").replace('\\', "\\\\")),
            None => "null".to_string(),
        };
        format!(
            r#"{{"current":"{}","latest":{},"update_available":{},"release_url":"{}","error":{}}}"#,
            self.current.replace('"', "'"),
            latest_field,
            self.update_available,
            RELEASES_PAGE,
            error_field,
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
    if let Some((when, info)) = CACHE.lock().unwrap().as_ref() {
        if when.elapsed() < CACHE_TTL {
            return info.clone();
        }
    }
    let info = fetch_latest();
    *CACHE.lock().unwrap() = Some((Instant::now(), info.clone()));
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
    let latest_stripped = latest_tag.trim_start_matches('v').to_string();
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

/// SemVer-ish comparison: split on `.` and `-`, compare component by
/// component (numbers numerically, suffixes lexically). Handles
/// `0.1.3` vs `0.1.3-beta.7` correctly (prerelease loses to release of
/// the same base version). Returns true iff `latest` is strictly newer
/// than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let (lat_base, lat_pre) = split_prerelease(latest);
    let (cur_base, cur_pre) = split_prerelease(current);
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
            (Some(a), Some(b)) => a > b,
        },
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
}
