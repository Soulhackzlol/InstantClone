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
//! Windows and `libc::setsockopt` on Linux (no `socket2` dependency),
//! no-op stubs on platforms we do not publish.
//!
//! Not every helper needs both halves, and the ones that do not say why at
//! the stub. Treat a silent `Ok(())` here as a claim that the platform
//! already does the right thing, not as a gap - `set_aggressive_keepalive`
//! was such a stub on Linux by oversight, and the result was a build that
//! set no keepalive at all.

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

/// Linux half of the same tuning. `SO_KEEPALIVE` is OFF by default on every
/// unix, so without this a Linux build sets no keepalive at all - strictly
/// worse than Windows, which at least has a two-hour default to fall back on.
///
/// That matters more here than the numbers suggest. `ingest_alive` is cleared
/// by `PublishGuard::drop`, which runs when the connection handler returns,
/// and nothing else clears it - there is no tag-timeout behind it. So a
/// half-open socket (the publisher's machine loses power, someone pulls the
/// cable, Wi-Fi drops) leaves `read_message().await` parked forever: the
/// dashboard reports the publisher alive indefinitely, and every path gated on
/// `ingest_alive` waits on a stream that is never coming back. On a headless
/// relay, which is what the Linux build is for, there is nobody at the machine
/// to notice.
///
/// 30 s idle then 3 probes 10 s apart, so a dead peer surfaces in ~60 s.
/// Windows sets the same idle and interval but fixes its own probe count, so
/// the two platforms land close rather than identical - near enough that the
/// dashboard behaves the same way on both.
#[cfg(target_os = "linux")]
pub fn set_aggressive_keepalive(sock: &TcpStream) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    fn set(fd: i32, level: libc::c_int, name: libc::c_int, val: libc::c_int) -> io::Result<()> {
        let rc = unsafe {
            libc::setsockopt(
                fd,
                level,
                name,
                &val as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    let fd = sock.as_raw_fd();
    set(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1)?;
    set(fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, 30)?;
    set(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, 10)?;
    set(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT, 3)?;
    Ok(())
}

// Windows and Linux are the platforms we publish; anywhere else keeps the
// OS defaults.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
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

/// Deliberately nothing on unix, unlike the keepalive above.
///
/// Linux auto-tunes the send buffer between the bounds in
/// `net.ipv4.tcp_wmem`, growing it under exactly the load this exists to
/// smooth. Calling `setsockopt(SO_SNDBUF)` opts that socket OUT of the
/// autotuning and pins it at the value given, so "fixing" the gap by
/// mirroring the Windows call would cap a socket the kernel would otherwise
/// have grown past 1 MB. The right amount of code here is none.
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn opt(fd: i32, level: libc::c_int, name: libc::c_int) -> libc::c_int {
        let mut v: libc::c_int = -1;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                level,
                name,
                &mut v as *mut libc::c_int as *mut libc::c_void,
                &mut len,
            )
        };
        assert_eq!(rc, 0, "getsockopt failed");
        v
    }

    /// The keepalive has to reach the socket, not just compile.
    ///
    /// This was a `Ok(())` stub on every non-Windows target, and because
    /// `SO_KEEPALIVE` is off by default on unix that left Linux with no
    /// keepalive at all - not the OS default, none. `ingest_alive` is cleared
    /// only when the connection handler returns, so a half-open socket would
    /// have parked the read forever and reported the publisher alive for good.
    #[test]
    fn keepalive_reaches_the_socket_on_linux() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            use std::os::fd::AsRawFd;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listen");
            let sock = tokio::net::TcpStream::connect(listener.local_addr().expect("addr"))
                .await
                .expect("connect");
            let fd = sock.as_raw_fd();

            assert_eq!(
                opt(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE),
                0,
                "unix leaves keepalive off, which is why the stub was worse than a default"
            );

            set_aggressive_keepalive(&sock).expect("set keepalive");

            assert_eq!(opt(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE), 1);
            // 30 s idle then 3 probes 10 s apart: a dead peer surfaces in ~60 s.
            assert_eq!(opt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE), 30);
            assert_eq!(opt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL), 10);
            assert_eq!(opt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT), 3);
        });
    }
}
