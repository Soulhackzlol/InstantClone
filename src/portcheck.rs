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

/// Pid + executable filename of the process owning a given listening
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

/// Best-effort executable basename for a PID. `None` if the OS denies
/// us access (e.g. PID belongs to an elevated process) - the caller
/// still has the PID to show.
#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
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
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        // Trim to the basename - full path is noise in a user dialog.
        Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string())
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
#[cfg(windows)]
pub fn ask_user(
    label: &str,
    port: u16,
    owner: Option<(u32, Option<String>)>,
    proposed_port: Option<u16>,
) -> ConflictChoice {
    let owner_text = match owner {
        Some((pid, Some(name))) => format!("{} (PID {})", name, pid),
        Some((pid, None)) => format!("PID {} (process name unavailable)", pid),
        None => "another process".to_string(),
    };

    let (body_text, flags, accept) = match proposed_port {
        Some(new_port) => (
            format!(
                "{label} :{port} is already being used by {owner_text}.\n\n\
                 Switch to :{new_port} and continue?\n\
                 You'll need to update OBS (and the dashboard URL) \
                 to use the new port."
            ),
            MB_YESNO | MB_ICONWARNING,
            Some(new_port),
        ),
        None => (
            format!(
                "{label} :{port} is already being used by {owner_text}.\n\n\
                 No free port found in the range :{port} to :{}.\n\
                 Free a port (close that program) or change \
                 the configured port in instantclone.config.json, then restart.",
                port.saturating_add(9),
            ),
            MB_OK | MB_ICONWARNING,
            None,
        ),
    };

    let title = wide("InstantClone");
    let body = wide(&body_text);
    let r = unsafe { MessageBoxW(ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) };
    match (r, accept) {
        (IDYES, Some(p)) => ConflictChoice::SwitchPort(p),
        _ => ConflictChoice::Quit,
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
        // Hold a known port and ask find_free_port to start there.
        // It should jump to a later one.
        let (held, port) = bind_eph();
        let found = find_free_port("127.0.0.1", port, 50)
            .expect("some port in the +50 window must be free");
        assert_ne!(found, port, "must skip the held port");
        assert!(found > port && found <= port + 50);
        drop(held);
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
