//! OBS Studio services.json patcher.
//!
//! OBS knows about streaming services through a `services.json` file
//! in the user's `AppData` directory. Each entry declares the server
//! URLs, recommended encoder settings, and — critically for our
//! use-case — the `multitrack_video_configuration_url` that turns on
//! the Enhanced Broadcasting / multi-track-video UI inside OBS.
//!
//! By inserting an InstantClone entry into that file we let the
//! streamer pick us straight from the OBS service dropdown — no
//! Custom-server fiddling, no manual JSON paste, multi-track Video
//! "Auto" buttons work because they hit our local config endpoint.
//!
//! No `serde_json` dependency — the file is well-formed JSON produced
//! by OBS, so a careful string-level insert + an idempotent check by
//! exact-substring match handles all the cases we care about. Every
//! write goes via a `.bak` copy first; if anything goes sideways the
//! user can restore manually.
//!
//! Location resolution priority:
//!   1. `$APPDATA\obs-studio\plugin_config\rtmp-services\services.json`
//!      (the OBS-on-Windows standard path)
//!   2. `$LOCALAPPDATA\obs-studio\plugin_config\rtmp-services\services.json`
//!      (rare — some portable installs land here)
//!
//! macOS / Linux paths are not handled — the rest of the app is
//! Windows-only by design.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Locate the user's `services.json`. Returns `None` if neither
/// candidate path exists — typical when OBS isn't installed at all.
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
/// enables multi-track video — we serve it from our own web port.
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
            }}
        }}"#,
        web = web_port,
        rtmp = ingest_port,
    )
}

/// Check whether an "InstantClone" entry is present anywhere in
/// services.json. Exact-substring match on the name key — services.json
/// is OBS-generated and stable enough that this won't false-positive on
/// e.g. a "FooInstantCloneBar" service name (no such service exists in
/// the upstream services.json).
fn entry_exists(file: &str) -> bool {
    file.contains(r#""name": "InstantClone""#) || file.contains(r#""name":"InstantClone""#)
}

/// Register InstantClone with OBS. Idempotent — running twice doesn't
/// duplicate the entry. Returns an `io::Error` with a user-readable
/// message when something goes wrong, so the UI can surface it.
pub fn register(web_port: u16, ingest_port: u16) -> io::Result<()> {
    let path = services_json_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "OBS services.json not found — is OBS installed for the current user?",
        )
    })?;
    let original = fs::read_to_string(&path)?;
    if entry_exists(&original) {
        return Ok(()); // already registered, no-op
    }
    let patched = insert_entry(&original, &entry_json(web_port, ingest_port)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't locate the `\"services\":[` array in services.json — file shape unexpected",
        )
    })?;
    let bak = path.with_extension("json.instantclone.bak");
    fs::write(&bak, &original)?;
    fs::write(&path, patched)?;
    Ok(())
}

/// Remove our entry from services.json. Returns Ok(()) even when not
/// registered (idempotent) — only surfaces an error if the file is
/// present but in an unexpected shape we can't safely edit.
pub fn unregister() -> io::Result<()> {
    let path = services_json_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "OBS services.json not found — nothing to unregister",
        )
    })?;
    let original = fs::read_to_string(&path)?;
    if !entry_exists(&original) {
        return Ok(()); // nothing to do
    }
    let stripped = remove_entry(&original).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't unambiguously remove the InstantClone entry — restore from .bak manually",
        )
    })?;
    let bak = path.with_extension("json.instantclone.bak");
    fs::write(&bak, &original)?;
    fs::write(&path, stripped)?;
    Ok(())
}

/// Splice `entry` into the `"services":[ … ]` array as the first
/// element. Returns `None` if the array marker can't be found —
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
    // formats — the exact-substring check `entry_exists` already
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
    // Walk forward from start to find the matching `}` — tracks
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
        // No trailing comma — try eating a leading one instead so the
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
}
