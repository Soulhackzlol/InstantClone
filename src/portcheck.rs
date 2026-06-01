//! Startup port pre-flight: detect if the configured RTMP / web ports
//! are already in use, identify the owning process, ask the user via a
//! native MessageBox whether to fall back to the next free port or quit.
//!
//! All FFI is Windows-only; the non-Windows stubs make this a no-op.
//!
//! Why we have it: with `windows_subsystem = "windows"` there is no
//! console, so a silent bind-loop on a busy port is the worst possible
//! first-run UX. A modal dialog with the offending process's name is
//! the cheapest fix that doesn't add a runtime dep.

#![cfg(windows)]

use std::ffi::OsStr;
use std::net::TcpListener;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{ntohs, AF_INET};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONWARNING, MB_OK, MB_YESNO,
};

/// Header struct of the variable-length table returned by
/// `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)`. We never
/// instantiate this — we cast a `Vec<u8>` buffer to it.
#[repr(C)]
struct MibTcpTableOwnerPid {
    num_entries: u32,
    table: [MIB_TCPROW_OWNER_PID; 1],
}

/// Pid + executable filename of the process owning a given listening
/// TCP port. Returns `None` if no listener is on that port, or if the
/// OS-level lookup failed entirely.
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
/// us access (e.g. PID belongs to an elevated process) — the caller
/// still has the PID to show.
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
        // Trim to the basename — full path is noise in a user dialog.
        Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string())
    }
}

/// True iff we can bind a TCP listener at `addr` right now. Released
/// immediately. Cheap pre-flight check.
pub fn is_port_free(addr: &str) -> bool {
    TcpListener::bind(addr).is_ok()
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

pub enum ConflictChoice {
    SwitchPort(u16),
    Quit,
}

/// Pop a native modal asking the user what to do about a port conflict.
/// `label` is shown to the user as "RTMP port" / "web port".
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
/// a closed (or never-opened) console — buffer file unwritable, etc.
/// Safe to call from any thread; on non-Windows it's a no-op.
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
