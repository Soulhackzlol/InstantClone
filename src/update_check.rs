//! Lightweight "is there a newer release on GitHub" check.
//!
//! Calls GitHub's public Releases API, parses the `tag_name`, and compares
//! it to our compiled-in `CARGO_PKG_VERSION`. When the release is this same
//! version, a release build also compares its own SHA-256 with the one the
//! release published, so a fix shipped under the same version number is
//! offered too (`rebuild`).
//!
//! With `update_check_enabled`, [`watch_for_updates`] checks shortly after
//! start and then daily, so a tray or autostart user who never opens the
//! dashboard still hears of an update: a quiet tray notification once per
//! update on Windows, a log line elsewhere. The dashboard asks too
//! (`/update-check`); every caller shares a 10 minute cache. No automatic
//! download or install: the user decides, in System > About.
//!
//! After an update, in-app or by hand, [`whats_new_json`] says what changed
//! since the build the user last saw.
//!
//! Failures are non-fatal: if the user is offline, GitHub rate-limits
//! us, or the JSON shape changes, we silently report "couldn't check"
//! and the dashboard hides the pill.

use crate::config::Settings;
use crate::controller::Controller;
use crate::sync::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::watch;

/// The repository's API root: `/releases/latest` and `/commits` hang off it.
const GITHUB_API: &str = "https://api.github.com/repos/Soulhackzlol/InstantClone";
const RELEASES_PAGE: &str = "https://github.com/Soulhackzlol/InstantClone/releases/latest";
/// One release's notes: `{TAG_PAGE}/v0.1.16`.
const TAG_PAGE: &str = "https://github.com/Soulhackzlol/InstantClone/releases/tag";
const COMPARE_PAGE: &str = "https://github.com/Soulhackzlol/InstantClone/compare";
const CACHE_TTL: Duration = Duration::from_secs(600); // 10 min
/// Commits of the release read to find this build's: one page, GitHub's
/// largest. A build further back than that gets the release notes link.
const COMMITS_PAGE_SIZE: usize = 100;
/// The first automatic check waits for startup to settle: it is a nicety,
/// and the stream comes first.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the watcher looks at the wall clock. Hourly rather than one
/// 24 hour sleep, which a PC that sleeps overnight would stretch, and which
/// would miss the setting being switched back on for a day.
const WAKE_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    /// The update is a new build of this same version: the release
    /// published a different exe under the number already running.
    pub rebuild: bool,
    /// What the update brings over this build, newest first. None when it
    /// can't tell: a local build, or this build's commit isn't among the
    /// release's recent ones. The release notes link covers that case.
    pub changes: Option<Vec<Change>>,
    /// GitHub's page comparing this build with the release, when known.
    pub changes_url: Option<String>,
    /// For a `rebuild`, the SHA-256 the release published: it tells one
    /// rebuild of a version from the next.
    pub published_sha256: Option<String>,
    pub error: Option<String>,
}

/// One change an update brings that a user would care about, from a
/// commit subject.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// "Added", "Fixed" or "Faster", from the commit's conventional type.
    pub kind: &'static str,
    pub text: String,
}

impl UpdateInfo {
    /// A check that couldn't finish, and why.
    pub fn failed(error: impl Into<String>) -> Self {
        UpdateInfo {
            current: current_version().to_string(),
            latest: None,
            update_available: false,
            rebuild: false,
            changes: None,
            changes_url: None,
            published_sha256: None,
            error: Some(error.into()),
        }
    }

    pub fn to_json(&self) -> String {
        use crate::config::json_str;
        format!(
            r#"{{"current":{},"latest":{},"update_available":{},"rebuild":{},"changes":{},"changes_url":{},"release_url":{},"error":{}}}"#,
            json_str(&self.current),
            optional_json(&self.latest),
            self.update_available,
            self.rebuild,
            changes_json(self.changes.as_deref()),
            optional_json(&self.changes_url),
            json_str(RELEASES_PAGE),
            optional_json(&self.error),
        )
    }

    /// What identifies this update for "tell the user once": its version,
    /// plus the published hash for a new build of the same version. None
    /// when there is no update.
    fn notice_key(&self) -> Option<String> {
        if !self.update_available {
            return None;
        }
        let latest = self.latest.as_deref()?;
        Some(match &self.published_sha256 {
            Some(sha) if self.rebuild => format!("{latest}+{}", &sha[..sha.len().min(12)]),
            _ => latest.to_string(),
        })
    }

    /// The update in a few words, for the tray menu.
    fn short_label(&self) -> Option<String> {
        self.notice_key()?;
        let latest = self.latest.as_deref()?;
        Some(if self.rebuild {
            format!("New build of v{latest} available")
        } else {
            format!("Update available: v{latest}")
        })
    }

    /// The update in a sentence, for the tray notification and the log.
    fn sentence(&self) -> Option<String> {
        self.notice_key()?;
        let latest = self.latest.as_deref()?;
        Some(if self.rebuild {
            format!(
                "A new build of v{latest} is out, with fixes. Open System > About to install it."
            )
        } else {
            format!("InstantClone v{latest} is out. Open System > About to update.")
        })
    }
}

fn optional_json(v: &Option<String>) -> String {
    v.as_deref()
        .map_or("null".to_string(), crate::config::json_str)
}

fn changes_json(changes: Option<&[Change]>) -> String {
    use crate::config::json_str;
    changes.map_or("null".to_string(), |list| {
        let items: Vec<String> = list
            .iter()
            .map(|c| {
                format!(
                    r#"{{"kind":{},"text":{}}}"#,
                    json_str(c.kind),
                    json_str(&c.text)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    })
}

/// The commit this binary was built from, which the release workflow sets.
/// None for a local build.
fn build_commit() -> Option<&'static str> {
    option_env!("INSTANTCLONE_BUILD_COMMIT").filter(|commit| !commit.is_empty())
}

/// Whether this binary was built by the release workflow, which sets
/// INSTANTCLONE_RELEASE_BUILD. Only those compare their hash with the
/// published exe: a local build never matches it, so it would always look
/// out of date and offer to replace itself.
fn is_release_build() -> bool {
    option_env!("INSTANTCLONE_RELEASE_BUILD").is_some()
}

static CACHE: Mutex<Option<(Instant, UpdateInfo)>> = Mutex::new(None);

/// The tray menu's line for the update the last finished check found, from
/// any caller. Stays until a check says otherwise or the process restarts.
static AVAILABLE: Mutex<Option<String>> = Mutex::new(None);

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
    // A failed check says nothing about the update the last one found.
    if info.error.is_none() {
        *AVAILABLE.lock() = info.short_label();
    }
    info
}

/// The tray menu's "Update available" line, when a check found one.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn available_label() -> Option<String> {
    AVAILABLE.lock().clone()
}

/// The API root to ask. A debug build can point it at a local mock with
/// INSTANTCLONE_GITHUB_API, to try the update flows without GitHub; a
/// release build always asks GitHub.
fn github_api() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("INSTANTCLONE_GITHUB_API") {
        return url;
    }
    GITHUB_API.to_string()
}

fn fetch_latest() -> UpdateInfo {
    let current = current_version().to_string();
    let agent = crate::https::https_agent();
    let api = github_api();
    let req = agent
        .get(format!("{api}/releases/latest"))
        .header("User-Agent", format!("InstantClone/{current}"))
        .header("Accept", "application/vnd.github+json");
    let resp = match req.call() {
        Ok(r) => r,
        Err(e) => return UpdateInfo::failed(format!("network: {e}")),
    };
    if resp.status() != 200 {
        return UpdateInfo::failed(format!("github http {}", resp.status()));
    }
    let mut body = resp;
    let text = match body.body_mut().read_to_string() {
        Ok(t) => t,
        Err(e) => return UpdateInfo::failed(format!("body: {e}")),
    };
    let Some(latest_tag) = extract_tag_name(&text) else {
        return UpdateInfo::failed("couldn't parse tag_name from GitHub response");
    };
    let latest_stripped = strip_tag_prefix(&latest_tag).to_string();
    let newer = is_newer(&latest_stripped, &current);
    // The same version as the release: a fix republished under the same
    // number shows only as a different exe. Can't tell (offline, no
    // checksum) reads as no update, like any other failed nicety.
    let same_version = !newer && !is_newer(&current, &latest_stripped);
    let published_sha256 = if same_version && is_release_build() {
        crate::self_update::published_rebuild(&agent, &latest_stripped)
    } else {
        None
    };
    let rebuild = published_sha256.is_some();
    let (changes, changes_url) = match build_commit() {
        Some(commit) if newer || rebuild => (
            changes_since(&agent, &api, &latest_tag, commit),
            Some(format!("{COMPARE_PAGE}/{commit}...{latest_tag}")),
        ),
        _ => (None, None),
    };
    UpdateInfo {
        current,
        latest: Some(latest_stripped),
        update_available: newer || rebuild,
        rebuild,
        changes,
        changes_url,
        published_sha256,
        error: None,
    }
}

/// What `tag` (a release tag or a commit) brings over `commit`, an older
/// build's: the user-facing commits on top of it, newest first. None when
/// the request to `api` fails or `commit` isn't among the tag's last ones.
fn changes_since(agent: &ureq::Agent, api: &str, tag: &str, commit: &str) -> Option<Vec<Change>> {
    let url = format!("{api}/commits?sha={tag}&per_page={COMMITS_PAGE_SIZE}");
    let mut resp = agent
        .get(&url)
        .header("User-Agent", format!("InstantClone/{}", current_version()))
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?;
    if resp.status() != 200 {
        return None;
    }
    let body = resp.body_mut().read_to_string().ok()?;
    changes_from_commits(&body, commit)
}

/// The changes in a GitHub commit list (newest first) that come after
/// `since`. Merges are left out: the commits they bring are listed anyway.
fn changes_from_commits(body: &str, since: &str) -> Option<Vec<Change>> {
    let doc = crate::json::parse(body).ok()?;
    let mut changes = Vec::new();
    for commit in doc.as_array()? {
        if commit.str_or("sha", "").eq_ignore_ascii_case(since) {
            return Some(changes);
        }
        let parents = commit
            .get("parents")
            .and_then(|p| p.as_array())
            .map_or(0, <[_]>::len);
        let message = commit
            .path("commit.message")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let subject = message.lines().next().unwrap_or("");
        if parents <= 1 {
            changes.extend(change_from_subject(subject));
        }
    }
    None
}

/// A commit subject as a change a user cares about: `fix(scope): play the
/// screen` reads "Fixed: Play the screen". Chores, CI, docs, tests and
/// subjects without a conventional type are left out.
fn change_from_subject(subject: &str) -> Option<Change> {
    let (head, text) = subject.split_once(": ")?;
    let kind = match head.split('(').next()?.trim_end_matches('!') {
        "feat" => "Added",
        "fix" => "Fixed",
        "perf" => "Faster",
        _ => return None,
    };
    let mut chars = text.trim().chars();
    let first = chars.next()?;
    Some(Change {
        kind,
        text: first.to_uppercase().chain(chars).collect(),
    })
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

// ---- Automatic checks ----

/// Check for updates shortly after start and then daily while
/// `update_check_enabled`, and tell the user once per update: a log line
/// once per run, and on Windows a quiet tray notification once ever (unless
/// they turned it off). Runs for the life of the process. `cfg_path` is None
/// when startup must not write the config (see `Settings::load_for_startup`);
/// "told once" then lasts for this run only.
pub async fn watch_for_updates(
    settings: Arc<watch::Sender<Settings>>,
    // Only the tray's "told once" is saved, and only Windows has a tray.
    #[cfg_attr(not(windows), allow(unused_variables))] cfg_path: Option<PathBuf>,
    ctrl: Arc<Controller>,
) {
    tokio::time::sleep(FIRST_CHECK_AFTER).await;
    let mut last_check: Option<SystemTime> = None;
    let mut logged: Option<String> = None;
    loop {
        let enabled = settings.borrow().update_check_enabled;
        if enabled && check_due(last_check, SystemTime::now()) {
            let info = tokio::task::spawn_blocking(check_update)
                .await
                .unwrap_or_else(|_| UpdateInfo::failed("update check task panicked"));
            // A failed check (an autostart before the network is up) is
            // retried at the next wake, not a day later.
            if info.error.is_none() {
                last_check = Some(SystemTime::now());
            }
            if let (Some(key), Some(sentence)) = (info.notice_key(), info.sentence()) {
                if logged.as_deref() != Some(key.as_str()) {
                    ctrl.log(format!("[update] {sentence}"));
                    log_line!("[update] {sentence}");
                    logged = Some(key.clone());
                }
                #[cfg(windows)]
                tell_in_tray(&key, &sentence, &settings, cfg_path.as_deref());
            }
        }
        tokio::time::sleep(WAKE_EVERY).await;
    }
}

/// Whether the daily check is due at `now`. A clock that went backwards
/// counts as due, rather than holding checks back until it catches up.
fn check_due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        None => true,
        Some(at) => now
            .duration_since(at)
            .map_or(true, |since| since >= CHECK_EVERY),
    }
}

/// Whether the tray should announce update `key`: the user wants it, and
/// hasn't been told about this one yet.
#[cfg_attr(not(windows), allow(dead_code))]
fn tray_should_tell(s: &Settings, key: &str) -> bool {
    s.update_tray_notify && s.update_notified != key
}

/// The quiet tray notification, then remember it was given so a restart
/// doesn't repeat it.
#[cfg(windows)]
fn tell_in_tray(
    key: &str,
    sentence: &str,
    settings: &watch::Sender<Settings>,
    cfg_path: Option<&std::path::Path>,
) {
    if !tray_should_tell(&settings.borrow(), key) || !crate::tray::notify_update(sentence) {
        return;
    }
    let _wl = crate::web::settings_write_guard();
    let mut ns = settings.borrow().clone();
    ns.update_notified = key.to_string();
    if let Some(path) = cfg_path {
        let _ = ns.save(path);
    }
    let _ = settings.send(ns);
}

// ---- What's new after an update ----

/// The running build is an update over the one the user last saw.
#[derive(Debug, PartialEq)]
struct Updated {
    /// The version the user last saw. None for an install that ran before
    /// InstantClone kept track.
    from: Option<String>,
    /// That build's commit, when known.
    from_commit: Option<String>,
    /// A new build of the same version.
    rebuild: bool,
}

/// Whether the running build (`running`, built from `running_commit`) is an
/// update over the last one `s` saw. The same build or a downgrade is not.
fn updated_since(s: &Settings, running: &str, running_commit: Option<&str>) -> Option<Updated> {
    let seen = s.last_seen_version.as_str();
    if seen.is_empty() {
        // A configured install that never recorded a build ran one from
        // before InstantClone kept track, so this one is new to its user.
        // A fresh install starts from its own (see `start_from_this_build`).
        return s.configured.then_some(Updated {
            from: None,
            from_commit: None,
            rebuild: false,
        });
    }
    let seen_commit = s.last_seen_commit.as_str();
    let other_build = matches!(running_commit, Some(commit)
        if !seen_commit.is_empty() && !commit.eq_ignore_ascii_case(seen_commit));
    let newer = is_newer(running, seen);
    let same_version = !newer && !is_newer(seen, running);
    let rebuilt = same_version && other_build;
    if !newer && !rebuilt {
        return None;
    }
    Some(Updated {
        from: Some(seen.to_string()),
        from_commit: (!seen_commit.is_empty()).then(|| seen_commit.to_string()),
        rebuild: same_version,
    })
}

/// What changed since the build the user last saw, for the dashboard's
/// one-time "Updated to vX" card: `{"show":false}`, or the versions, the
/// changes between the two builds when both commits are known (asked of
/// GitHub only while update checks are on), and the release notes link.
/// Blocking: it may ask GitHub.
pub fn whats_new_json(s: &Settings) -> String {
    use crate::config::json_str;
    let running_commit = build_commit();
    let Some(updated) = updated_since(s, current_version(), running_commit) else {
        return r#"{"show":false}"#.to_string();
    };
    let span = updated.from_commit.as_deref().zip(running_commit);
    let changes = span
        .filter(|_| s.update_check_enabled)
        .and_then(|(from, to)| changes_between(from, to));
    let changes_url = span.map(|(from, to)| format!("{COMPARE_PAGE}/{from}...{to}"));
    format!(
        r#"{{"show":true,"from":{},"to":{},"rebuild":{},"changes":{},"changes_url":{},"release_url":{}}}"#,
        optional_json(&updated.from),
        json_str(current_version()),
        updated.rebuild,
        changes_json(changes.as_deref()),
        optional_json(&changes_url),
        json_str(&format!("{TAG_PAGE}/v{}", current_version())),
    )
}

/// The changes from build `from` to `to`, asked of GitHub once per run. A
/// failed ask is retried after the update check's cache time, not on every
/// dashboard load.
fn changes_between(from: &str, to: &str) -> Option<Vec<Change>> {
    type Asked = (String, Instant, Option<Vec<Change>>);
    static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
    if let Some((key, at, changes)) = ASKED.lock().as_ref() {
        if key == from && (changes.is_some() || at.elapsed() < CACHE_TTL) {
            return changes.clone();
        }
    }
    let agent = crate::https::https_agent();
    let changes = changes_since(&agent, &github_api(), to, from);
    *ASKED.lock() = Some((from.to_string(), Instant::now(), changes.clone()));
    changes
}

/// Record the running build as seen: its "what's new" is dismissed.
pub fn mark_seen(s: &mut Settings) {
    s.last_seen_version = current_version().to_string();
    s.last_seen_commit = build_commit().unwrap_or_default().to_string();
}

/// A fresh install starts from the build it was installed with, so its
/// first run isn't announced as an update. True when `s` changed.
pub fn start_from_this_build(s: &mut Settings) -> bool {
    if !s.last_seen_version.is_empty() || s.configured {
        return false;
    }
    mark_seen(s);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the spacing, and None when GitHub sends no tag.
    #[test]
    fn the_release_tag_is_read_from_githubs_answer() {
        // extract tag name handles typical github payload
        {
            let json = r#"{"url":"...","tag_name":"v0.1.3","name":"v0.1.3"}"#;
            assert_eq!(extract_tag_name(json).as_deref(), Some("v0.1.3"));
        }
        // extract tag name tolerates whitespace around colon
        {
            let json = r#"{ "tag_name" : "v9.9.9-rc.1" }"#;
            assert_eq!(extract_tag_name(json).as_deref(), Some("v9.9.9-rc.1"));
        }
        // extract tag name returns none when field absent
        {
            let json = r#"{"name":"oops","prerelease":false}"#;
            assert_eq!(extract_tag_name(json), None);
        }
    }

    /// Versions compare as SemVer says, in both directions.
    #[test]
    fn a_release_is_newer_only_when_semver_says_so() {
        for (candidate, installed, newer, why) in [
            ("0.1.4", "0.1.3", true, "a patch bump"),
            ("0.1.3", "0.1.3", false, "the same version"),
            ("0.1.2", "0.1.3", false, "an older patch"),
            ("0.2.0", "0.1.9", true, "a minor bump"),
            // Two-digit components must not sort as text.
            ("0.1.10", "0.1.9", true, "a two-digit patch"),
            ("0.1.9", "0.1.10", false, "a one-digit patch against two"),
            (
                "0.1.3",
                "0.1.3-beta.7",
                true,
                "a release over its own prerelease",
            ),
            (
                "0.1.3-beta.7",
                "0.1.3",
                false,
                "a prerelease against its release",
            ),
            ("0.1.3-beta.8", "0.1.3-beta.7", true, "the next prerelease"),
            (
                "0.1.3-beta.6",
                "0.1.3-beta.7",
                false,
                "an earlier prerelease",
            ),
            // Prerelease numbers compare as numbers (SemVer 11.4.1). As
            // text "10" sorts before "9": beta.10 would never be offered.
            (
                "0.1.3-beta.10",
                "0.1.3-beta.9",
                true,
                "prerelease 10 over 9",
            ),
            (
                "0.1.3-beta.9",
                "0.1.3-beta.10",
                false,
                "prerelease 9 against 10",
            ),
            (
                "0.1.3-beta.1.1",
                "0.1.3-beta.1",
                true,
                "a longer identifier list",
            ),
            ("0.1.3-rc", "0.1.3-2", true, "a word over a number"),
            // Tag prefixes and build metadata are not part of the version:
            // they used to parse a component as 0, so a release looked old.
            ("V1.0.0", "0.1.14", true, "an uppercase tag prefix"),
            (
                "0.1.15+build.7",
                "0.1.14",
                true,
                "build metadata on a newer one",
            ),
            ("0.1.14+build.7", "0.1.14", false, "build metadata alone"),
        ] {
            assert_eq!(
                is_newer(candidate, installed),
                newer,
                "{why}: {candidate} vs {installed}"
            );
        }
        assert_eq!(strip_tag_prefix("V1.0.0"), "1.0.0");
        assert_eq!(strip_tag_prefix("v0.1.15"), "0.1.15");
    }

    /// The error text comes from ureq or the OS and can carry newlines or
    /// control characters, which a raw JSON string cannot contain. The
    /// dashboard would then fail to parse the whole response.
    #[test]
    fn to_json_escapes_control_characters_in_the_error() {
        let info = UpdateInfo::failed("network: line one\nline \"two\"\t\\ end");
        let json = info.to_json();
        assert!(crate::config::is_valid_json(&json), "invalid JSON: {json}");
        assert!(json.contains(r#""error":"network: line one\nline \"two\"\t\\ end""#));
    }

    /// The dashboard tells a new build of the same version from a newer
    /// version by this flag.
    #[test]
    fn to_json_says_when_the_update_is_a_rebuild() {
        let info = UpdateInfo {
            current: "0.1.16".into(),
            latest: Some("0.1.16".into()),
            update_available: true,
            rebuild: true,
            changes: Some(vec![Change {
                kind: "Fixed",
                text: "Play the \"reconnect\" screen on HEVC".into(),
            }]),
            changes_url: Some("https://github.com/x/compare/a...v0.1.16".into()),
            published_sha256: Some("ab".repeat(32)),
            error: None,
        };
        let json = info.to_json();
        assert!(crate::config::is_valid_json(&json), "invalid JSON: {json}");
        assert!(
            json.contains(r#""update_available":true,"rebuild":true"#),
            "{json}"
        );
        assert!(
            json.contains(
                r#""changes":[{"kind":"Fixed","text":"Play the \"reconnect\" screen on HEVC"}]"#
            ),
            "{json}"
        );
    }

    /// The real shape of this repo's history: a fix and a chore on top of
    /// the build, a merge, then the build's own commit and older ones.
    #[test]
    fn changes_are_the_user_facing_commits_since_this_build() {
        let body = r#"[
          {"sha":"m1","parents":[{"sha":"a"},{"sha":"b"}],"commit":{"message":"Merge pull request #73 from x/fix"}},
          {"sha":"c2","parents":[{"sha":"c1"}],"commit":{"message":"chore: keep 0.1.15 in its changelog"}},
          {"sha":"c1","parents":[{"sha":"b0"}],"commit":{"message":"fix(crash-protection): play the reconnect screen on HEVC tracks\n\nLong body."}},
          {"sha":"f0","parents":[{"sha":"e0"}],"commit":{"message":"feat!: delays up to an hour"}},
          {"sha":"B0","parents":[{"sha":"a0"}],"commit":{"message":"fix: the build this binary came from"}},
          {"sha":"a0","parents":[],"commit":{"message":"fix: older than the build"}}
        ]"#;
        let changes = changes_from_commits(body, "b0").expect("the build is in the list");
        let fixed = Change {
            kind: "Fixed",
            text: "Play the reconnect screen on HEVC tracks".into(),
        };
        let added = Change {
            kind: "Added",
            text: "Delays up to an hour".into(),
        };
        assert_eq!(changes, vec![fixed, added]);
        assert_eq!(
            changes_from_commits(body, "not-there"),
            None,
            "can't tell, so say nothing"
        );
        assert_eq!(changes_from_commits("not json", "b0"), None);
    }

    #[test]
    fn only_conventional_feature_fix_and_speed_subjects_count() {
        let faster = Change {
            kind: "Faster",
            text: "Fewer copies".into(),
        };
        assert_eq!(
            change_from_subject("perf(buffer): fewer copies"),
            Some(faster)
        );
        assert_eq!(change_from_subject("ci: bump github/codeql-action"), None);
        assert_eq!(change_from_subject("release: 0.1.15 - integrations"), None);
        assert_eq!(change_from_subject("Update README"), None);
        assert_eq!(change_from_subject("fix: "), None);
    }

    fn available(latest: &str, published_sha256: Option<String>) -> UpdateInfo {
        UpdateInfo {
            current: "0.1.16".into(),
            latest: Some(latest.into()),
            update_available: true,
            rebuild: published_sha256.is_some(),
            changes: None,
            changes_url: None,
            published_sha256,
            error: None,
        }
    }

    /// The tray says each update once. A second rebuild of the same
    /// version is a different update, so it is told too.
    #[test]
    fn each_update_is_told_once_in_the_tray() {
        let newer = available("0.1.17", None);
        let rebuild_a = available("0.1.16", Some("a".repeat(64)));
        let rebuild_b = available("0.1.16", Some("b".repeat(64)));
        assert_eq!(newer.notice_key().as_deref(), Some("0.1.17"));
        assert_ne!(rebuild_a.notice_key(), rebuild_b.notice_key());
        assert_eq!(UpdateInfo::failed("offline").notice_key(), None);

        let mut s = Settings::defaults();
        assert!(tray_should_tell(&s, "0.1.17"));
        s.update_notified = "0.1.17".into();
        assert!(!tray_should_tell(&s, "0.1.17"), "never twice");
        assert!(tray_should_tell(&s, "0.1.18"));
        s.update_tray_notify = false;
        assert!(!tray_should_tell(&s, "0.1.18"), "the user turned it off");
    }

    #[test]
    fn the_tray_names_a_new_version_and_a_new_build_apart() {
        let newer = available("0.1.17", None);
        let rebuild = available("0.1.16", Some("a".repeat(64)));
        assert_eq!(
            newer.short_label().as_deref(),
            Some("Update available: v0.1.17")
        );
        assert_eq!(
            rebuild.short_label().as_deref(),
            Some("New build of v0.1.16 available")
        );
        assert!(rebuild.sentence().unwrap().contains("new build of v0.1.16"));
        assert_eq!(UpdateInfo::failed("offline").short_label(), None);
    }

    #[test]
    fn the_daily_check_is_due_after_a_day_or_when_the_clock_went_back() {
        let now = SystemTime::now();
        assert!(check_due(None, now), "the first check");
        assert!(!check_due(Some(now - Duration::from_secs(3600)), now));
        assert!(check_due(Some(now - CHECK_EVERY), now));
        assert!(check_due(Some(now + Duration::from_secs(3600)), now));
    }

    fn seen(version: &str, commit: &str) -> Settings {
        let mut s = Settings::defaults();
        s.configured = true;
        s.last_seen_version = version.into();
        s.last_seen_commit = commit.into();
        s
    }

    /// A newer version or a new build of the same one; never a downgrade, never a fresh install.
    #[test]
    fn what_counts_as_an_update_since_the_last_one_seen() {
        // a newer version than the one last seen is an update
        {
            let s = seen("0.1.15", "aaa111");
            let updated = Updated {
                from: Some("0.1.15".into()),
                from_commit: Some("aaa111".into()),
                rebuild: false,
            };
            assert_eq!(updated_since(&s, "0.1.16", Some("bbb222")), Some(updated));
            // Replaced by hand with a build that doesn't know its commit.
            assert!(updated_since(&s, "0.1.16", None).is_some());
        }
        // a new build of the same version is an update too
        {
            let s = seen("0.1.16", "aaa111");
            let rebuilt = updated_since(&s, "0.1.16", Some("bbb222")).expect("a new build");
            assert!(rebuilt.rebuild);
            assert_eq!(
                updated_since(&s, "0.1.16", Some("AAA111")),
                None,
                "same build"
            );
            // Without both commits there is no telling two builds apart.
            assert_eq!(updated_since(&s, "0.1.16", None), None);
            assert_eq!(
                updated_since(&seen("0.1.16", ""), "0.1.16", Some("b")),
                None
            );
        }
        // a downgrade is not an update
        {
            assert_eq!(
                updated_since(&seen("0.1.17", "a"), "0.1.16", Some("b")),
                None
            );
        }
        // a fresh install starts from its own build
        {
            let mut fresh = Settings::defaults();
            assert!(!fresh.configured);
            assert_eq!(updated_since(&fresh, "0.1.16", Some("a")), None);
            assert!(start_from_this_build(&mut fresh));
            assert_eq!(fresh.last_seen_version, current_version());
            assert!(!start_from_this_build(&mut fresh), "only the first run");

            // Configured, but from before InstantClone kept track: new to them.
            let mut older = seen("", "");
            assert!(!start_from_this_build(&mut older));
            let updated = updated_since(&older, "0.1.16", Some("a")).expect("an update");
            assert_eq!(updated.from, None);
        }
    }

    /// The card's payload, then gone for good once dismissed.
    #[test]
    fn whats_new_shows_until_dismissed() {
        let mut s = seen("0.0.1", "");
        s.update_check_enabled = false;
        let json = whats_new_json(&s);
        assert!(crate::config::is_valid_json(&json), "invalid JSON: {json}");
        assert!(json.contains(r#""show":true,"from":"0.0.1""#), "{json}");
        let notes = format!("{TAG_PAGE}/v{}", current_version());
        assert!(json.contains(&notes), "{json}");

        mark_seen(&mut s);
        assert_eq!(whats_new_json(&s), r#"{"show":false}"#);
    }

    /// The changes since a release tag, from a local stand-in for GitHub.
    #[test]
    fn changes_since_a_tag_come_from_the_commit_list() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (asked_tx, asked) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = asked_tx.send(request.lines().next().unwrap_or("").to_string());
            let body = r#"[
              {"sha":"new2","parents":[{"sha":"old1"}],"commit":{"message":"fix: keep the delay on restart"}},
              {"sha":"old1","parents":[{"sha":"x"}],"commit":{"message":"feat: the build last seen"}}
            ]"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
        });

        let agent = crate::https::https_agent();
        let changes = changes_since(&agent, &api, "new2", "old1");

        let fixed = Change {
            kind: "Fixed",
            text: "Keep the delay on restart".into(),
        };
        assert_eq!(changes, Some(vec![fixed]));
        let request = asked.recv().unwrap();
        assert!(
            request.starts_with("GET /commits?sha=new2&per_page=100 "),
            "{request}"
        );
    }
}
