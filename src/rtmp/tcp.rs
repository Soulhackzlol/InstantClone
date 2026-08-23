//! TCP tuning helpers shared by ingest + egress sockets.
//!
//! Why this matters:
//!   * INGEST: if OBS's process hangs without closing the socket, Windows's
//!     default keepalive (2 hours) means our dashboard reports OBS as
//!     "alive" long after it stopped sending bytes. Aggressive keepalive
//!     surfaces the real state in ~30 s.
//!   * EGRESS: Twitch / NAT / firewall idle-paths sometimes silently drop
//!     long connections. Without probes, we'd only discover a dead socket
//!     on the next send - by then the viewer has been frozen for a while.
//!   * INGEST (IPv6): `set_v6_only` keeps our two listen sockets from
//!     overlapping, and keeps v4 peers from arriving v4-mapped.
//!
//! Implementation: raw `WSAIoctl(SIO_KEEPALIVE_VALS)` / `setsockopt` on
//! Windows and `libc::setsockopt` on unix (no `socket2` dependency),
//! no-op stubs elsewhere.

use std::io;
use tokio::net::TcpStream;

#[cfg(target_os = "windows")]
pub fn set_aggressive_keepalive(sock: &TcpStream) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    #[repr(C)]
    struct TcpKeepalive {
        onoff: u32,
        keepalivetime: u32,
        keepaliveinterval: u32,
    }
    const SIO_KEEPALIVE_VALS: u32 = 0x9800_0004;

    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAIoctl(
            s: usize,
            dw_io_control_code: u32,
            lpv_in_buffer: *const std::ffi::c_void,
            cb_in_buffer: u32,
            lpv_out_buffer: *mut std::ffi::c_void,
            cb_out_buffer: u32,
            lpcb_bytes_returned: *mut u32,
            lp_overlapped: *mut std::ffi::c_void,
            lp_completion_routine: *mut std::ffi::c_void,
        ) -> i32;
    }

    let raw = sock.as_raw_socket() as usize;
    let kpa = TcpKeepalive {
        onoff: 1,
        keepalivetime: 30_000,
        keepaliveinterval: 10_000,
    };
    let mut returned: u32 = 0;
    let rc = unsafe {
        WSAIoctl(
            raw,
            SIO_KEEPALIVE_VALS,
            &kpa as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<TcpKeepalive>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_aggressive_keepalive(_sock: &TcpStream) -> io::Result<()> {
    Ok(())
}

/// Increase the kernel send buffer for a TCP socket. The Windows default
/// (~64 KB) is enough at typical streaming bitrates but starts blocking
/// our writes at 10+ Mbps when the receiving platform (Twitch / YouTube)
/// is momentarily slow to ACK - every send waits for buffer space,
/// fighting the bursty IDR pattern. Bumping to 1 MB gives roughly
/// 800 ms of slack at 10 Mbps, costing one extra MB per active egress
/// connection (negligible vs. the 300 MB disk-ring buffer).
///
/// Best-effort: any setsockopt failure is returned but callers should
/// treat it as non-fatal - the smaller default buffer still works, just
/// with more write-side back-pressure under load.
#[cfg(target_os = "windows")]
pub fn set_send_buffer(sock: &TcpStream, bytes: u32) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{setsockopt, SOL_SOCKET, SO_SNDBUF};

    let raw = sock.as_raw_socket() as usize;
    let val = bytes as i32;
    let rc = unsafe {
        setsockopt(
            raw,
            SOL_SOCKET,
            SO_SNDBUF,
            &val as *const i32 as *const u8,
            std::mem::size_of::<i32>() as i32,
        )
    };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_send_buffer(_sock: &TcpStream, _bytes: u32) -> io::Result<()> {
    Ok(())
}

/// Pin an unbound IPv6 socket to IPv6 only, so it never claims v4-mapped
/// addresses. Must be called BEFORE `bind`: the option is fixed once the
/// socket is bound.
///
/// Not optional, for two reasons.
///
/// Platforms disagree on the default. Windows starts with the option on,
/// Linux with it off (`net.ipv6.bindv6only=0`), so a dual-stack `[::]`
/// socket would swallow v4-mapped addresses on Linux and collide with our
/// separate `0.0.0.0` bind, while behaving fine on Windows. Setting it
/// makes both platforms run the same two-socket layout.
///
/// It also keeps peer addresses honest. A dual-stack socket reports v4
/// peers as `::ffff:127.0.0.1`, and `Ipv6Addr::is_loopback` is false for
/// v4-mapped addresses, so `Controller::begin_publish` would classify a
/// local OBS as remote and apply the wrong-key rate limiter to someone
/// sitting at the machine.
#[cfg(target_os = "windows")]
pub fn set_v6_only(sock: &tokio::net::TcpSocket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{setsockopt, IPPROTO_IPV6, IPV6_V6ONLY};

    let on: i32 = 1;
    let rc = unsafe {
        setsockopt(
            sock.as_raw_socket() as usize,
            IPPROTO_IPV6,
            IPV6_V6ONLY,
            &on as *const i32 as *const u8,
            std::mem::size_of::<i32>() as i32,
        )
    };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub fn set_v6_only(sock: &tokio::net::TcpSocket) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let on: libc::c_int = 1;
    let rc = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            &on as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
pub fn set_v6_only(_sock: &tokio::net::TcpSocket) -> io::Result<()> {
    Ok(())
}
