//! Startup port pre-flight: detect if the configured RTMP / web ports are
//! already in use before we try to bind them.
//!
//! The bind checks (`is_port_free`, `find_free_port`, `wait_until_bindable`)
//! are pure `std::net` and run on every platform. The extras that identify
//! the owning process (IpHelper) and prompt the user (MessageBox) are
//! Windows-only, gated below; the caller in main.rs supplies a headless
//! stderr path for those on other platforms.
//!
//! Why we have it: a silent bind-loop on a busy port is the worst possible
//! first-run UX - on Windows there's no console (`windows_subsystem =
//! "windows"`), on a headless VPS there's no one watching a hung process.

use std::net::TcpListener;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, NO_ERROR};
#[cfg(windows)]
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::{ntohs, AF_INET};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONWARNING, MB_OK, MB_YESNO,
};

/// Header struct of the variable-length table returned by
/// `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)`. We never
/// instantiate this - we cast a `Vec<u8>` buffer to it.
#[cfg(windows)]
#[repr(C)]
struct MibTcpTableOwnerPid {
    num_entries: u32,
    table: [MIB_TCPROW_OWNER_PID; 1],
}

/// Pid + full executable path of the process owning a given listening
/// TCP port. Returns `None` if no listener is on that port, or if the
/// OS-level lookup failed entirely.
#[cfg(windows)]
pub fn find_process_on_port(port: u16) -> Option<(u32, Option<String>)> {
    unsafe {
        // First call: get required buffer size.
        let mut size: u32 = 0;
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0, // bOrder = FALSE
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if size == 0 {
            return None;
        }

        let mut buf = vec![0u8; size as usize];
        let r = GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if r != NO_ERROR {
            return None;
        }

        let table = &*(buf.as_ptr() as *const MibTcpTableOwnerPid);
        let n = table.num_entries as usize;
        let rows = table.table.as_ptr();
        for i in 0..n {
            let row = &*rows.add(i);
            // `dwLocalPort` is a DWORD with the port in network byte
            // order packed into the low 16 bits. Use ntohs to convert.
            let row_port = ntohs(row.dwLocalPort as u16);
            if row_port == port {
                let pid = row.dwOwningPid;
                return Some((pid, process_name(pid)));
            }
        }
        None
    }
}

/// Best-effort full executable path for a PID. `None` if the OS denies
/// us access (e.g. PID belongs to an elevated process) - the caller
/// still has the PID to show.
#[cfg(windows)]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        // The full path: for another InstantClone, its folder is the one
        // detail that tells the user which copy it is.
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// The file name part of an executable path, for showing in a dialog.
#[cfg(windows)]
fn exe_basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether `path` is an InstantClone exe. By file name, since a portable
/// copy can sit in any folder: `instantclone.exe`, or a renamed download
/// such as `InstantClone-0.1.16.exe`.
#[cfg(windows)]
fn is_instantclone_exe(path: &str) -> bool {
    exe_basename(path)
        .to_ascii_lowercase()
        .starts_with("instantclone")
}

/// Whether another running InstantClone (not this process) listens on
/// `port`. Used to leave OBS's registration alone when it points at a copy
/// that is running right now.
///
/// Windows names the owner, so this is exact there. Elsewhere there is no
/// cheap way to ask, so anything listening counts: the caller only uses the
/// answer to decide NOT to rewrite something, which is the safe direction to
/// be wrong in.
pub fn port_held_by_other_instance(port: u16) -> bool {
    #[cfg(windows)]
    {
        find_process_on_port(port).is_some_and(|(pid, exe)| {
            pid != std::process::id() && exe.as_deref().is_some_and(is_instantclone_exe)
        })
    }
    #[cfg(not(windows))]
    {
        !is_port_free(&format!("127.0.0.1:{port}"))
    }
}

/// True iff we can bind a TCP listener at `addr` right now. Released
/// immediately. Cheap pre-flight check.
pub fn is_port_free(addr: &str) -> bool {
    TcpListener::bind(addr).is_ok()
}

/// Poll until `addr` becomes bindable, up to `timeout`. Returns `true` the
/// instant a bind succeeds, `false` if the whole window elapses still busy.
///
/// Used on a restart / self-update relaunch: the outgoing instance can take a
/// moment to release its listening socket (Windows may even still attribute
/// the port to the now-dead PID), and we'd rather wait that out silently than
/// pop the "switch port?" modal mid-handoff. The window is generous because a
/// genuine conflict is rare and a spurious modal on every restart is worse.
pub fn wait_until_bindable(addr: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_port_free(addr) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Find the first port in `[start ..= start+range]` that we can bind
/// on `host`. Returns `None` if every port in the window is busy.
pub fn find_free_port(host: &str, start: u16, range: u16) -> Option<u16> {
    for offset in 0..=range {
        let port = start.checked_add(offset)?;
        if is_port_free(&format!("{}:{}", host, port)) {
            return Some(port);
        }
    }
    None
}

#[cfg(windows)]
pub enum ConflictChoice {
    SwitchPort(u16),
    Quit,
}

/// Pop a native modal asking the user what to do about a port conflict.
/// `label` is shown to the user as "RTMP port" / "web port".
/// `starting_copy`: another InstantClone that is starting has claimed the
/// port (see `instance_lock::reserve_port`), whoever listens on it yet.
#[cfg(windows)]
pub fn ask_user(
    label: &str,
    port: u16,
    owner: Option<(u32, Option<String>)>,
    proposed_port: Option<u16>,
    starting_copy: bool,
) -> ConflictChoice {
    let body_text = conflict_message(label, port, owner.as_ref(), proposed_port, starting_copy);
    let flags = if proposed_port.is_some() {
        MB_YESNO | MB_ICONWARNING
    } else {
        MB_OK | MB_ICONWARNING
    };
    let title = wide("InstantClone");
    let body = wide(&body_text);
    let r = unsafe { MessageBoxW(ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) };
    match (r, proposed_port) {
        (IDYES, Some(p)) => ConflictChoice::SwitchPort(p),
        _ => ConflictChoice::Quit,
    }
}

/// The text of the port-conflict dialog.
///
/// Another InstantClone gets its own wording. The same setup can't get here
/// (a second copy of it never starts, see `instance_lock`), so this is a copy
/// from a different folder, and moving this one to a free port is the normal
/// way to run two side by side. Its folder is what tells the user which copy
/// it is, so that is what the dialog names.
#[cfg(windows)]
fn conflict_message(
    label: &str,
    port: u16,
    owner: Option<&(u32, Option<String>)>,
    proposed_port: Option<u16>,
    starting_copy: bool,
) -> String {
    let other_copy = owner.and_then(|(_, exe)| exe.as_deref().filter(|e| is_instantclone_exe(e)));
    let is_copy = other_copy.is_some() || (owner.is_none() && starting_copy);
    let owner_text = match (owner, other_copy) {
        (Some((pid, _)), Some(exe)) => {
            let folder = exe.rsplit_once(['\\', '/']).map_or(exe, |(dir, _)| dir);
            format!("another InstantClone (PID {pid}), running from:\n{folder}")
        }
        (Some((pid, Some(exe))), None) => format!("{} (PID {pid})", exe_basename(exe)),
        (Some((pid, None)), None) => format!("PID {pid} (process name unavailable)"),
        (None, _) if starting_copy => "another InstantClone that is starting up".to_string(),
        (None, _) => "another process".to_string(),
    };
    match proposed_port {
        Some(new_port) if is_copy => format!(
            "{label} :{port} is already used by {owner_text}\n\n\
             Use :{new_port} for this copy and continue? It is saved in this copy's \
             settings, so OBS and the dashboard address for this copy use :{new_port} from now on."
        ),
        Some(new_port) => format!(
            "{label} :{port} is already being used by {owner_text}.\n\n\
             Switch to :{new_port} and continue?\n\
             You'll need to update OBS (and the dashboard URL) \
             to use the new port."
        ),
        None => format!(
            "{label} :{port} is already being used by {owner_text}.\n\n\
             No free port found from :{} to :{}.\n\
             Free a port (close that program) or change \
             the configured port in instantclone.config.json, then restart.",
            port.saturating_add(1),
            port.saturating_add(9),
        ),
    }
}

/// Pop a one-shot native error dialog. Used by main.rs for fatal
/// cold-start failures that would otherwise leave the user staring at
/// a closed (or never-opened) console - buffer file unwritable, etc.
/// Windows-only; other platforms print to stderr at the call site.
#[cfg(windows)]
pub fn show_error(title: &str, body: &str) {
    let title_w = wide(title);
    let body_w = wide(body);
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            body_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Bind on an OS-chosen ephemeral port (port 0) so the test doesn't
    /// clash with anything else on the dev box.
    fn bind_eph() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let p = l.local_addr().unwrap().port();
        (l, p)
    }

    #[test]
    fn is_port_free_distinguishes_held_from_open() {
        let (held, port) = bind_eph();
        assert!(
            !is_port_free(&format!("127.0.0.1:{port}")),
            "held port must NOT report as free"
        );
        drop(held);
        // Brief breath for the OS to release the socket. On Windows the
        // socket lingers briefly after drop; the loop tolerates this.
        for _ in 0..20 {
            if is_port_free(&format!("127.0.0.1:{port}")) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("released port did not become free");
    }

    #[test]
    fn wait_until_bindable_returns_when_socket_released() {
        use std::time::Duration;
        let (held, port) = bind_eph();
        let addr = format!("127.0.0.1:{port}");
        // Release the socket after a short hold, on another thread.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });
        assert!(
            wait_until_bindable(&addr, Duration::from_secs(5)),
            "must report bindable once the holder releases within the window"
        );
    }

    #[test]
    fn wait_until_bindable_times_out_on_a_held_port() {
        use std::time::Duration;
        let (_held, port) = bind_eph(); // held for the whole test
        let addr = format!("127.0.0.1:{port}");
        assert!(
            !wait_until_bindable(&addr, Duration::from_millis(400)),
            "a permanently held port must time out as not bindable"
        );
    }

    #[test]
    fn find_free_port_skips_held_and_returns_next() {
        const WINDOW: u16 = 50;
        // Hold a known port and ask find_free_port to start there. It
        // should jump to a later one. The OS can hand out an ephemeral port
        // within WINDOW of 65535, where the scan has no room to move on and
        // `port + WINDOW` would overflow, so draw again until it has room.
        let (held, port) = loop {
            let (listener, port) = bind_eph();
            if port <= u16::MAX - WINDOW {
                break (listener, port);
            }
        };
        let found = find_free_port("127.0.0.1", port, WINDOW)
            .expect("some port in the window must be free");
        assert_ne!(found, port, "must skip the held port");
        assert!(found > port && found <= port + WINDOW);
        drop(held);
    }

    /// A port held by another InstantClone says so, names its folder, and
    /// offers the next port as the normal way to run two copies.
    #[cfg(windows)]
    #[test]
    fn a_port_held_by_another_instantclone_names_its_folder() {
        let owner = (
            4242,
            Some(r"C:\Streams\Second\InstantClone.exe".to_string()),
        );
        let text = conflict_message("RTMP port", 1935, Some(&owner), Some(1936), false);
        assert!(text.contains("another InstantClone (PID 4242)"), "{text}");
        assert!(text.contains(r"C:\Streams\Second"), "{text}");
        assert!(
            !text.contains("InstantClone.exe"),
            "the folder, not the exe: {text}"
        );
        assert!(text.contains("Use :1936 for this copy"), "{text}");

        let other = (77, Some(r"C:\Tools\nginx.exe".to_string()));
        let text = conflict_message("RTMP port", 1935, Some(&other), Some(1936), false);
        assert!(text.contains("nginx.exe (PID 77)"), "{text}");
        assert!(text.contains("Switch to :1936"), "{text}");

        // Claimed by a copy that is starting and hasn't bound it yet.
        let text = conflict_message("RTMP port", 1935, None, Some(1936), true);
        assert!(
            text.contains("another InstantClone that is starting up"),
            "{text}"
        );
        assert!(text.contains("Use :1936 for this copy"), "{text}");
        assert!(!text.contains('\u{2014}'), "no em dashes in dialog copy");

        assert!(is_instantclone_exe(r"D:\x\instantclone-0.1.16.exe"));
        assert!(!is_instantclone_exe(r"D:\instantclone\obs64.exe"));
    }

    /// This process listening is never "another" InstantClone.
    #[cfg(windows)]
    #[test]
    fn our_own_port_is_not_another_instance() {
        let (_held, port) = bind_eph();
        assert!(!port_held_by_other_instance(port));
    }

    #[cfg(windows)]
    #[test]
    fn find_process_on_port_finds_self_when_holding_socket() {
        // Bind a port in this very process, then ask the FFI who owns
        // it. The answer must be our own PID. Process name resolution
        // can vary (test binary path), so we only assert PID equality.
        let (held, port) = bind_eph();
        let owner = find_process_on_port(port);
        drop(held);

        let (pid, _name) = owner.expect("FFI should locate the listener");
        assert_eq!(
            pid,
            std::process::id(),
            "owning PID must be this test process"
        );
    }
}
