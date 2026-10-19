//! Keeps winget's record of a portable install in step with the exe.
//!
//! `winget install` of the portable build copies the exe into
//! `...\WinGet\Packages\<id>__<source>\` and writes an uninstall entry of the
//! same name under HKCU. That entry holds the version `winget list` shows
//! and the SHA-256 of the exe winget installed. The one-click update swaps
//! the exe in place, so without this winget keeps offering the version that
//! is already running, and refuses to uninstall a file it no longer
//! recognises ("has been modified"). Checked at every startup rather than
//! after the swap, so it holds whichever version did the swap, and for an
//! exe copied in by hand too. Copies winget didn't install are left alone.

use std::path::Path;

/// The package name on winget, for the commands we suggest to the user.
pub const PACKAGE_ID: &str = "Soulhackzlol.InstantClone";

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// Make winget's entry for `exe` say it holds `version`, whose file hash is
/// `sha256`, when winget installed it. Best effort: the app runs the same
/// either way, so a failure is logged and nothing else.
pub fn record(exe: &Path, version: &str, sha256: &str) {
    match record_under(UNINSTALL_KEY, exe, version, sha256) {
        Ok(true) => log_line!("[winget] recorded this install as v{version}"),
        Ok(false) => {}
        Err(e) => log_line!("[winget] couldn't update winget's record of this install: {e}"),
    }
}

/// Whether `exe` sits in a folder winget installed it into, for either the
/// current user or (in Program Files) every user.
pub fn is_winget_install(exe: &Path) -> bool {
    entry_name(exe).is_some()
}

/// `record` under any HKCU parent key, so tests can use a private one.
/// Whether it changed anything.
fn record_under(parent: &str, exe: &Path, version: &str, sha256: &str) -> std::io::Result<bool> {
    let Some(entry) = entry_name(exe) else {
        return Ok(false);
    };
    let key = format!(r"{parent}\{entry}");
    // Only an entry that names this exact file is ours to change. Checked
    // before writing, because writing a value creates a missing key.
    let target = crate::autostart::read_value(&key, "TargetFullPath");
    let names_this_exe =
        target.is_some_and(|target| Path::new(&target).as_os_str().eq_ignore_ascii_case(exe));
    if !names_this_exe {
        return Ok(false);
    }
    let sha256 = sha256.to_ascii_lowercase();
    let read = |name| crate::autostart::read_value(&key, name);
    // Every startup lands here, so an entry that is already right is left
    // unwritten.
    if read("DisplayVersion").as_deref() == Some(version) && read("SHA256") == Some(sha256.clone())
    {
        return Ok(false);
    }
    crate::autostart::write_value(&key, "DisplayVersion", version)?;
    crate::autostart::write_value(&key, "SHA256", &sha256)?;
    Ok(true)
}

/// The uninstall entry winget names after the folder it puts a portable
/// package in (`Publisher.App__DefaultSource`), or None when `exe` isn't in
/// a `WinGet\Packages` folder.
fn entry_name(exe: &Path) -> Option<&str> {
    let folder = exe.parent()?;
    let packages = folder.parent()?;
    let in_winget_packages = packages.file_name()?.eq_ignore_ascii_case("Packages")
        && packages
            .parent()?
            .file_name()?
            .eq_ignore_ascii_case("WinGet");
    if !in_winget_packages {
        return None;
    }
    folder.file_name()?.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const USER_EXE: &str = r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\Soulhackzlol.InstantClone__DefaultSource\instantclone.exe";
    const MACHINE_EXE: &str = r"C:\Program Files\WinGet\Packages\Soulhackzlol.InstantClone__DefaultSource\instantclone.exe";

    #[test]
    fn a_winget_folder_names_its_uninstall_entry() {
        let expected = Some("Soulhackzlol.InstantClone__DefaultSource");
        assert_eq!(entry_name(Path::new(USER_EXE)), expected);
        assert_eq!(entry_name(Path::new(MACHINE_EXE)), expected);
    }

    #[test]
    fn a_copy_outside_winget_has_no_entry() {
        assert_eq!(
            entry_name(Path::new(r"C:\Users\me\Downloads\instantclone.exe")),
            None
        );
        assert_eq!(
            entry_name(Path::new(r"C:\Tools\Packages\App\instantclone.exe")),
            None
        );
        assert_eq!(entry_name(Path::new(r"C:\instantclone.exe")), None);
    }

    /// A private parent key per test process, deleted on drop so a failed
    /// assertion still cleans up. Never the real Uninstall key.
    struct TestKey(String);

    impl TestKey {
        fn new(name: &str) -> Self {
            Self(format!(
                r"Software\InstantCloneWingetTest{name}{}",
                std::process::id()
            ))
        }

        fn entry(&self) -> String {
            format!(r"{}\Soulhackzlol.InstantClone__DefaultSource", self.0)
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
            let subkey: Vec<u16> = self.0.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, subkey.as_ptr()) };
        }
    }

    /// What winget writes at install time, trimmed to the values we touch.
    fn fake_winget_entry(key: &TestKey, target: &str) {
        let entry = key.entry();
        crate::autostart::write_value(&entry, "TargetFullPath", target).unwrap();
        crate::autostart::write_value(&entry, "DisplayVersion", "0.1.17").unwrap();
        crate::autostart::write_value(&entry, "SHA256", "aa").unwrap();
    }

    #[test]
    fn an_update_rewrites_the_version_and_hash_winget_checks() {
        let key = TestKey::new("Update");
        fake_winget_entry(&key, USER_EXE);

        let changed = record_under(&key.0, Path::new(USER_EXE), "0.1.18", "BEEF").unwrap();

        let entry = key.entry();
        let version = crate::autostart::read_value(&entry, "DisplayVersion");
        let hash = crate::autostart::read_value(&entry, "SHA256");
        assert!(changed);
        assert_eq!(version.as_deref(), Some("0.1.18"));
        assert_eq!(
            hash.as_deref(),
            Some("beef"),
            "winget stores the hash lowercase"
        );
    }

    /// Every startup checks, so an entry that already matches is left as is.
    #[test]
    fn an_entry_that_already_matches_is_not_rewritten() {
        let key = TestKey::new("Same");
        fake_winget_entry(&key, USER_EXE);

        let changed = record_under(&key.0, Path::new(USER_EXE), "0.1.17", "AA").unwrap();

        assert!(!changed, "same version and hash, in any case, is no change");
    }

    /// A rebuild ships under the same version number with a new hash, and
    /// winget checks the hash, so that alone must be recorded.
    #[test]
    fn a_new_build_of_the_same_version_is_recorded() {
        let key = TestKey::new("Rebuild");
        fake_winget_entry(&key, USER_EXE);

        let changed = record_under(&key.0, Path::new(USER_EXE), "0.1.17", "beef").unwrap();

        let hash = crate::autostart::read_value(&key.entry(), "SHA256");
        assert!(changed);
        assert_eq!(hash.as_deref(), Some("beef"));
    }

    /// Windows paths ignore case, and the registry may spell it differently.
    #[test]
    fn the_target_path_matches_regardless_of_case() {
        let key = TestKey::new("Case");
        fake_winget_entry(&key, &USER_EXE.to_ascii_uppercase());

        record_under(&key.0, Path::new(USER_EXE), "0.1.18", "beef").unwrap();

        let version = crate::autostart::read_value(&key.entry(), "DisplayVersion");
        assert_eq!(version.as_deref(), Some("0.1.18"));
    }

    /// An entry pointing at another copy's exe is that copy's business.
    #[test]
    fn an_entry_for_another_exe_is_left_alone() {
        let key = TestKey::new("Other");
        fake_winget_entry(&key, MACHINE_EXE);

        record_under(&key.0, Path::new(USER_EXE), "0.1.18", "beef").unwrap();

        let version = crate::autostart::read_value(&key.entry(), "DisplayVersion");
        assert_eq!(version.as_deref(), Some("0.1.17"));
    }

    /// Writing creates missing keys, so a missing entry must stay missing.
    #[test]
    fn no_entry_is_created_when_winget_has_none() {
        let key = TestKey::new("Missing");

        record_under(&key.0, Path::new(USER_EXE), "0.1.18", "beef").unwrap();

        let version = crate::autostart::read_value(&key.entry(), "DisplayVersion");
        assert_eq!(version, None);
    }

    #[test]
    fn a_copy_outside_winget_is_not_a_winget_install() {
        assert!(is_winget_install(&PathBuf::from(USER_EXE)));
        assert!(!is_winget_install(&PathBuf::from(
            r"D:\Stream\instantclone.exe"
        )));
    }
}
