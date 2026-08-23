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
//! Config-dir resolution (services.json + logs both live under it):
//!   * Windows: `%APPDATA%\obs-studio`, then `%LOCALAPPDATA%\obs-studio`.
//!   * Linux: `$XDG_CONFIG_HOME/obs-studio` (native / deb), then the Flatpak
//!     and Snap locations.
//!   * macOS: not handled yet (no candidates).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Candidate OBS config directories for this platform, in priority order.
/// Both `services.json` and the log folder are resolved relative to these,
/// so one list drives every OBS-path lookup.
fn obs_config_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        ["APPDATA", "LOCALAPPDATA"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .map(|base| PathBuf::from(base).join("obs-studio"))
            .collect()
    }
    #[cfg(target_os = "linux")]
    {
        obs_config_dirs_from(
            std::env::var_os("HOME").map(PathBuf::from),
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        )
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Pure builder for the Linux OBS config-dir candidates - no env reads, so the
/// XDG / Flatpak / Snap layout is unit-testable. Empty when no home resolves.
#[cfg(target_os = "linux")]
fn obs_config_dirs_from(home: Option<PathBuf>, xdg: Option<PathBuf>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let xdg = xdg
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    vec![
        xdg.join("obs-studio"), // native / deb / ppa
        home.join(".var/app/com.obsproject.Studio/config/obs-studio"), // flatpak
        home.join("snap/obs-studio/current/.config/obs-studio"), // snap
    ]
}

/// Locate the user's `services.json`. Returns `None` when no candidate path
/// exists - typical when OBS isn't installed at all.
pub fn services_json_path() -> Option<PathBuf> {
    obs_config_dirs()
        .into_iter()
        .map(|d| d.join("plugin_config/rtmp-services/services.json"))
        .find(|p| p.exists())
}

/// True when services.json holds an InstantClone entry that matches the
/// ports and URLs we would write today. Used by the System tab to render
/// the button as "Unregister" instead of "Register".
///
/// Deliberately stricter than "an entry with our name exists". An entry
/// written by an older build, or before the user retuned a port, still
/// carries our name while pointing OBS somewhere useless, and reporting
/// that as registered leaves the user with no way to notice or fix it.
/// Reporting it as unregistered puts the Register button back, and
/// `register` is remove-and-replace, so one click repairs it.
pub fn is_registered(web_port: u16, ingest_port: u16) -> bool {
    let Some(p) = services_json_path() else {
        return false;
    };
    let Ok(file) = fs::read_to_string(&p) else {
        return false;
    };
    entry_exists(&file) && registration_is_current(&file, web_port, ingest_port)
}

/// Whether the entry in `file` still matches what `entry_json` produces.
///
/// Checks the two URLs that decide whether OBS can reach us, the RTMP
/// server and the multi-track config endpoint. Both carry a port, so a
/// retuned port shows up here too.
///
/// Leftover literal servers inside OUR entry also count as stale: they
/// would sit in the dropdown as options that fail under one IP Family
/// setting each.
///
/// Every check runs against `entry_span` and not the whole file. Scanning
/// the file would let anything InstantClone-shaped elsewhere in
/// services.json hold the answer at "stale" permanently, and since
/// re-registering only rewrites our own entry, the user would be stuck
/// with a Register button that never turns into Unregister no matter how
/// many times they press it.
fn registration_is_current(file: &str, web_port: u16, ingest_port: u16) -> bool {
    let Some(span) = entry_span(file) else {
        return false;
    };
    let entry = &file[span];
    let stale_literals = [
        format!("rtmp://127.0.0.1:{ingest_port}/live"),
        format!("rtmp://[::1]:{ingest_port}/live"),
    ];
    entry.contains(&server_url(ingest_port))
        && entry.contains(&multitrack_url(web_port))
        && !stale_literals.iter().any(|u| entry.contains(u.as_str()))
}

/// The RTMP URL we advertise to OBS.
///
/// `localhost`, not a literal, and that is the whole point. OBS's
/// Settings -> Advanced -> Network -> IP Family is a resolver setting: it
/// picks the address family passed to `getaddrinfo`. Under IP Family =
/// IPv6 an IPv4 literal cannot resolve (`getaddrinfo("127.0.0.1",
/// AF_INET6)` returns host-not-found), and under IPv4 an IPv6 literal
/// cannot either. `localhost` is the one spelling that resolves under all
/// three settings, so a single entry covers every configuration and the
/// user never has to know the setting exists.
///
/// This is safe only because the ingest binds both families (see
/// `Settings::ingest_addrs`). Under IPv4+IPv6 OBS resolves both `::1` and
/// `127.0.0.1` and races them; happy-eyeballs reports failure only when
/// every candidate fails, so even if our IPv6 leg could not bind, the
/// IPv4 leg still wins the race.
fn server_url(ingest_port: u16) -> String {
    format!("rtmp://localhost:{ingest_port}/live")
}

fn multitrack_url(web_port: u16) -> String {
    format!("http://127.0.0.1:{web_port}/obs/multitrack-config")
}

/// Build the services.json entry for InstantClone. The
/// `multitrack_video_configuration_url` is what OBS hits when the user
/// enables multi-track video - we serve it from our own web port.
///
/// `"name"` MUST stay the first key: `remove_entry` walks backwards from
/// the name marker to the nearest `{` without tracking strings, so it only
/// finds our object's opening brace while nothing precedes the name.
///
/// One server entry, on `localhost`. See `server_url` for why that
/// spelling is the only one that works under every OBS IP Family setting,
/// and why a single entry beats offering the user a choice.
fn entry_json(web_port: u16, ingest_port: u16) -> String {
    format!(
        r#"{{
            "name": "InstantClone",
            "common": true,
            "more_info_link": "https://github.com/Soulhackzlol/InstantClone",
            "stream_key_link": "http://127.0.0.1:{web}/",
            "multitrack_video_configuration_url": "{mtv}",
            "multitrack_video_name": "Multi-track via InstantClone",
            "multitrack_video_learn_more_link": "https://github.com/Soulhackzlol/InstantClone",
            "servers": [
                {{
                    "name": "InstantClone (local proxy)",
                    "url": "{server}"
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
        mtv = multitrack_url(web_port),
        server = server_url(ingest_port),
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
fn write_or_friendly(path: &std::path::Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
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
                    "the OBS config file is locked - close OBS Studio first, then try again.",
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

/// Byte range of our service object in `file`, braces included.
///
/// Scoping matters: the rest of the file belongs to OBS and to other
/// services, and a stray `InstantClone`-flavoured string elsewhere (a
/// leftover server object inside another service's `servers` array, say)
/// must not be read as ours. Anything that inspects "our entry" has to go
/// through here rather than searching the whole file.
fn entry_span(file: &str) -> Option<std::ops::Range<usize>> {
    // Locate the marker (handles both indented and non-indented
    // formats). The trailing quote is what keeps this from matching a
    // nested `"name": "InstantClone (local proxy)"` server object.
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
    Some(start..end)
}

/// Delete our entry plus any preceding/trailing comma so the surrounding
/// array stays valid.
fn remove_entry(file: &str) -> Option<String> {
    let span = entry_span(file)?;
    let (start, end) = (span.start, span.end);
    let bytes = file.as_bytes();
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

// ── VOD-audio integration: OBS user.ini flag ────────────────────────
//
// OBS gates the VOD Track checkbox behind `EnableCustomServerVodTrack`
// in `[General]` of the user config. The frontend reads it as:
//
//   bool enableForCustomServer = config_get_bool(
//       App()->GetUserConfig(), "General", "EnableCustomServerVodTrack");
//   bool enableVodTrack = ui->service->currentText() == "Twitch";
//   if (enableForCustomServer && IsCustomService())
//       enableVodTrack = true;
//
// (frontend/settings/OBSBasicSettings_Stream.cpp on master)
//
// IMPORTANT: OBS 32 split the legacy global.ini into two files -
// `global.ini` (app-level) and `user.ini` (user-level) - and
// `GetUserConfig()` now returns user.ini. v0.1.0..0.1.2 wrote the
// flag to global.ini, which OBS 32 simply ignored. Issue #9 was the
// streamer report ("the bypass didn't work") that surfaced this.
//
// Strategy: prefer user.ini when it exists (OBS 32+), fall back to
// global.ini for older installs. On every write we also strip the
// legacy key from global.ini if it's there, so upgraders don't carry
// dead state across files.
//
// We write/remove this flag in lockstep with the per-destination
// `vod_audio` toggle so the streamer doesn't have to hand-edit an INI
// file. The flag is global to the user's OBS - it affects every
// Custom RTMP setup they have, not just InstantClone. That's
// deliberate on OBS's side: they only want power users who understand
// the implication enabling this, which is exactly what our dashboard
// toggle's copy spells out.

/// Pure path-resolution helper: given an OBS install directory
/// (typically `%APPDATA%\obs-studio`), pick the authoritative user
/// config file - user.ini on OBS 32+, falling back to global.ini on
/// older installs that haven't been split. Separated from
/// `obs_user_config_path` so tests can exercise the prefer-user.ini
/// logic with a temp directory instead of mutating real %APPDATA%.
fn resolve_user_config_in(obs_dir: &Path) -> Option<PathBuf> {
    let user = obs_dir.join("user.ini");
    if user.exists() {
        return Some(user);
    }
    let global = obs_dir.join("global.ini");
    global.exists().then_some(global)
}

/// Pure helper for the cleanup-only path that resolves the legacy
/// global.ini under an OBS install directory, but ONLY when user.ini
/// is present alongside (i.e. OBS 32+, where global.ini's
/// `EnableCustomServerVodTrack` is dead state to strip). Returns None
/// on older installs where global.ini is still the live config.
fn legacy_global_ini_in(obs_dir: &Path) -> Option<PathBuf> {
    let user = obs_dir.join("user.ini");
    if !user.exists() {
        return None;
    }
    let global = obs_dir.join("global.ini");
    global.exists().then_some(global)
}

/// Authoritative user-config path: user.ini in OBS 32+, falling back
/// to global.ini for pre-32 installs that haven't been split yet.
/// `[Basic] Profile=` lives in this file too, so the active-profile
/// detection below shares the same lookup.
fn obs_user_config_path() -> Option<PathBuf> {
    // Walk the same cross-platform candidate dirs as services.json (Windows
    // APPDATA/LOCALAPPDATA, Linux XDG/Flatpak/Snap). The old %APPDATA%-only
    // lookup returned None on Linux, silently no-op'ing the VOD-audio flag.
    obs_config_dirs()
        .into_iter()
        .find_map(|obs_dir| resolve_user_config_in(&obs_dir))
}

/// Legacy global.ini path - only consulted for the post-upgrade
/// cleanup pass that strips dead `EnableCustomServerVodTrack` entries
/// left behind by v0.1.0..0.1.2. None on pre-32 installs where
/// `obs_user_config_path` already points at global.ini.
fn legacy_global_ini_path_for_cleanup() -> Option<PathBuf> {
    obs_config_dirs()
        .into_iter()
        .find_map(|obs_dir| legacy_global_ini_in(&obs_dir))
}

/// Read `[General] EnableCustomServerVodTrack` from OBS's user config.
/// Returns `false` when OBS isn't installed, the file is missing, or
/// the key isn't set. We treat any value other than literal `true` /
/// `1` as off, matching OBS's `config_get_bool` parser.
pub fn vod_audio_flag_set() -> bool {
    let Some(p) = obs_user_config_path() else {
        return false;
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return false;
    };
    ini_get(&s, "General", "EnableCustomServerVodTrack")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false)
}

/// Set `[General] EnableCustomServerVodTrack=<true|false>` in OBS's
/// user config. Idempotent. Returns `Ok(false)` if OBS isn't installed
/// (the file path doesn't exist) so callers can degrade gracefully
/// instead of treating "no OBS" as a hard error.
///
/// Disabling rewrites the existing line to `=false` rather than
/// deleting it. OBS's `config_get_bool` reads `=false` and "key
/// absent" identically, but the in-place rewrite is robust against
/// OBS rewriting user.ini on shutdown and moving the key between
/// duplicate [General] blocks - wherever it lands, the next toggle
/// just flips the value. The downside (one extra line on a never-
/// enabled install) is gated away: insertion only happens on enable.
///
/// On OBS 32+ we also flip the same key to false in the now-orphan
/// global.ini, so users upgrading from v0.1.0..0.1.2 (which mistakenly
/// wrote there) end up with no live flag anywhere.
pub fn set_vod_audio_flag(enable: bool) -> io::Result<bool> {
    let Some(p) = obs_user_config_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    let updated = ini_set(&original, "General", "EnableCustomServerVodTrack", enable);
    if updated != original {
        let bak = p.with_extension("ini.instantclone.bak");
        write_or_friendly(&bak, &original)?;
        write_or_friendly(&p, &updated)?;
    }
    // Best-effort legacy cleanup: silently strip the key from
    // global.ini if v0.1.0..0.1.2 ever wrote it there. Failures are
    // non-fatal - the user.ini write above is the load-bearing one,
    // global.ini being unwritable just means the dead key lingers as
    // harmless noise until the user closes OBS.
    if let Some(legacy) = legacy_global_ini_path_for_cleanup() {
        if let Ok(legacy_original) = fs::read_to_string(&legacy) {
            let cleaned = ini_set(
                &legacy_original,
                "General",
                "EnableCustomServerVodTrack",
                false,
            );
            if cleaned != legacy_original {
                let bak = legacy.with_extension("ini.instantclone.bak");
                let _ = write_or_friendly(&bak, &legacy_original);
                let _ = write_or_friendly(&legacy, &cleaned);
            }
        }
    }
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
    // Strategy: walk lines, copy through, ALWAYS rewrite the key's
    // value when we encounter it (true OR false - never drop), or
    // insert at end of section / end of file if missing AND we're
    // enabling. Robust against OBS rewriting user.ini and moving the
    // key around between duplicate [General] blocks - we don't depend
    // on "where" the key lives, just that it lives in the target
    // section *somewhere*. Disabling is just `=false`, which OBS's
    // `config_get_bool` reads identically to "key absent" - so we
    // never need to delete a line we can't reliably find.
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
                    out.push_str(key);
                    out.push('=');
                    out.push_str(want_value);
                    out.push('\n');
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
// `username` / `password` to settings, and discards every other key
// in its `rtmp_custom_update` function. That means the injected URL
// is dropped on LOAD (never reaches OBS's MultitrackVideoAutoConfigURL
// lookup), not only on SAVE - so the injection mechanism is dead.
// We keep `inject_vod_eb` / `revert_vod_eb` only for cleanup of stale
// state left behind by v0.1.0..0.1.2. New Phase C is the
// `launch_obs_with_eb_config` helper below: spawn obs64.exe with the
// `--config-url` CLI flag, which is the only path OBS's frontend
// actually honours for Custom RTMP services.
//
// We touch only the ACTIVE profile (not every profile we find). This
// is the user-chosen scope from the dashboard: minimal footprint, the
// user is warned that switching OBS profiles disables the injection.

// Relative to an OBS config dir (the `.../obs-studio` paths from obs_config_dirs).
const PROFILES_DIR_REL: &str = "basic/profiles";

/// Active OBS profile name, read from `[Basic] Profile=<name>` in
/// OBS's user config (user.ini on 32+, global.ini on older installs).
/// None when OBS isn't installed or the key isn't present.
pub fn active_profile() -> Option<String> {
    let p = obs_user_config_path()?;
    let s = fs::read_to_string(&p).ok()?;
    ini_get(&s, "Basic", "Profile").map(|v| v.to_string())
}

/// Path to the active profile's service.json, or None if OBS isn't
/// installed / no profile is selected.
pub fn active_profile_service_json_path() -> Option<PathBuf> {
    let profile = active_profile()?;
    obs_config_dirs()
        .into_iter()
        .map(|obs_dir| {
            obs_dir
                .join(PROFILES_DIR_REL)
                .join(&profile)
                .join("service.json")
        })
        .find(|p| p.exists())
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

// ── Phase C v2: launch OBS with --config-url ────────────────────────
//
// OBS's frontend looks for the multi-track config URL in this order
// (frontend/utility/GoLiveAPI_Network.cpp::MultitrackVideoAutoConfigURL):
//
//   1. `--config-url <url>` command-line argument
//   2. `multitrack_video_configuration_url` key in the active service's
//      settings (services.json for rtmp_common, service.json for
//      anything else).
//
// For rtmp_custom services, path 2 is dead - the plugin's
// `rtmp_custom_update` only extracts 5 known keys from the settings
// object and discards the rest, so OBS never sees our injection. The
// CLI flag is the only durable way to enable Enhanced Broadcasting
// alongside VOD audio (which itself requires Custom RTMP, since
// rtmp_common services gate the VOD track checkbox on the service name
// being literally "Twitch"). That's why this is a "Launch OBS" button
// instead of an auto-injected file edit.

/// Standard Windows install paths for obs64.exe. The first match wins.
/// Portable / non-standard installs aren't auto-discovered (rare and
/// the user can always launch OBS themselves with the flag).
#[cfg(windows)]
fn obs_executable_candidates() -> [PathBuf; 2] {
    [
        PathBuf::from("C:\\Program Files\\obs-studio\\bin\\64bit\\obs64.exe"),
        PathBuf::from("C:\\Program Files (x86)\\obs-studio\\bin\\64bit\\obs64.exe"),
    ]
}

#[cfg(windows)]
pub fn find_obs_executable() -> Option<PathBuf> {
    obs_executable_candidates().into_iter().find(|p| p.exists())
}

/// Native `obs` binary from the standard prefixes, then a PATH scan. Flatpak
/// / Snap installs expose no plain binary path (they launch via `flatpak run`
/// / the snap wrapper), so those users launch OBS themselves.
#[cfg(target_os = "linux")]
pub fn find_obs_executable() -> Option<PathBuf> {
    for p in ["/usr/bin/obs", "/usr/local/bin/obs", "/bin/obs"] {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("obs"))
        .find(|p| p.exists())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn find_obs_executable() -> Option<PathBuf> {
    None
}

// ── OBS version + optional VOD-unlocker script install ──────────────

/// Pull the first `<maj>.<min>.<patch>` triple out of a string. Used to
/// read OBS's version from its log header line
/// (`... OBS 32.2.1 (64-bit, windows)`). Ignores 2-part numbers.
fn first_version_triple(s: &str) -> Option<(u32, u32, u32)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let parts: Vec<&str> = s[start..i].split('.').collect();
            if parts.len() >= 3 {
                if let (Ok(a), Ok(b), Ok(c)) =
                    (parts[0].parse(), parts[1].parse(), parts[2].parse())
                {
                    return Some((a, b, c));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Best-effort OBS version, parsed from the newest log file's header line
/// (`... OBS 32.2.1 (64-bit, windows)`). `None` when OBS has never run or
/// its logs can't be read. Callers treat `None` as "unknown - allow with
/// a note", not a hard block: this only gates the experimental
/// VOD-unlocker script.
pub fn obs_version() -> Option<(u32, u32, u32)> {
    let logs_dir = obs_config_dirs()
        .into_iter()
        .map(|d| d.join("logs"))
        .find(|p| p.is_dir())?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&logs_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }
    let contents = fs::read_to_string(newest?.1).ok()?;
    contents
        .lines()
        .take(30)
        .filter(|l| l.contains("OBS"))
        .find_map(first_version_triple)
}

/// Whether an `obs64.exe` process is currently running. Used by the setup
/// wizard to show the "close OBS first" note only when it actually applies:
/// registering edits `services.json`, which a running OBS overwrites on exit,
/// so the warning is noise if OBS is already shut. Any enumeration failure
/// reads as "not running" - the note just stays hidden, which is harmless.
#[cfg(windows)]
pub fn is_obs_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut running = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("obs64.exe")
                {
                    running = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        running
    }
}

/// Whether an `obs` process is currently running, by scanning `/proc/*/comm`.
/// Any read failure reads as "not running" - the setup note just stays hidden,
/// which is harmless.
#[cfg(target_os = "linux")]
pub fn is_obs_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        // Only numeric PID dirs carry a comm file worth reading.
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            if comm.trim() == "obs" {
                return true;
            }
        }
    }
    false
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn is_obs_running() -> bool {
    false
}

/// Spawn OBS Studio with the `--config-url` flag pointing at InstantClone's
/// multitrack-config endpoint, so Enhanced Broadcasting comes up pre-wired.
///
/// Returns Ok when the spawn syscall succeeded (NOT when OBS finishes loading,
/// which is async). Errors carry an actionable "OBS not found" message so the
/// dashboard can tell the user what to do.
#[cfg(windows)]
pub fn launch_obs_with_eb_config(web_port: u16, dock_token: &str) -> io::Result<PathBuf> {
    let exe = find_obs_executable().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "obs64.exe not found in the standard install locations. Launch OBS \
             manually with the --config-url <endpoint> flag.",
        )
    })?;
    let config_url = eb_config_url(web_port, dock_token);
    // Working dir = OBS's own folder: its logging and locale paths resolve
    // relative to bin/64bit.
    let workdir = exe.parent().unwrap_or(&exe);
    use std::os::windows::process::CommandExt;
    std::process::Command::new(&exe)
        .args(["--config-url", &config_url])
        .current_dir(workdir)
        // CREATE_NEW_PROCESS_GROUP: OBS gets its own console group so
        // InstantClone closing doesn't propagate a break to it.
        .creation_flags(0x0000_0200)
        .spawn()?;
    Ok(exe)
}

/// Linux: try a native `obs` binary, then the Flatpak app, then the Snap.
/// Ubuntu's default OBS is commonly a Flatpak or Snap with no plain binary on
/// PATH, so covering those is what makes the launch button actually work
/// instead of failing for most desktop users.
#[cfg(target_os = "linux")]
pub fn launch_obs_with_eb_config(web_port: u16, dock_token: &str) -> io::Result<PathBuf> {
    let config_url = eb_config_url(web_port, dock_token);

    // 1) Native binary (deb / ppa / most distros).
    if let Some(exe) = find_obs_executable() {
        let workdir = exe.parent().unwrap_or(&exe);
        std::process::Command::new(&exe)
            .args(["--config-url", &config_url])
            .current_dir(workdir)
            .spawn()?;
        return Ok(exe);
    }

    // 2) Flatpak: present when its per-app data dir exists; launched via
    //    `flatpak run`, which forwards our flag to OBS.
    if flatpak_obs_present() && command_on_path("flatpak") {
        std::process::Command::new("flatpak")
            .args(["run", "com.obsproject.Studio", "--config-url", &config_url])
            .spawn()?;
        return Ok(PathBuf::from("flatpak run com.obsproject.Studio"));
    }

    // 3) Snap.
    if snap_obs_present() && command_on_path("snap") {
        std::process::Command::new("snap")
            .args(["run", "obs-studio", "--config-url", &config_url])
            .spawn()?;
        return Ok(PathBuf::from("snap run obs-studio"));
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "OBS not found. Looked for a native obs binary, the Flatpak \
         com.obsproject.Studio, and the obs-studio snap. Launch OBS manually \
         with the --config-url <endpoint> flag.",
    ))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn launch_obs_with_eb_config(_web_port: u16, _dock_token: &str) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "launching OBS is not supported on this platform",
    ))
}

/// The `--config-url` OBS should hit for Enhanced Broadcasting. When the
/// dashboard password is on, the dock token rides in the query so OBS (which
/// cannot sign in) still passes the auth gate to the control-plane multitrack
/// endpoint; empty token = the plain URL for the default (no-auth) install.
#[cfg(any(windows, target_os = "linux"))]
fn eb_config_url(web_port: u16, dock_token: &str) -> String {
    if dock_token.is_empty() {
        format!("http://127.0.0.1:{web_port}/obs/multitrack-config")
    } else {
        format!("http://127.0.0.1:{web_port}/obs/multitrack-config?token={dock_token}")
    }
}

/// A `~/.var/app/com.obsproject.Studio` dir means the Flatpak OBS is installed.
#[cfg(target_os = "linux")]
fn flatpak_obs_present() -> bool {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(".var/app/com.obsproject.Studio")
                .exists()
        })
        .unwrap_or(false)
}

/// A `~/snap/obs-studio` dir means the Snap OBS is installed.
#[cfg(target_os = "linux")]
fn snap_obs_present() -> bool {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("snap/obs-studio").exists())
        .unwrap_or(false)
}

/// True if `name` resolves to a file on PATH.
#[cfg(target_os = "linux")]
fn command_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).exists()))
        .unwrap_or(false)
}

// ── Desktop shortcut: one-click "Start InstantClone + OBS (VOD + EB)" ──
//
// Generates a clickable launcher on the Desktop that cold-starts the app
// with `--launch-eb`, so a streamer who always wants VOD audio + Enhanced
// Broadcasting gets the whole setup from a single double-click. On Windows
// we prefer a real .lnk (so it carries the app icon), created via
// PowerShell's WScript.Shell COM to avoid a COM crate, falling back to a
// .cmd launcher. On Linux we write a freedesktop `.desktop` launcher.

/// Best-effort Desktop folder. `%USERPROFILE%\Desktop` / `$HOME/Desktop`
/// covers the vast majority of installs; localized or redirected desktops
/// are not auto-discovered. None when we can't resolve a home dir.
fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join("Desktop"))
}

/// PowerShell single-quoted-string escaping: a literal `'` becomes `''`.
#[cfg(windows)]
fn ps_single_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Try to create a real Windows `.lnk` shortcut via PowerShell COM.
/// Returns Ok only when PowerShell reports success AND the file exists.
#[cfg(windows)]
fn try_create_lnk(lnk: &Path, exe: &Path) -> io::Result<()> {
    let workdir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{lnk}');\
         $s.TargetPath='{exe}';$s.Arguments='--launch-eb';\
         $s.IconLocation='{exe}';$s.WorkingDirectory='{wd}';$s.Save()",
        lnk = ps_single_quote(&lnk.display().to_string()),
        exe = ps_single_quote(&exe.display().to_string()),
        wd = ps_single_quote(&workdir.display().to_string()),
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()?;
    if status.success() && lnk.exists() {
        Ok(())
    } else {
        Err(io::Error::other("PowerShell did not create the shortcut"))
    }
}

/// Create a Desktop shortcut that launches InstantClone in VOD+EB mode
/// (`instantclone --launch-eb`). Returns the path of the file actually
/// created (a `.lnk` when possible, otherwise a `.cmd` fallback) so the UI
/// can tell the user exactly what to look for.
#[cfg(windows)]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let desktop = desktop_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not locate your Desktop folder",
        )
    })?;
    // Make sure the directory exists (it always should, but a redirected
    // profile mid-setup could briefly not have it).
    if !desktop.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Desktop folder not found at {}", desktop.display()),
        ));
    }
    let lnk = desktop.join("InstantClone (VOD + EB).lnk");
    if try_create_lnk(&lnk, &exe).is_ok() {
        return Ok(lnk);
    }
    // Fallback: a plain .cmd launcher. `start "" "<exe>"` detaches so the
    // console window closes immediately after spawning the app.
    let cmd = desktop.join("InstantClone (VOD + EB).cmd");
    let body = format!(
        "@echo off\r\nstart \"\" \"{}\" --launch-eb\r\n",
        exe.display()
    );
    write_or_friendly(&cmd, &body)?;
    Ok(cmd)
}

/// Linux: write a freedesktop `.desktop` launcher on the Desktop. It must be
/// marked executable (GNOME also treats that as the "trusted" bit) to run on
/// double-click.
#[cfg(target_os = "linux")]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let desktop = desktop_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not locate your Desktop folder",
        )
    })?;
    if !desktop.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Desktop folder not found at {}", desktop.display()),
        ));
    }
    let file = desktop.join("InstantClone (VOD + EB).desktop");
    let icon = crate::autostart::ensure_icon();
    let icon_line = icon
        .as_ref()
        .and_then(|p| p.to_str())
        .map(|i| format!("Icon={i}\n"))
        .unwrap_or_default();
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=InstantClone (VOD + EB)\n\
         Comment=Start InstantClone in VOD + EB mode and launch OBS\n\
         {icon_line}\
         Exec=\"{}\" --launch-eb\n\
         Terminal=false\n",
        exe.display()
    );
    write_or_friendly(&file, &entry)?;
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&file)?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&file, perm)?;
    Ok(file)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "desktop shortcut creation is not supported on this platform",
    ))
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
    use std::sync::atomic::{AtomicU32 as TestUniq, Ordering as TestOrd};

    // OBS resolves the server URL with the address family picked in
    // Settings -> Advanced -> IP Family. An IPv4 literal is not resolvable
    // under IPv6, so the single entry we register has to carry a hostname.
    #[test]
    fn the_registered_server_url_is_resolvable_under_either_ip_family() {
        assert_eq!(server_url(1935), "rtmp://localhost:1935/live");
        let entry = entry_json(7799, 1935);
        assert!(entry.contains("rtmp://localhost:1935/live"), "{entry}");
        assert!(!entry.contains("rtmp://127.0.0.1"), "{entry}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_obs_config_dirs_cover_native_flatpak_snap() {
        use std::path::PathBuf;
        let dirs =
            obs_config_dirs_from(Some(PathBuf::from("/home/u")), Some(PathBuf::from("/xdg")));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/xdg/obs-studio"),
                PathBuf::from("/home/u/.var/app/com.obsproject.Studio/config/obs-studio"),
                PathBuf::from("/home/u/snap/obs-studio/current/.config/obs-studio"),
            ]
        );
        // No home -> no candidates (every OBS lookup then returns None).
        assert!(obs_config_dirs_from(None, None).is_empty());
        // A relative XDG value is ignored; falls back to ~/.config.
        let fb = obs_config_dirs_from(Some(PathBuf::from("/home/u")), Some(PathBuf::from("rel")));
        assert_eq!(fb[0], PathBuf::from("/home/u/.config/obs-studio"));
    }

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
    fn first_version_triple_reads_obs_log_header() {
        // The real OBS log header line.
        assert_eq!(
            first_version_triple("18:52:33.123: OBS 32.2.1 (64-bit, windows)"),
            Some((32, 2, 1))
        );
        assert_eq!(first_version_triple("OBS 30.0.0"), Some((30, 0, 0)));
        // Two-part numbers don't count; need major.minor.patch.
        assert_eq!(first_version_triple("version 1.2 only"), None);
        assert_eq!(first_version_triple("no digits here"), None);
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

    /// The single server entry must be the `localhost` spelling. An IP
    /// literal of either family is unreachable under one of OBS's three IP
    /// Family settings, because that setting picks the address family
    /// handed to `getaddrinfo`. Measured on Windows 11:
    /// `getaddrinfo("127.0.0.1", AF_INET6)` returns 11001 host-not-found,
    /// while `localhost` resolves under AF_INET, AF_INET6 and AF_UNSPEC.
    #[test]
    fn entry_advertises_localhost_so_every_ip_family_setting_resolves() {
        let entry = entry_json(7799, 1935);
        assert!(entry.contains(r#""url": "rtmp://localhost:1935/live""#));
        // A literal would strand one IP Family setting apiece.
        assert!(
            !entry.contains("rtmp://127.0.0.1:1935/live"),
            "no v4 literal"
        );
        assert!(!entry.contains("rtmp://[::1]:1935/live"), "no v6 literal");
        // One server, so the user never has to pick.
        assert_eq!(entry.matches(r#""url": "rtmp://"#).count(), 1);
    }

    /// An entry from an older build still carries our name while pointing
    /// OBS at an address it may not be able to resolve. Reporting that as
    /// registered would leave the user no way to notice, so staleness has
    /// to read as unregistered and put the Register button back.
    #[test]
    fn a_stale_entry_does_not_count_as_registered() {
        let current = entry_json(7799, 1935);
        assert!(registration_is_current(&current, 7799, 1935));

        // Pre-IPv6 builds wrote the v4 literal.
        let legacy = current.replace("rtmp://localhost:1935/live", "rtmp://127.0.0.1:1935/live");
        assert!(entry_exists(&legacy), "still ours by name");
        assert!(
            !registration_is_current(&legacy, 7799, 1935),
            "a v4-literal entry is stale and must prompt a re-register"
        );

        // A retuned port is stale for the same reason.
        assert!(!registration_is_current(&current, 7799, 1936));
        assert!(!registration_is_current(&current, 7800, 1935));

        // Our current URL present but literal servers left over beside it:
        // the dropdown would still offer options that fail under one IP
        // Family setting each, so this has to read as stale too.
        let with_leftovers = current.replace(
            r#""url": "rtmp://localhost:1935/live""#,
            r#""url": "rtmp://localhost:1935/live"}, {"name": "old", "url": "rtmp://[::1]:1935/live""#,
        );
        assert!(with_leftovers.contains("rtmp://localhost:1935/live"));
        assert!(
            !registration_is_current(&with_leftovers, 7799, 1935),
            "a superset of our entry is still out of date"
        );
    }

    /// Found on a real install: a leftover `InstantClone (local proxy)`
    /// server object sitting inside Twitch's `servers` array, unrelated to
    /// our own entry. A whole-file staleness scan sees that literal and
    /// reports stale forever, and because re-registering only rewrites our
    /// entry, the Register button can never turn into Unregister no matter
    /// how many times it is pressed. The check has to look at our entry
    /// only.
    #[test]
    fn a_stray_instantclone_server_in_another_service_does_not_block_registration() {
        let polluted = format!(
            r#"{{"format_version":4,"services":[
{},
{{"name":"Twitch","common":true,"servers":[
                {{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }},
                {{"name":"Asia: Hong Kong","url":"rtmp://hk.contribute.live-video.net/app"}}]}}
]}}"#,
            entry_json(7799, 1935)
        );
        assert!(
            polluted.contains("rtmp://127.0.0.1:1935/live"),
            "the stray must be present for this test to mean anything"
        );
        assert!(
            registration_is_current(&polluted, 7799, 1935),
            "a stray in someone else's entry must not make ours look stale"
        );
        assert!(is_entry_current_for_test(&polluted));
    }

    /// Removing our entry must leave the unrelated stray alone: it is not
    /// ours to delete, and clobbering another service's servers array
    /// would be far worse than the dead dropdown option it causes.
    #[test]
    fn remove_entry_leaves_a_stray_in_another_service_untouched() {
        let polluted = format!(
            r#"{{"format_version":4,"services":[
{},
{{"name":"Twitch","common":true,"servers":[
                {{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }}]}}
]}}"#,
            entry_json(7799, 1935)
        );
        let stripped = remove_entry(&polluted).expect("remove");
        assert!(!entry_exists(&stripped), "our entry is gone");
        assert!(
            stripped.contains(
                r#"{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }"#
            ),
            "Twitch's array must be untouched"
        );
        assert!(stripped.contains("Twitch"));
    }

    fn is_entry_current_for_test(file: &str) -> bool {
        entry_exists(file) && registration_is_current(file, 7799, 1935)
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
    fn ini_set_writes_false_in_place_when_disabling() {
        // Issue #9 follow-up: disabling REWRITES the value as `false`
        // instead of deleting the line. OBS's config_get_bool reads
        // `=false` and "key absent" identically, but rewriting the
        // line in place is robust against OBS shuffling the key into
        // a different [General] block on its own file rewrite -
        // wherever the key ended up, we just flip the value.
        let ini = "[General]\nEnableCustomServerVodTrack=true\nName=Foo\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);
        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("false")
        );
        assert!(!out.contains("EnableCustomServerVodTrack=true"));
        assert_eq!(ini_get(&out, "General", "Name"), Some("Foo"));
    }

    #[test]
    fn ini_set_does_not_create_false_line_when_disabling_a_virgin_file() {
        // A user who has never toggled VOD audio on shouldn't end up
        // with a stray `EnableCustomServerVodTrack=false` line in
        // their user.ini just because the dashboard reconcile ran on
        // a clean state. Insertion is still gated to the enable path.
        let ini = "[General]\nName=Foo\n[Basic]\nProfile=A\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);
        assert_eq!(out, ini, "no key found + disable should be a no-op");
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

    /// Reported on the v0.1.3 test build: user.ini ends up with two
    /// `[General]` sections (OBS itself maintains a second one at the
    /// end of the file). Our toggle-ON correctly lands the key in the
    /// trailing block; this test confirms toggle-OFF flips its value
    /// in place to `false` without disturbing either section's other
    /// keys or trying to delete the line - which would be fragile
    /// against OBS rewriting the file and moving the key around. The
    /// file shape mirrors what OBS 32 wrote after the first toggle +
    /// restart, trimmed down to the bits that exercise the
    /// duplicate-section walk.
    #[test]
    fn ini_set_flips_value_to_false_in_trailing_duplicate_general_section() {
        let ini = "[General]\n\
            Pre19Defaults=false\n\
            FirstRun=true\n\
            \n\
            [Basic]\n\
            Profile=Sin Título\n\
            \n\
            [Audio]\n\
            DisableAudioDucking=true\n\
            \n\
            [General]\n\
            EnableCustomServerVodTrack=true\n\
            FirstRun=true\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);

        // Value flipped, line stays.
        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("false")
        );
        assert!(out.contains("EnableCustomServerVodTrack=false"));
        assert!(!out.contains("EnableCustomServerVodTrack=true"));

        // Both [General] sections are still there - we never want to
        // collapse OBS's own structure, just edit our one key.
        assert_eq!(out.matches("[General]").count(), 2);

        // Sibling keys in BOTH [General] blocks survive.
        assert!(out.contains("Pre19Defaults=false"));
        assert!(out.contains("FirstRun=true"));

        // Unrelated sections are untouched.
        assert_eq!(ini_get(&out, "Basic", "Profile"), Some("Sin Título"));
        assert_eq!(ini_get(&out, "Audio", "DisableAudioDucking"), Some("true"));
    }

    /// Sibling property: toggle-ON on a fresh duplicate-section file
    /// (no key present in either [General]) lands the key in the
    /// trailing [General] - which is what we observed during the
    /// v0.1.3 test build. Documenting this so a future refactor that
    /// chooses to insert into the first block instead doesn't surprise
    /// the next maintainer.
    #[test]
    fn ini_set_lands_in_trailing_general_when_duplicate_sections_exist() {
        let ini = "[General]\n\
            FirstRun=true\n\
            \n\
            [Basic]\n\
            Profile=A\n\
            \n\
            [General]\n\
            ConfirmOnExit=true\n";
        let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);

        assert_eq!(
            ini_get(&out, "General", "EnableCustomServerVodTrack"),
            Some("true")
        );
        // Lands inside the trailing [General], after ConfirmOnExit.
        let trailing = out.rfind("[General]").expect("trailing [General] kept");
        assert!(out[trailing..].contains("EnableCustomServerVodTrack=true"));
        // Idempotent toggle-on - second call must not double-write.
        let twice = ini_set(&out, "General", "EnableCustomServerVodTrack", true);
        assert_eq!(twice.matches("EnableCustomServerVodTrack=true").count(), 1);
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

    /// Manually crafted legacy state from v0.1.0..0.1.2's broken
    /// inject path: an `rtmp_custom` service.json that was once
    /// patched by the old `inject_service_json_key` helper. Strip must
    /// remove our key without disturbing real OBS settings, so an
    /// upgrade from a polluted file ends up clean.
    fn legacy_polluted_rtmp_custom_service_json() -> String {
        r#"{
    "settings": {
        "multitrack_video_configuration_url": "http://127.0.0.1:7799/obs/multitrack-config",
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
    fn strip_service_json_key_removes_only_our_line() {
        let polluted = legacy_polluted_rtmp_custom_service_json();
        let stripped = strip_service_json_key(&polluted, 7799);
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

    // ── OBS user-config path resolution ─────────────────────────────
    //
    // Issue #9 (v0.1.2): the EnableCustomServerVodTrack flag was
    // being written to global.ini, but OBS 32 reads the VOD-track
    // gate from user.ini (via App()->GetUserConfig()). These tests
    // lock the prefer-user.ini behaviour so the file selection
    // can't silently regress back to global.ini in a future refactor.

    fn fresh_obs_dir(label: &str) -> std::path::PathBuf {
        // Each test gets its own dir so a tempdir cleanup race
        // between them can't make assertions flap.
        let dir = std::env::temp_dir().join(format!(
            "ic-obs-{}-{}-{}",
            label,
            std::process::id(),
            UNIQ_OBS.fetch_add(1, TestOrd::SeqCst),
        ));
        std::fs::create_dir_all(&dir).expect("fresh_obs_dir create_dir_all");
        dir
    }

    #[test]
    fn resolve_user_config_prefers_user_ini_when_both_files_exist() {
        let dir = fresh_obs_dir("prefer-user");
        let user = dir.join("user.ini");
        let global = dir.join("global.ini");
        std::fs::write(&user, "[General]\n").unwrap();
        std::fs::write(&global, "[General]\n").unwrap();

        let resolved = resolve_user_config_in(&dir).expect("must resolve");
        assert_eq!(
            resolved, user,
            "OBS 32+ keeps user.ini as the authoritative user config - \
             writing to global.ini is the v0.1.0..0.1.2 bug from issue #9"
        );
    }

    #[test]
    fn resolve_user_config_falls_back_to_global_ini_when_user_ini_missing() {
        let dir = fresh_obs_dir("fallback-global");
        let global = dir.join("global.ini");
        std::fs::write(&global, "[General]\n").unwrap();

        let resolved = resolve_user_config_in(&dir).expect("must resolve");
        assert_eq!(
            resolved, global,
            "pre-OBS-32 installs only have global.ini; the resolver \
             must still find the user config there"
        );
    }

    #[test]
    fn resolve_user_config_returns_none_when_neither_exists() {
        let dir = fresh_obs_dir("no-obs");
        assert!(
            resolve_user_config_in(&dir).is_none(),
            "no OBS install means no config to write to - caller \
             should degrade gracefully, not error"
        );
    }

    #[test]
    fn legacy_global_ini_only_reported_when_user_ini_present() {
        // Case 1: OBS 32+ with both files - legacy global.ini is
        // dead state we want to clean up.
        let dir = fresh_obs_dir("legacy-both");
        let user = dir.join("user.ini");
        let global = dir.join("global.ini");
        std::fs::write(&user, "[General]\n").unwrap();
        std::fs::write(&global, "[General]\n").unwrap();
        assert_eq!(legacy_global_ini_in(&dir), Some(global.clone()));

        // Case 2: pre-32 install - global.ini IS the live config,
        // not legacy. The cleanup path must skip so we don't
        // strip the flag from the file OBS is actively reading.
        let dir = fresh_obs_dir("legacy-no-user");
        let global = dir.join("global.ini");
        std::fs::write(&global, "[General]\n").unwrap();
        assert!(legacy_global_ini_in(&dir).is_none());
    }

    static UNIQ_OBS: TestUniq = TestUniq::new(0);
}
