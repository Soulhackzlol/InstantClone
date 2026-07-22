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

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set(_enabled: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "start with Windows is only available on Windows",
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
