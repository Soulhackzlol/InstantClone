//! "Start with Windows" - one value per copy under the per-user Run key.
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
/// entry and silently start InstantClone twice. It stays the first name a
/// copy takes (see `value_names`), so a single copy keeps the entry every
/// earlier build wrote.
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

/// The Run values a copy may use, in the order it takes them. Two copies
/// from two folders each need their own: with one shared name, switching
/// it on in the second copy repointed the first copy's entry, and
/// switching it off there deleted it. The second name is readable in
/// regedit ("InstantClone (Second channel)"); the third only breaks a tie
/// between two folders of the same name.
#[cfg(windows)]
fn value_names(exe: &std::path::Path) -> [String; 3] {
    let folder = exe
        .parent()
        .and_then(|dir| dir.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = exe.to_string_lossy().to_lowercase();
    let id = &crate::sha256::hex(path.as_bytes())[..8];
    [
        VALUE_NAME.to_string(),
        format!("{VALUE_NAME} ({folder})"),
        format!("{VALUE_NAME} ({folder} {id})"),
    ]
}

/// Who a Run value belongs to, judged by the exe its command starts.
#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
enum Owner {
    /// No such value.
    Nobody,
    /// It starts this exe.
    Us,
    /// It starts an exe that no longer exists: left by a copy that was moved
    /// or deleted, free to take over.
    Gone,
    /// It starts another copy that is still there. Never touched.
    OtherCopy,
}

/// The exe a Run command starts: the quoted path we write, or the first
/// word of a hand-written unquoted one.
#[cfg(windows)]
fn command_exe(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split_whitespace().next().unwrap_or(command),
    }
}

/// Pure apart from `exists`, so the rules are testable without a registry.
#[cfg(windows)]
fn owner_of(
    command: Option<&str>,
    exe: &std::path::Path,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Owner {
    let Some(command) = command else {
        return Owner::Nobody;
    };
    let target = std::path::Path::new(command_exe(command));
    // Windows paths compare without case.
    if target.to_string_lossy().to_lowercase() == exe.to_string_lossy().to_lowercase() {
        Owner::Us
    } else if exists(target) {
        Owner::OtherCopy
    } else {
        Owner::Gone
    }
}

/// A string value under an HKCU subkey, or None when it is missing.
#[cfg(windows)]
fn read_value(subkey: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let subkey = wide(subkey);
    let name = wide(name);
    // First ask the size, then read into a buffer of it.
    let mut bytes: u32 = 0;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if rc != 0 || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Each of this copy's candidate value names with its current owner.
#[cfg(windows)]
fn candidates(subkey: &str) -> io::Result<Vec<(String, Owner)>> {
    let exe = std::env::current_exe()?;
    Ok(value_names(&exe)
        .into_iter()
        .map(|name| {
            let owner = owner_of(read_value(subkey, &name).as_deref(), &exe, |p| p.exists());
            (name, owner)
        })
        .collect())
}

/// Whether this copy is registered to start at login. Another copy's entry
/// doesn't count: each dashboard shows its own switch.
///
/// Any failure reads as "not enabled": the value being absent is itself
/// the common failure, and a missing Run key on an exotic Windows install
/// means the same thing to the user either way.
#[cfg(windows)]
pub fn is_enabled() -> bool {
    is_enabled_under(RUN_KEY)
}

/// `is_enabled` against any HKCU subkey. Split out so the round-trip test
/// can use a private key instead of the developer's real Run key.
#[cfg(windows)]
fn is_enabled_under(subkey: &str) -> bool {
    candidates(subkey).is_ok_and(|names| names.iter().any(|(_, owner)| *owner == Owner::Us))
}

/// Add or remove this copy's Run entry.
///
/// Enabling always rewrites the command rather than checking first, so an
/// entry left behind by a previous install location is corrected instead
/// of pointing at an executable that no longer exists. Another copy's
/// entry is never rewritten or removed.
#[cfg(windows)]
pub fn set(enabled: bool) -> io::Result<()> {
    set_under(RUN_KEY, enabled)
}

/// `set` against any HKCU subkey. Split out so the round-trip test can use
/// a private key instead of the developer's real Run key.
#[cfg(windows)]
fn set_under(subkey: &str, enabled: bool) -> io::Result<()> {
    let names = candidates(subkey)?;
    if !enabled {
        for (name, _) in names.iter().filter(|(_, owner)| *owner == Owner::Us) {
            delete_value(subkey, name)?;
        }
        return Ok(());
    }
    // Ours if we have one, otherwise the first that is free.
    let name = names
        .iter()
        .find(|(_, owner)| *owner == Owner::Us)
        .or_else(|| {
            names
                .iter()
                .find(|(_, owner)| matches!(owner, Owner::Nobody | Owner::Gone))
        })
        .map(|(name, _)| name)
        .ok_or_else(|| io::Error::other("every startup entry name is taken by another copy"))?;
    write_value(subkey, name, &run_command()?)
}

#[cfg(windows)]
fn write_value(subkey: &str, name: &str, command: &str) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let subkey = wide(subkey);
    let name = wide(name);
    let value = wide(command);
    // Length is in BYTES and must include the terminating null.
    let bytes = (value.len() * std::mem::size_of::<u16>()) as u32;
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr() as *const std::ffi::c_void,
            bytes,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc as i32))
    }
}

#[cfg(windows)]
fn delete_value(subkey: &str, name: &str) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
    let subkey = wide(subkey);
    let name = wide(name);
    let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
    // ERROR_FILE_NOT_FOUND - already absent, which is the state the caller
    // asked for. Disabling twice is not an error.
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    if rc == 0 || rc == ERROR_FILE_NOT_FOUND {
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

    /// Private HKCU key the round-trip test owns outright. A single level so
    /// deleting it leaves nothing behind, and never the real Run key: a test
    /// that touched that could rewrite or drop the developer's own startup
    /// entry, and a panic midway would leave it that way.
    /// A private key per test process, so two runs at once never share one.
    fn test_key(name: &str) -> String {
        format!(
            r"Software\InstantCloneAutostartTest{name}{}",
            std::process::id()
        )
    }

    /// Deletes its key on drop, so a failed assertion still cleans up.
    struct TestKeyGuard(String);

    impl Drop for TestKeyGuard {
        fn drop(&mut self) {
            use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
            let subkey = wide(&self.0);
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, subkey.as_ptr()) };
        }
    }

    /// Round-trip through the same code path `set`/`is_enabled` use, but
    /// against a private key, so running the suite never changes the
    /// developer's own startup.
    #[test]
    fn enable_disable_round_trip() {
        let key = test_key("");
        let _cleanup = TestKeyGuard(key.clone());
        assert!(
            !is_enabled_under(&key),
            "stale test key from an earlier run"
        );

        set_under(&key, true).expect("enable");
        assert!(is_enabled_under(&key), "value must exist after enabling");

        set_under(&key, false).expect("disable");
        assert!(
            !is_enabled_under(&key),
            "value must be gone after disabling"
        );

        // Disabling an already-absent value is a no-op, not an error.
        set_under(&key, false).expect("disable twice");
    }

    /// A second copy (another folder) switching autostart on or off
    /// leaves the first copy's entry exactly as it was, and an entry left
    /// by a copy that no longer exists is taken over.
    #[test]
    fn each_copy_has_its_own_startup_entry() {
        let key = test_key("Copies");
        let _cleanup = TestKeyGuard(key.clone());
        let key = key.as_str();
        // A file that exists and isn't this exe stands in for the other copy.
        let other_exe =
            std::env::temp_dir().join(format!("ic-other-copy-{}.exe", std::process::id()));
        std::fs::write(&other_exe, b"").unwrap();
        let other = format!(r#""{}" --no-browser"#, other_exe.display());
        let other = other.as_str();
        write_value(key, VALUE_NAME, other).unwrap();
        assert!(!is_enabled_under(key), "the other copy's entry isn't ours");

        set_under(key, true).expect("enable");
        assert!(is_enabled_under(key));
        assert_eq!(
            read_value(key, VALUE_NAME).as_deref(),
            Some(other),
            "left alone"
        );

        set_under(key, false).expect("disable");
        assert!(!is_enabled_under(key));
        assert_eq!(
            read_value(key, VALUE_NAME).as_deref(),
            Some(other),
            "still there"
        );

        let gone = r#""C:\InstantCloneMovedAway\InstantClone.exe" --no-browser"#;
        write_value(key, VALUE_NAME, gone).unwrap();
        set_under(key, true).expect("enable over a gone copy");
        assert_eq!(read_value(key, VALUE_NAME), Some(run_command().unwrap()));
        let _ = std::fs::remove_file(&other_exe);
    }

    #[test]
    fn run_commands_are_read_for_their_exe_owner_and_value_name() {
        let exe = std::path::Path::new(r"C:\Streams\Main\InstantClone.exe");
        let ours = r#""c:\streams\main\instantclone.exe" --no-browser"#;
        assert_eq!(
            owner_of(Some(ours), exe, |_| true),
            Owner::Us,
            "case-insensitive"
        );
        let theirs = r#""C:\Streams\Second\InstantClone.exe" --no-browser"#;
        assert_eq!(owner_of(Some(theirs), exe, |_| true), Owner::OtherCopy);
        assert_eq!(owner_of(Some(theirs), exe, |_| false), Owner::Gone);
        assert_eq!(owner_of(None, exe, |_| true), Owner::Nobody);
        assert_eq!(
            command_exe(r"C:\Tools\ic.exe --no-browser"),
            r"C:\Tools\ic.exe"
        );
        let names = value_names(exe);
        assert_eq!(names[0], "InstantClone");
        assert_eq!(names[1], "InstantClone (Main)");
        assert!(names[2].starts_with("InstantClone (Main ") && names[2].len() == 28);
    }
}
