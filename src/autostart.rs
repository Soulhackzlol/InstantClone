//! "Start with Windows" - one value under the per-user Run key.
//!
//! The registry is the source of truth here, deliberately: it is not
//! mirrored into `Settings`. Windows gives users their own switches for
//! startup entries (Task Manager's Startup tab, Settings → Apps → Startup),
//! and a copy of the flag on our side would sit there claiming "on" after
//! someone turned it off somewhere else. Reading the key costs one call, so
//! the dashboard just asks it directly and can never disagree with reality.
//!
//! HKCU rather than HKLM: no elevation needed, and autostart is a per-user
//! preference. The command is registered with `--no-browser` so logging in
//! doesn't throw a dashboard tab in the user's face.

use std::io;

/// Value name under the Run key. Stable - renaming it would strand the old
/// entry and silently start InstantClone twice.
#[cfg(windows)]
const VALUE_NAME: &str = "InstantClone";

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// UTF-16, null-terminated - every `*W` registry API expects this.
#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The exact command line the Run key should hold. Quoted because
/// `C:\Program Files\…` and friends contain spaces, and an unquoted path
/// would have Windows try to launch `C:\Program`.
#[cfg(windows)]
fn run_command() -> io::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" --no-browser", exe.display()))
}

/// Whether InstantClone is currently registered to start at login.
///
/// Any failure reads as "not enabled": the value being absent is itself
/// the common failure, and a missing Run key on an exotic Windows install
/// means the same thing to the user either way.
#[cfg(windows)]
pub fn is_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let subkey = wide(RUN_KEY);
    let name = wide(VALUE_NAME);
    // Null output buffer + null size: asks only "does this value exist,
    // and is it a string?" without reading the data back.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    rc == 0
}

/// Add or remove the Run-key entry.
///
/// Enabling always rewrites the command rather than checking first, so an
/// entry left behind by a previous install location is corrected instead
/// of pointing at an executable that no longer exists.
#[cfg(windows)]
pub fn set(enabled: bool) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };
    let subkey = wide(RUN_KEY);
    let name = wide(VALUE_NAME);

    let rc = if enabled {
        let value = wide(&run_command()?);
        // Length is in BYTES and must include the terminating null.
        let bytes = (value.len() * std::mem::size_of::<u16>()) as u32;
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr() as *const std::ffi::c_void,
                bytes,
            )
        }
    } else {
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
        // ERROR_FILE_NOT_FOUND - already absent, which is the state the
        // caller asked for. Disabling twice is not an error.
        const ERROR_FILE_NOT_FOUND: u32 = 2;
        if rc == ERROR_FILE_NOT_FOUND {
            0
        } else {
            rc
        }
    };

    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc as i32))
    }
}

// ── Linux: a freedesktop autostart entry ────────────────────────────
//
// The XDG spec's equivalent of the Run key is a `.desktop` file dropped in
// `$XDG_CONFIG_HOME/autostart` (default `~/.config/autostart`). Its presence
// is the source of truth, mirroring the Windows design: no copy in Settings,
// so the desktop environment's own "Startup Applications" toggle can never
// disagree with us. Launched with `--no-browser` for the same reason.

/// The autostart directory for the given env inputs. Pure (no env reads) so
/// the XDG-over-HOME precedence is unit-testable. `xdg` wins only when it is
/// an absolute path, matching the XDG base-dir spec.
#[cfg(target_os = "linux")]
fn autostart_dir_from(
    xdg: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    xdg.filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|base| base.join("autostart"))
}

/// Path to our autostart `.desktop` file, or None when no home dir resolves.
#[cfg(target_os = "linux")]
fn desktop_entry_path() -> Option<std::path::PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    autostart_dir_from(xdg, home).map(|d| d.join("instantclone.desktop"))
}

/// Brand icon as SVG (vector, so no image codec and it stays crisp at any
/// size). Matches the Windows tray glyph: a cyan rounded square with two white
/// pause bars, where "pause" stands for the delay.
#[cfg(target_os = "linux")]
const ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="48" height="48"><rect x="3" y="3" width="42" height="42" rx="10" fill="#5ac8fa"/><rect x="17" y="14" width="5" height="20" rx="1.5" fill="#f4f6f9"/><rect x="26" y="14" width="5" height="20" rx="1.5" fill="#f4f6f9"/></svg>"##;

/// Write the brand icon into the user's icon dir and return its path, so a
/// `.desktop` entry can point `Icon=` at a real file instead of a generic
/// launcher glyph. Best effort: None when no home resolves or the write fails
/// (the entry then just omits Icon=). Idempotent, and only rewrites on change.
#[cfg(target_os = "linux")]
pub(crate) fn ensure_icon() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("icons");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("instantclone.svg");
    let fresh = std::fs::read_to_string(&path)
        .map(|c| c == ICON_SVG)
        .unwrap_or(false);
    if !fresh {
        std::fs::write(&path, ICON_SVG).ok()?;
    }
    Some(path)
}

/// The `.desktop` file body. Pure so the Exec-quoting (an install path with
/// spaces must still launch), the `--no-browser` flag, and the optional
/// `Icon=` line are unit-testable without touching disk.
#[cfg(target_os = "linux")]
fn desktop_entry(exe: &std::path::Path, icon: Option<&str>) -> String {
    let icon_line = icon.map(|i| format!("Icon={i}\n")).unwrap_or_default();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=InstantClone\n\
         Comment=RTMP delay proxy\n\
         {icon_line}\
         Exec=\"{}\" --no-browser\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    )
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    desktop_entry_path().map(|p| p.exists()).unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn set(enabled: bool) -> io::Result<()> {
    let path = desktop_entry_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no HOME / XDG_CONFIG_HOME to place the autostart entry",
        )
    })?;
    if enabled {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let exe = std::env::current_exe()?;
        let icon = ensure_icon();
        let icon_ref = icon.as_ref().and_then(|p| p.to_str());
        std::fs::write(&path, desktop_entry(&exe, icon_ref))
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Already absent is the state the caller asked for, not an error.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::{autostart_dir_from, desktop_entry};
    use std::path::{Path, PathBuf};

    #[test]
    fn xdg_config_home_wins_when_absolute() {
        let dir = autostart_dir_from(Some(PathBuf::from("/xdg")), Some(PathBuf::from("/home/u")));
        assert_eq!(dir, Some(PathBuf::from("/xdg/autostart")));
    }

    #[test]
    fn falls_back_to_home_config_when_xdg_missing_or_relative() {
        assert_eq!(
            autostart_dir_from(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
        // A relative XDG value is ignored per the base-dir spec.
        assert_eq!(
            autostart_dir_from(
                Some(PathBuf::from("rel/ative")),
                Some(PathBuf::from("/home/u"))
            ),
            Some(PathBuf::from("/home/u/.config/autostart"))
        );
    }

    #[test]
    fn none_when_no_home() {
        assert_eq!(autostart_dir_from(None, None), None);
    }

    #[test]
    fn entry_quotes_exec_and_stays_headless() {
        let body = desktop_entry(Path::new("/opt/My Apps/instantclone"), None);
        assert!(body.contains("Exec=\"/opt/My Apps/instantclone\" --no-browser"));
        assert!(body.contains("Type=Application"));
        // No icon supplied -> no Icon= line at all.
        assert!(!body.contains("Icon="));
    }

    #[test]
    fn entry_includes_icon_when_supplied() {
        let body = desktop_entry(
            Path::new("/usr/bin/instantclone"),
            Some("/x/instantclone.svg"),
        );
        assert!(body.contains("Icon=/x/instantclone.svg\n"));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn set(_enabled: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "start-at-login is not supported on this platform",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn run_command_is_quoted_and_headless() {
        let cmd = run_command().expect("current_exe");
        assert!(
            cmd.starts_with('"'),
            "path must be quoted or a spaced install directory breaks the entry: {cmd}"
        );
        assert!(
            cmd.contains("\" --no-browser"),
            "autostart must not pop a browser tab at login: {cmd}"
        );
    }

    /// Round-trip against the real registry. Writes under HKCU, which
    /// needs no elevation, and restores whatever state the machine was in
    /// so running the suite never changes the developer's own startup.
    #[test]
    fn enable_disable_round_trip() {
        let original = is_enabled();

        set(true).expect("enable");
        assert!(is_enabled(), "value must exist after enabling");

        set(false).expect("disable");
        assert!(!is_enabled(), "value must be gone after disabling");

        // Disabling an already-absent value is a no-op, not an error.
        set(false).expect("disable twice");

        if original {
            set(true).expect("restore");
        }
        assert_eq!(
            is_enabled(),
            original,
            "test must leave startup state as found"
        );
    }
}
