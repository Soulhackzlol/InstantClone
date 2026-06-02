//! OBS Studio services.json patcher.
//!
//! OBS knows about streaming services through a `services.json` file
//! in the user's `AppData` directory. Each entry declares the server
//! URLs, recommended encoder settings, and - critically for our
//! use-case - the `multitrack_video_configuration_url` that turns on
//! the Enhanced Broadcasting / multi-track-video UI inside OBS.
//!
//! By inserting an InstantClone entry into that file we let the
//! streamer pick us straight from the OBS service dropdown - no
//! Custom-server fiddling, no manual JSON paste, multi-track Video
//! "Auto" buttons work because they hit our local config endpoint.
//!
//! No `serde_json` dependency - the file is well-formed JSON produced
//! by OBS, so a careful string-level insert + an idempotent check by
//! exact-substring match handles all the cases we care about. Every
//! write goes via a `.bak` copy first; if anything goes sideways the
//! user can restore manually.
//!
//! Location resolution priority:
//!   1. `$APPDATA\obs-studio\plugin_config\rtmp-services\services.json`
//!      (the OBS-on-Windows standard path)
//!   2. `$LOCALAPPDATA\obs-studio\plugin_config\rtmp-services\services.json`
//!      (rare - some portable installs land here)
//!
//! macOS / Linux paths are not handled - the rest of the app is
//! Windows-only by design.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Locate the user's `services.json`. Returns `None` if neither
/// candidate path exists - typical when OBS isn't installed at all.
pub fn services_json_path() -> Option<PathBuf> {
    let candidates = [
        std::env::var("APPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("obs-studio/plugin_config/rtmp-services/services.json")),
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("obs-studio/plugin_config/rtmp-services/services.json")),
    ];
    candidates.into_iter().flatten().find(|c| c.exists())
}

/// True when the user's services.json already contains an entry whose
/// `name` matches our `SERVICE_NAME`. Used by the System tab to render
/// the button as "Unregister" instead of "Register".
pub fn is_registered() -> bool {
    let Some(p) = services_json_path() else {
        return false;
    };
    matches!(fs::read_to_string(&p), Ok(s) if entry_exists(&s))
}

/// Build the services.json entry for InstantClone. The
/// `multitrack_video_configuration_url` is what OBS hits when the user
/// enables multi-track video - we serve it from our own web port.
fn entry_json(web_port: u16, ingest_port: u16) -> String {
    format!(
        r#"{{
            "name": "InstantClone",
            "common": true,
            "more_info_link": "https://github.com/Soulhackzlol/InstantClone",
            "stream_key_link": "http://127.0.0.1:{web}/",
            "multitrack_video_configuration_url": "http://127.0.0.1:{web}/obs/multitrack-config",
            "multitrack_video_name": "Multi-track via InstantClone",
            "multitrack_video_learn_more_link": "https://github.com/Soulhackzlol/InstantClone",
            "servers": [
                {{
                    "name": "InstantClone (local proxy)",
                    "url": "rtmp://127.0.0.1:{rtmp}/live"
                }}
            ],
            "recommended": {{
                "keyint": 2,
                "profile": "main",
                "max video bitrate": 50000,
                "max audio bitrate": 320
            }},
            "supported video codecs": ["h264"]
        }}"#,
        web = web_port,
        rtmp = ingest_port,
    )
}

/// Check whether an "InstantClone" entry is present anywhere in
/// services.json. Exact-substring match on the name key - services.json
/// is OBS-generated and stable enough that this won't false-positive on
/// e.g. a "FooInstantCloneBar" service name (no such service exists in
/// the upstream services.json).
fn entry_exists(file: &str) -> bool {
    file.contains(r#""name": "InstantClone""#) || file.contains(r#""name":"InstantClone""#)
}

/// Register InstantClone with OBS. Idempotent and self-healing - a
/// pre-existing entry is removed and replaced so a changed web_port
/// (e.g. user retuned the dashboard port in System settings) refreshes
/// the URL OBS will hit. Returns an `io::Error` with a user-readable
/// message when something goes wrong, so the UI can surface it.
pub fn register(web_port: u16, ingest_port: u16) -> io::Result<()> {
    let path = services_json_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "OBS services.json not found - is OBS installed for the current user?",
        )
    })?;
    let original = fs::read_to_string(&path)?;
    looks_like_services_json(&original)?;
    // If our entry is already there, strip it first so a port-changed
    // re-register actually refreshes the URL. remove_entry's parse is
    // forgiving - falling back to the raw file keeps register() a
    // no-fail path when the entry isn't structurally cleanly bounded.
    let base = if entry_exists(&original) {
        remove_entry(&original).unwrap_or_else(|| original.clone())
    } else {
        original.clone()
    };
    let patched = insert_entry(&base, &entry_json(web_port, ingest_port)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't locate the `\"services\":[` array in services.json - file shape unexpected",
        )
    })?;
    let bak = path.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&path, &patched)?;
    Ok(())
}

/// Remove our entry from services.json. Returns Ok(()) even when not
/// registered (idempotent) - only surfaces an error if the file is
/// present but in an unexpected shape we can't safely edit.
pub fn unregister() -> io::Result<()> {
    let path = services_json_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "OBS services.json not found - nothing to unregister",
        )
    })?;
    let original = fs::read_to_string(&path)?;
    if !entry_exists(&original) {
        return Ok(()); // nothing to do
    }
    looks_like_services_json(&original)?;
    let stripped = remove_entry(&original).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't unambiguously remove the InstantClone entry - restore from .bak manually",
        )
    })?;
    let bak = path.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&path, &stripped)?;
    Ok(())
}

/// Reject a services.json that doesn't at least look like the expected
/// shape - a JSON object containing a `"services"` array. Spares the
/// user a half-corrupted file when something else (a botched manual
/// edit, a different program writing to the same path) trashed it
/// since OBS last touched it.
fn looks_like_services_json(file: &str) -> io::Result<()> {
    let trimmed = file.trim_start();
    if !trimmed.starts_with('{') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "services.json doesn't start with a JSON object - file may be corrupted, close OBS and run it once to regenerate it.",
        ));
    }
    if !file.contains("\"services\"") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "services.json is missing the `services` array - file looks unfamiliar, restore from .instantclone.bak or let OBS regenerate it.",
        ));
    }
    Ok(())
}

/// Wrap `fs::write` with a friendlier error when Windows reports the
/// file is locked by another process - usually OBS holding it open.
fn write_or_friendly(path: &std::path::Path, contents: &str) -> io::Result<()> {
    match fs::write(path, contents) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Windows: ERROR_ACCESS_DENIED (5) and ERROR_SHARING_VIOLATION
            // (32) are the two ways OBS-holding-the-file shows up.
            // PermissionDenied catches the unix-style mapping of those.
            let locked = matches!(e.raw_os_error(), Some(5) | Some(32))
                || e.kind() == io::ErrorKind::PermissionDenied;
            if locked {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "services.json is locked - close OBS Studio first, then try again.",
                ))
            } else {
                Err(e)
            }
        }
    }
}

/// Splice `entry` into the `"services":[ … ]` array as the first
/// element. Returns `None` if the array marker can't be found -
/// services.json is OBS-managed so this should always succeed for an
/// install we can trust, but we hand back `None` rather than corrupt
/// the file if anything looks off.
fn insert_entry(file: &str, entry: &str) -> Option<String> {
    // Find the array opening bracket after the `"services"` key. OBS
    // formats with two-space indent and no leading whitespace at the
    // bracket, but we tolerate optional whitespace just in case.
    let key = "\"services\"";
    let key_pos = file.find(key)?;
    let after_key = &file[key_pos + key.len()..];
    // Skip whitespace + colon + whitespace until the `[`.
    let bracket_offset = after_key.find('[')?;
    let absolute_bracket = key_pos + key.len() + bracket_offset;
    let (head, tail) = file.split_at(absolute_bracket + 1);
    // Insert our entry followed by a comma so the existing first
    // service stays valid JSON.
    Some(format!("{head}\n{entry},{tail}"))
}

/// Walk the file from the InstantClone name marker outward to find the
/// `{ … }` span that encloses our entry, then delete that span plus
/// any preceding/trailing comma so the surrounding array stays valid.
fn remove_entry(file: &str) -> Option<String> {
    // Locate the marker (handles both indented and non-indented
    // formats - the exact-substring check `entry_exists` already
    // confirmed at least one is present, so the unwraps below are
    // safe in practice but we still use `?` for paranoia).
    let marker = if file.contains(r#""name": "InstantClone""#) {
        r#""name": "InstantClone""#
    } else {
        r#""name":"InstantClone""#
    };
    let marker_pos = file.find(marker)?;
    // Walk backwards to find the `{` that opens our object.
    let mut start = marker_pos;
    let bytes = file.as_bytes();
    while start > 0 && bytes[start] != b'{' {
        start -= 1;
    }
    if bytes[start] != b'{' {
        return None;
    }
    // Walk forward from start to find the matching `}` - tracks
    // brace depth and ignores braces inside strings.
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut end = start;
    for (i, b) in bytes[start..].iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == start {
        return None;
    }
    // Also strip the trailing comma + whitespace if our entry isn't
    // the last item, or the leading comma if it is.
    let mut left = start;
    let mut right = end;
    // Skip whitespace right of `end` looking for a `,`.
    let mut probe = end;
    while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
        probe += 1;
    }
    if probe < bytes.len() && bytes[probe] == b',' {
        right = probe + 1;
    } else {
        // No trailing comma - try eating a leading one instead so the
        // remaining array doesn't end with a stray `,]`.
        let mut probe = start;
        while probe > 0 && bytes[probe - 1].is_ascii_whitespace() {
            probe -= 1;
        }
        if probe > 0 && bytes[probe - 1] == b',' {
            left = probe - 1;
        }
    }
    Some(format!("{}{}", &file[..left], &file[right..]))
}

// ── VOD-audio integration: OBS global.ini flag ──────────────────────
//
// OBS gates the VOD Track checkbox behind `EnableCustomServerVodTrack`
// in `[General]` of global.ini when the active service id is
// `rtmp_custom`. We write/remove this flag in lockstep with the
// per-destination `vod_audio` toggle so the streamer doesn't have to
// hand-edit an INI file. The flag is global to the user's OBS - it
// affects every Custom RTMP setup they have, not just InstantClone.
// That's deliberate on OBS's side: they only want power users who
// understand the implication enabling this, which is exactly what
// our dashboard toggle's copy spells out.

fn global_ini_path() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|p| PathBuf::from(p).join("obs-studio/global.ini"))
        .filter(|p| p.exists())
}

/// Read `[General] EnableCustomServerVodTrack` from OBS's global.ini.
/// Returns `false` when OBS isn't installed, the file is missing, or
/// the key isn't set. We treat any value other than literal `true` /
/// `1` as off, matching OBS's `config_get_bool` parser.
pub fn vod_audio_flag_set() -> bool {
    let Some(p) = global_ini_path() else {
        return false;
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return false;
    };
    ini_get(&s, "General", "EnableCustomServerVodTrack")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false)
}

/// Set or clear `[General] EnableCustomServerVodTrack=true` in OBS's
/// global.ini. Idempotent. Returns `Ok(false)` if OBS isn't installed
/// (the file path doesn't exist) so callers can degrade gracefully
/// instead of treating "no OBS" as a hard error.
pub fn set_vod_audio_flag(enable: bool) -> io::Result<bool> {
    let Some(p) = global_ini_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    let updated = ini_set(&original, "General", "EnableCustomServerVodTrack", enable);
    if updated == original {
        return Ok(true); // already in desired state
    }
    let bak = p.with_extension("ini.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&p, &updated)?;
    Ok(true)
}

/// Tiny INI reader / writer. OBS's global.ini is a strict
/// "[Section]" + "key=value" file with no comments / continuations -
/// no need to pull a full INI crate for two helpers.
fn ini_get<'a>(file: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut cur: Option<&str> = None;
    for line in file.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            cur = Some(rest);
            continue;
        }
        if cur != Some(section) {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim());
            }
        }
    }
    None
}

fn ini_set(file: &str, section: &str, key: &str, enable: bool) -> String {
    // Strategy: walk lines, copy through, replace the key inside its
    // section, or insert at end of section / end of file if missing.
    let want_value = if enable { "true" } else { "false" };
    let mut out = String::with_capacity(file.len() + 64);
    let mut cur: Option<String> = None;
    let mut in_target_section = false;
    let mut wrote_key = false;
    let mut section_end_idx: Option<usize> = None;

    // First pass: walk and either rewrite the existing key or remember
    // where the target section ends (so we can insert if absent).
    for line in file.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // Leaving a section without finding our key - remember the
            // line we're ABOUT to add (the new section header) as the
            // place to splice the key in if needed.
            if in_target_section && !wrote_key {
                section_end_idx = Some(out.len());
            }
            in_target_section = rest == section;
            cur = Some(rest.to_string());
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_target_section && !wrote_key {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    if enable {
                        out.push_str(key);
                        out.push('=');
                        out.push_str(want_value);
                        out.push('\n');
                    }
                    // If disabling, drop the line entirely.
                    wrote_key = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    // End-of-file inside the target section without finding the key.
    if in_target_section && !wrote_key && enable {
        out.push_str(key);
        out.push('=');
        out.push_str(want_value);
        out.push('\n');
        wrote_key = true;
    }
    // Target section ended before EOF without finding the key:
    // splice the line in at the boundary.
    if !wrote_key && enable {
        if let Some(idx) = section_end_idx {
            let line = format!("{}={}\n", key, want_value);
            out.insert_str(idx, &line);
            wrote_key = true;
        }
    }
    // Section doesn't exist at all - append it with our key.
    if !wrote_key && enable && cur.as_deref() != Some(section) {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("[{}]\n{}={}\n", section, key, want_value));
    }
    // `cur` is set only for triggering the !cur match on absent section
    let _ = cur;
    out
}

// ── VOD+EB experimental: per-profile service.json injection ─────────
//
// When the streamer enables the experimental "Also enable Enhanced
// Broadcasting" sub-toggle, we inject
// `"multitrack_video_configuration_url": "<our endpoint>"` into the
// active OBS profile's service.json under `settings`. OBS's
// rtmp-custom plugin only writes `server` / `key` / `use_auth` /
// `username` / `password` to settings, but obs_data_t preserves
// arbitrary keys round-trip. So the injected URL persists until we
// remove it, and OBS auto-fetches our multitrack config from there.
//
// We touch only the ACTIVE profile (not every profile we find). This
// is the user-chosen scope from the dashboard: minimal footprint, the
// user is warned that switching OBS profiles disables the injection.

const PROFILES_DIR_REL: &str = "obs-studio/basic/profiles";

/// Active OBS profile name, read from `%APPDATA%\obs-studio\global.ini`
/// (`[Basic] Profile=<name>`). None when OBS isn't installed or the
/// key isn't present.
pub fn active_profile() -> Option<String> {
    let p = global_ini_path()?;
    let s = fs::read_to_string(&p).ok()?;
    ini_get(&s, "Basic", "Profile").map(|v| v.to_string())
}

/// Path to the active profile's service.json, or None if OBS isn't
/// installed / no profile is selected.
pub fn active_profile_service_json_path() -> Option<PathBuf> {
    let profile = active_profile()?;
    let base = std::env::var("APPDATA").ok().map(PathBuf::from)?;
    let p = base
        .join(PROFILES_DIR_REL)
        .join(&profile)
        .join("service.json");
    p.exists().then_some(p)
}

/// True when our `multitrack_video_configuration_url` is currently
/// injected into the active profile's service.json. Used by the UI
/// status indicator so a manual edit / profile switch is reflected.
pub fn vod_eb_injection_present(web_port: u16) -> bool {
    let Some(p) = active_profile_service_json_path() else {
        return false;
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return false;
    };
    let needle = format!(
        "\"multitrack_video_configuration_url\":\"http://127.0.0.1:{}/obs/multitrack-config\"",
        web_port
    );
    let needle_spaced = format!(
        "\"multitrack_video_configuration_url\": \"http://127.0.0.1:{}/obs/multitrack-config\"",
        web_port
    );
    s.contains(&needle) || s.contains(&needle_spaced)
}

/// Inject the multitrack-video config URL into the active OBS profile's
/// rtmp_custom service.json. Idempotent. Returns Ok(true) when the
/// active profile exists AND uses rtmp_custom AND the write landed
/// (or was already present); Ok(false) when there's no active profile
/// or the profile uses a different service type (rtmp_common is one
/// where the URL would have no effect, since OBS gets it from
/// services.json).
pub fn inject_vod_eb(web_port: u16) -> io::Result<bool> {
    let Some(p) = active_profile_service_json_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    // Only inject into rtmp_custom — for rtmp_common services the
    // multitrack-video URL is read from services.json (not from
    // service.json), and injecting would be a no-op at best, foot-gun
    // at worst.
    if !original.contains("\"type\": \"rtmp_custom\"")
        && !original.contains("\"type\":\"rtmp_custom\"")
    {
        return Ok(false);
    }
    let needle = format!("http://127.0.0.1:{}/obs/multitrack-config", web_port);
    if original.contains(&needle) {
        return Ok(true); // already injected
    }
    let patched = inject_service_json_key(&original, web_port).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't find `settings` object in service.json - file shape unexpected",
        )
    })?;
    let bak = p.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&p, &patched)?;
    Ok(true)
}

/// Remove the injected URL from the active OBS profile's service.json.
/// Idempotent. Returns Ok(true) when the active profile exists and we
/// removed (or there was nothing to remove); Ok(false) when there's no
/// active profile.
pub fn revert_vod_eb(web_port: u16) -> io::Result<bool> {
    let Some(p) = active_profile_service_json_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    let needle = format!("http://127.0.0.1:{}/obs/multitrack-config", web_port);
    if !original.contains(&needle) {
        return Ok(true); // nothing to remove
    }
    let stripped = strip_service_json_key(&original, web_port);
    let bak = p.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&p, &stripped)?;
    Ok(true)
}

/// Splice `"multitrack_video_configuration_url": "<url>"` into the
/// JSON object's `settings` block. We locate `"settings": {` then
/// insert the key as the first child, comma-separated from whatever
/// comes next. Returns None if the marker isn't found - safer than
/// blindly editing a file we don't recognise.
fn inject_service_json_key(file: &str, web_port: u16) -> Option<String> {
    let key_pos = file.find("\"settings\"")?;
    let brace_offset = file[key_pos..].find('{')?;
    let absolute_brace = key_pos + brace_offset;
    let (head, tail) = file.split_at(absolute_brace + 1);
    let entry = format!(
        "\n        \"multitrack_video_configuration_url\": \"http://127.0.0.1:{}/obs/multitrack-config\",",
        web_port
    );
    Some(format!("{head}{entry}{tail}"))
}

/// Remove the multitrack-video-configuration-url key (matching our
/// local URL) from the settings object. We match the FULL key+value
/// pair plus surrounding whitespace + a trailing comma if present.
fn strip_service_json_key(file: &str, web_port: u16) -> String {
    let needle = format!("http://127.0.0.1:{}/obs/multitrack-config", web_port);
    let Some(needle_pos) = file.find(&needle) else {
        return file.to_string();
    };
    // Walk backwards from the URL to find the start of the line
    // containing the key. Walk forwards to find the comma (or `}`).
    let bytes = file.as_bytes();
    let mut start = needle_pos;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = needle_pos + needle.len();
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'\n' {
        end += 1;
    }
    // Strip the line entirely. The JSON stays valid because the next
    // sibling line (or `}`) is still well-formed.
    format!("{}{}", &file[..start], &file[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_services_json() -> String {
        // Minimal but realistic shape. OBS's actual file has many more
        // services; we only need the structural skeleton for our edits.
        r#"{
  "format_version": 4,
  "services": [
    {
      "name": "Twitch",
      "common": true
    },
    {
      "name": "YouTube - RTMPS",
      "common": true
    }
  ]
}
"#
        .to_string()
    }

    #[test]
    fn insert_then_detect_then_remove_roundtrips() {
        let original = fake_services_json();
        let entry = entry_json(7799, 1935);
        let patched = insert_entry(&original, &entry).expect("insert must succeed");
        assert!(entry_exists(&patched));
        let stripped = remove_entry(&patched).expect("remove must succeed");
        assert!(!entry_exists(&stripped));
        // Whitespace differences are expected (we insert with a
        // newline, remove just strips the entry + a comma) but the
        // services array should still contain both original entries.
        assert!(stripped.contains("Twitch"));
        assert!(stripped.contains("YouTube"));
    }

    #[test]
    fn insert_is_idempotent_for_caller() {
        // Caller (`register`) checks `entry_exists` before splicing,
        // so the second call to `register` is the cheap no-op path
        // even though `insert_entry` would happily double-insert.
        let original = fake_services_json();
        let entry = entry_json(7799, 1935);
        let once = insert_entry(&original, &entry).unwrap();
        assert!(entry_exists(&once));
        // `register` would short-circuit here, but verify the helper
        // doesn't crash on a second insert call and produces a file
        // with exactly two `"name": "InstantClone"` markers (other
        // "InstantClone" mentions in URLs and labels don't count).
        let twice = insert_entry(&once, &entry).unwrap();
        assert_eq!(twice.matches(r#""name": "InstantClone""#).count(), 2);
    }

    #[test]
    fn remove_handles_single_entry_array() {
        // After our entry has been the only thing in the services
        // array, removing it should leave a syntactically valid (if
        // empty) array, not a stray comma.
        let single = r#"{
  "format_version": 4,
  "services": [
    {
      "name": "InstantClone",
      "common": true
    }
  ]
}
"#;
        let out = remove_entry(single).expect("remove must succeed");
        assert!(!entry_exists(&out));
        // Either `[]` or `[\n  ]` is fine; just no `,]` artifact.
        assert!(!out.contains(",]"));
        assert!(!out.contains(", ]"));
    }

    #[test]
    fn entry_exists_matches_both_indent_styles() {
        let indented = r#"  "name": "InstantClone","#;
        let compact = r#""name":"InstantClone","#;
        assert!(entry_exists(indented));
        assert!(entry_exists(compact));
        assert!(!entry_exists(r#""name": "InstantCloneAndCompany""#));
    }

    #[test]
    fn looks_like_services_json_rejects_garbage() {
        // A file that doesn't even start with `{` is almost certainly
        // not OBS's services.json - refuse to splice into it so we
        // don't compound the corruption.
        assert!(looks_like_services_json("garbage not even json").is_err());
        // Valid JSON object but no services array - could be some
        // other config file at the wrong path; bail out.
        assert!(looks_like_services_json(r#"{"foo":"bar"}"#).is_err());
        // The real shape passes.
        assert!(looks_like_services_json(&fake_services_json()).is_ok());
    }

    #[test]
    fn insert_into_garbage_file_is_refused_at_validate_step() {
        // Confirms the validate step in register() catches garbage
        // before insert_entry would happily splice into nonsense.
        // We can't drive register() in unit tests (it touches the
        // filesystem) but the validator + insert chain is what
        // protects us.
        let bad = "definitely not services.json";
        assert!(looks_like_services_json(bad).is_err());
    }

    #[test]
    fn re_register_refreshes_changed_port() {
        // The flow we exercise here: register at port A, then "register"
        // at port B with the same entry already present. We can't call
        // `register()` directly (it touches disk), but the helpers it
        // uses - remove_entry + insert_entry - must compose into a file
        // that contains the *new* port.
        let original = fake_services_json();
        let v1 = insert_entry(&original, &entry_json(7799, 1935)).unwrap();
        assert!(v1.contains(":7799/"));
        // Re-register with a new port: strip the old entry, splice fresh.
        let cleaned = remove_entry(&v1).expect("strip must succeed");
        let v2 = insert_entry(&cleaned, &entry_json(8800, 1935)).unwrap();
        assert!(v2.contains(":8800/"), "must reflect new port");
        assert!(!v2.contains(":7799/"), "old port must be gone");
        assert_eq!(
            v2.matches(r#""name": "InstantClone""#).count(),
            1,
            "exactly one InstantClone entry, not two"
        );
    }

    // ── INI read/write ───────────────────────────────────────────────

    #[test]
    fn ini_get_reads_value_from_correct_section() {
        let ini =
            "[Basic]\nProfile=Untitled\n[General]\nName=Foo\nEnableCustomServerVodTrack=true\n";
        assert_eq!(
            ini_get(ini, "General", "EnableCustomServerVodTrack"),
            Some("true")
        );
        assert_eq!(ini_get(ini, "Basic", "Profile"), Some("Untitled"));
        assert_eq!(ini_get(ini, "General", "Missing"), None);
        // Key exists in a different section - must not match.
        assert_eq!(ini_get(ini, "Basic", "EnableCustomServerVodTrack"), None);
    }

    #[test]
    fn ini_set_inserts_when_section_exists_but_key_missing() {
        let ini = "[General]\nName=Foo\n[Basic]\nProfile=A\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
        assert!(out.contains("EnableCustomServerVodTrack=true"));
        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("true")
        );
        // Existing [Basic] block is preserved.
        assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
    }

    #[test]
    fn ini_set_replaces_existing_key_value() {
        let ini = "[General]\nEnableCustomServerVodTrack=false\nName=Foo\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("true")
        );
        assert!(!out.contains("EnableCustomServerVodTrack=false"));
        // Sibling key not disturbed.
        assert_eq!(ini_get(&out, "General", "Name"), Some("Foo"));
    }

    #[test]
    fn ini_set_removes_key_when_disabling() {
        let ini = "[General]\nEnableCustomServerVodTrack=true\nName=Foo\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);
        assert_eq!(ini_get(&out, "General", "EnableCustomServerVodTrack"), None);
        assert_eq!(ini_get(&out, "General", "Name"), Some("Foo"));
    }

    #[test]
    fn ini_set_creates_section_when_absent() {
        let ini = "[Basic]\nProfile=A\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("true")
        );
        assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
    }

    // ── service.json injection ─────────────────────────────────────

    fn fake_rtmp_custom_service_json() -> String {
        // Minimal but realistic shape OBS writes for a Custom RTMP service.
        r#"{
    "settings": {
        "key": "main",
        "server": "rtmp://127.0.0.1:1935/live",
        "use_auth": false
    },
    "type": "rtmp_custom"
}
"#
        .to_string()
    }

    #[test]
    fn inject_service_json_key_adds_url_into_settings() {
        let original = fake_rtmp_custom_service_json();
        let patched = inject_service_json_key(&original, 7799).expect("inject must succeed");
        assert!(patched.contains("multitrack_video_configuration_url"));
        assert!(patched.contains("http://127.0.0.1:7799/obs/multitrack-config"));
        // Existing settings are still present.
        assert!(patched.contains("\"server\": \"rtmp://127.0.0.1:1935/live\""));
        assert!(patched.contains("\"type\": \"rtmp_custom\""));
    }

    #[test]
    fn strip_service_json_key_removes_only_our_line() {
        let original = fake_rtmp_custom_service_json();
        let patched = inject_service_json_key(&original, 7799).unwrap();
        let stripped = strip_service_json_key(&patched, 7799);
        assert!(!stripped.contains("multitrack_video_configuration_url"));
        // Sibling settings stay untouched.
        assert!(stripped.contains("\"server\": \"rtmp://127.0.0.1:1935/live\""));
        assert!(stripped.contains("\"key\": \"main\""));
    }

    #[test]
    fn strip_service_json_key_is_noop_when_not_present() {
        let original = fake_rtmp_custom_service_json();
        let stripped = strip_service_json_key(&original, 7799);
        assert_eq!(stripped, original);
    }
}
