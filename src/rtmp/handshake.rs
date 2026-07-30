//! "Simple" (non-FP9) RTMP handshake. Both OBS and Twitch accept it.
//!
//! Wire sequence:
//!   client → server: C0 (1 byte version=3) + C1 (1536 bytes: 4 time + 4 zero + 1528 random)
//!   server → client: S0 (version=3) + S1 (1536 bytes) + S2 (echo of C1)
//!   client → server: C2 (echo of S1)
//!
//! No HMAC, no digest. Just byte equality and a version check.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

const HS_SIZE: usize = 1536;

/// Server-side handshake (we're acting as the RTMP listener for OBS).
pub async fn perform_server(sock: &mut TcpStream) -> io::Result<()> {
    let mut c0 = [0u8; 1];
    sock.read_exact(&mut c0).await?;
    if c0[0] != 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad RTMP version",
        ));
    }
    let mut c1 = vec![0u8; HS_SIZE];
    sock.read_exact(&mut c1).await?;

    // S0 + S1 + S2 in one write to reduce syscalls. S2 is an echo of C1.
    let mut out = Vec::with_capacity(1 + HS_SIZE * 2);
    out.push(3);
    out.extend_from_slice(&fill_handshake_payload());
    out.extend_from_slice(&c1);
    sock.write_all(&out).await?;

    let mut c2 = vec![0u8; HS_SIZE];
    sock.read_exact(&mut c2).await?;
    Ok(())
}

/// Client-side handshake (we're connecting outbound to Twitch/YouTube).
/// Generic over the socket type so it works over both a plain TCP stream
/// and a TLS stream (RTMPS egress to Kick).
pub async fn perform_client<S: AsyncRead + AsyncWrite + Unpin>(sock: &mut S) -> io::Result<()> {
    let mut out = Vec::with_capacity(1 + HS_SIZE);
    out.push(3);
    let c1 = fill_handshake_payload();
    out.extend_from_slice(&c1);
    sock.write_all(&out).await?;

    let mut s0 = [0u8; 1];
    sock.read_exact(&mut s0).await?;
    if s0[0] != 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad RTMP version",
        ));
    }
    let mut s1 = vec![0u8; HS_SIZE];
    sock.read_exact(&mut s1).await?;
    let mut s2 = vec![0u8; HS_SIZE];
    sock.read_exact(&mut s2).await?;

    sock.write_all(&s1).await?; // C2 = echo of S1
    Ok(())
}

fn fill_handshake_payload() -> [u8; HS_SIZE] {
    let mut buf = [0u8; HS_SIZE];
    // Bytes 0..4 = time (uptime ms, we send 0 - accepted in practice).
    // Bytes 4..8 = version zeros for simple handshake.
    // Bytes 8.. = "random" - we fill with a cheap PRNG seeded by addr-of-stack.
    let seed = (&buf as *const _ as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut s = seed | 1;
    for chunk in buf[8..].chunks_mut(8) {
        // xorshift64
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let bytes = s.to_le_bytes();
        let n = chunk.len();
        chunk.copy_from_slice(&bytes[..n]);
    }
    buf
}
