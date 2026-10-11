//! One running copy per setup.
//!
//! Two InstantClones side by side are fine as long as each has its own
//! folder: its own settings, ports and buffer. Two copies of the SAME setup
//! are never fine. They would fight over the ports, write the same buffer
//! file, and the second one's quit would delete the first one's buffer from
//! under a live stream. The usual way to get there is innocent: double-click
//! the exe while it already sits in the tray.
//!
//! The guard is an OS lock on a file beside the settings, taken before
//! anything else is read or written and held for the life of the process.
//! The OS drops it when the process ends, however it ends, so a crash or a
//! power cut never leaves a stale lock the way a PID file would.
//!
//! Copies side by side also need telling apart at a glance, so each running
//! copy shows its tray icon in its own colour (see `show_tray_color`).

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Holds the lock until dropped, which in practice is process exit.
pub struct InstanceLock {
    _file: File,
}

pub enum Acquired {
    /// This process now owns the setup.
    Locked(InstanceLock),
    /// Another running InstantClone owns it.
    HeldElsewhere,
    /// The lock file could not be opened (a read-only folder, say). Nothing
    /// is known either way; the caller carries on as before this existed.
    Unavailable(io::Error),
}

/// `<settings file>.lock`, beside the settings file it guards. Named after
/// the settings file rather than fixed, so two settings files in one folder
/// (picked with CONFIG_PATH) are two setups.
pub fn lock_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "instantclone.config.json".into());
    name.push(".lock");
    config_path.with_file_name(name)
}

/// Take the lock for the setup at `config_path`. `wait` is how long to keep
/// trying while another process holds it: zero for a normal launch, a few
/// seconds on a restart, where the outgoing process still holds it until it
/// has finished exiting.
pub fn acquire(config_path: &Path, wait: Duration) -> Acquired {
    let file = match open(&lock_path(config_path)) {
        Ok(file) => file,
        Err(e) => return Acquired::Unavailable(e),
    };
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Acquired::Locked(InstanceLock { _file: file }),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(TryLockError::WouldBlock) => return Acquired::HeldElsewhere,
            Err(TryLockError::Error(e)) => return Acquired::Unavailable(e),
        }
    }
}

/// Open (creating if needed) without truncating. The file is never deleted:
/// removing it while another process waits on it would let a third one lock
/// a fresh file beside the deleted one, and two copies would run.
fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    // Hidden on Windows: it is an implementation detail sitting next to the
    // exe the user double-clicks. Only applies when the file is created.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        options.attributes(FILE_ATTRIBUTE_HIDDEN);
    }
    options.open(path)
}

/// The colours the tray icon comes in, by name and RGB. The first, the brand
/// blue, is every setup's until another is picked in System settings.
pub const TRAY_COLORS: [(&str, [u8; 3]); 5] = [
    ("Blue", [0x5a, 0xc8, 0xfa]),
    ("Purple", [0xbf, 0x5a, 0xf2]),
    ("Orange", [0xff, 0x9f, 0x0a]),
    ("Green", [0x30, 0xd1, 0x58]),
    ("Pink", [0xff, 0x37, 0x5f]),
];

/// `#rrggbb` for a tray colour, for the dashboard's tab icon. An index out of
/// range reads as the brand colour.
pub fn tray_color_hex(index: u8) -> String {
    let (_, [r, g, b]) = TRAY_COLORS
        .get(index as usize)
        .copied()
        .unwrap_or(TRAY_COLORS[0]);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Something this copy holds until dropped, which in practice is process
/// exit: a tray colour, or a port it is about to listen on. On Windows a
/// named mutex, which the OS removes with the last handle, so a crash never
/// strands one. Elsewhere there is no tray, and the port check that follows
/// a claim is all there is.
pub struct SessionClaim {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the handle is only ever closed, and Windows lets any thread close
// a handle its process owns.
#[cfg(windows)]
unsafe impl Send for SessionClaim {}

#[cfg(windows)]
impl Drop for SessionClaim {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

/// The tray colour this copy shows, and its hold on it while it runs.
static TRAY_COLOR: crate::sync::Mutex<(u8, Option<SessionClaim>)> =
    crate::sync::Mutex::new((0, None));

/// Show the tray colour picked in System settings (`chosen`), and return the
/// colour shown. That is another one, for this run only, while a different
/// running copy already shows the chosen one: two icons side by side never
/// match, and the usual case is a folder copied with its settings. `wait`
/// covers a restart, where the outgoing process can hold its colour a moment
/// after it lets go of the setup lock.
pub fn show_tray_color(chosen: u8, wait: Duration) -> u8 {
    let mut shown = TRAY_COLOR.lock();
    // Let go first, so picking the colour already shown finds it free.
    shown.1 = None;
    #[cfg(windows)]
    {
        *shown = claim_color_in(r"Local\InstantClone.TrayColor", chosen, wait);
    }
    // No tray here: the colour is only the dashboard's tab icon.
    #[cfg(not(windows))]
    {
        let _ = wait;
        shown.0 = chosen;
    }
    shown.0
}

/// The tray colour this copy shows right now (see `show_tray_color`).
pub fn shown_tray_color() -> u8 {
    TRAY_COLOR.lock().0
}

/// `preferred` when no other running copy shows it, otherwise the first
/// colour none of them shows. With every colour taken, the preferred one is
/// shown unclaimed.
#[cfg(windows)]
fn claim_color_in(prefix: &str, preferred: u8, wait: Duration) -> (u8, Option<SessionClaim>) {
    let count = TRAY_COLORS.len() as u8;
    let preferred = if preferred < count { preferred } else { 0 };
    if let Some(claim) = claim_waiting(prefix, preferred.into(), wait) {
        return (preferred, Some(claim));
    }
    for color in (0..count).filter(|&c| c != preferred) {
        if let Some(claim) = try_claim(prefix, color.into()) {
            return (color, Some(claim));
        }
    }
    (preferred, None)
}

/// Reserve a port this copy is about to listen on, or None when another
/// InstantClone has. Two copies started at the same moment (both set to
/// start with Windows, say) would otherwise both find a shared port free,
/// since neither is listening yet, and the slower one would be left with a
/// port it can never bind. The reservation settles it: one copy gets it, the
/// other gets the usual port-conflict choice. Machine-wide, like ports.
/// `wait` covers a restart, as for the tray colour.
pub fn reserve_port(port: u16, wait: Duration) -> Option<SessionClaim> {
    #[cfg(windows)]
    {
        claim_waiting(r"Global\InstantClone.Port", port.into(), wait)
    }
    #[cfg(not(windows))]
    {
        let _ = (port, wait);
        Some(SessionClaim {})
    }
}

/// `try_claim`, trying again until `wait` is up.
#[cfg(windows)]
fn claim_waiting(prefix: &str, item: u32, wait: Duration) -> Option<SessionClaim> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(claim) = try_claim(prefix, item) {
            return Some(claim);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Create the item's named mutex, or None when another process already has
/// it (or, under `Global\`, another user's process: then opening it is
/// refused, which counts as taken too). `Local\` names are this Windows
/// session's own, so another user signed in on the PC never decides this
/// user's tray colours.
#[cfg(windows)]
fn try_claim(prefix: &str, item: u32) -> Option<SessionClaim> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = format!("{prefix}.{item}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return None;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(SessionClaim { handle })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_config(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ic-instance-lock-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("instantclone.config.json")
    }

    #[test]
    fn the_lock_sits_beside_the_settings_file_it_guards() {
        assert_eq!(
            lock_path(Path::new("C:/Streams/Main/instantclone.config.json")),
            PathBuf::from("C:/Streams/Main/instantclone.config.json.lock")
        );
        assert_eq!(
            lock_path(Path::new("./other.json")),
            PathBuf::from("./other.json.lock")
        );
    }

    /// The same setup can't be taken twice while the first holder lives,
    /// and is free again the moment it is gone.
    #[test]
    fn a_second_copy_of_the_same_setup_is_refused_until_the_first_ends() {
        let config = scratch_config("same");
        let Acquired::Locked(first) = acquire(&config, Duration::ZERO) else {
            panic!("the first copy gets the lock");
        };
        assert!(matches!(
            acquire(&config, Duration::ZERO),
            Acquired::HeldElsewhere
        ));
        drop(first);
        assert!(matches!(
            acquire(&config, Duration::ZERO),
            Acquired::Locked(_)
        ));
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// A restart waits for the outgoing process instead of giving up.
    #[test]
    fn a_restart_waits_for_the_outgoing_copy_to_let_go() {
        let config = scratch_config("restart");
        let Acquired::Locked(outgoing) = acquire(&config, Duration::ZERO) else {
            panic!("the outgoing copy holds the lock");
        };
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(outgoing);
        });
        assert!(matches!(
            acquire(&config, Duration::from_secs(5)),
            Acquired::Locked(_)
        ));
        release.join().unwrap();
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// A setup keeps its colour while nobody else shows it; a copy wanting
    /// a colour that is taken gets a free one instead, and every colour is
    /// free again once its holder is gone.
    #[cfg(windows)]
    #[test]
    fn copies_side_by_side_never_share_a_tray_colour() {
        let prefix = format!(r"Local\InstantCloneTest.TrayColor.{}", std::process::id());
        let (first, held_first) = claim_color_in(&prefix, 2, Duration::ZERO);
        assert_eq!(first, 2, "the colour it had last time");
        let (second, held_second) = claim_color_in(&prefix, 2, Duration::ZERO);
        assert_eq!(second, 0, "a copied setup takes the first free colour");
        assert!(held_first.is_some() && held_second.is_some());

        drop(held_first);
        let (again, _held) = claim_color_in(&prefix, 2, Duration::ZERO);
        assert_eq!(again, 2, "free again once its holder is gone");

        let (out_of_range, _held) = claim_color_in(&prefix, 99, Duration::ZERO);
        assert_eq!(out_of_range, 1, "a bad value reads as cyan, which is taken");
        drop(held_second);
    }

    /// A port reserved by one copy is refused to the next until the first
    /// lets go of it. In a private namespace, so a real copy running on this
    /// PC can't collide with the test.
    #[cfg(windows)]
    #[test]
    fn a_reserved_port_is_refused_to_a_second_copy() {
        let prefix = format!(r"Local\InstantCloneTest.Port.{}", std::process::id());
        let port = 7799;
        {
            let _first = claim_waiting(&prefix, port, Duration::ZERO).expect("free");
            assert!(
                claim_waiting(&prefix, port, Duration::ZERO).is_none(),
                "taken"
            );
        }
        assert!(
            claim_waiting(&prefix, port, Duration::ZERO).is_some(),
            "free again"
        );
    }

    /// The tab icon's colour matches the tray's.
    #[test]
    fn tray_colours_read_as_hex() {
        assert_eq!(tray_color_hex(0), "#5ac8fa");
        assert_eq!(tray_color_hex(1), "#bf5af2");
        assert_eq!(tray_color_hex(200), "#5ac8fa");
    }

    /// Different settings files are different setups, even in one folder.
    #[test]
    fn two_setups_lock_independently() {
        let main = scratch_config("two");
        let other = main.with_file_name("second.config.json");
        let Acquired::Locked(first) = acquire(&main, Duration::ZERO) else {
            panic!("the first setup locks");
        };
        assert!(matches!(
            acquire(&other, Duration::ZERO),
            Acquired::Locked(_)
        ));
        drop(first);
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }
}
