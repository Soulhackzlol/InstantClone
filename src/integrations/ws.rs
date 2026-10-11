//! A minimal WebSocket client (RFC 6455) for the two servers integrations
//! talk to: OBS's own (plain, on this PC) and Twitch EventSub (over TLS).
//! Text frames only, no extensions, every frame we send masked.
//!
//! `peer` names the server in errors ("lost OBS: ...", "Twitch sent too
//! much at once"), so each caller's messages read as its own.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Neither server sends anything near this; more is not them talking.
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// One whole message from the server.
pub enum Message {
    Text(String),
    /// A ping, to answer with a pong carrying this payload.
    Ping(Vec<u8>),
    /// The server closed the connection; the payload starts with its code.
    Close(Vec<u8>),
}

/// The opening HTTP Upgrade. `target` is the Host header (with a port
/// when it isn't the default one) and the path; `protocol` the
/// subprotocol to ask for.
pub async fn handshake(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    target: (&str, &str),
    protocol: Option<&str>,
    peer: &str,
) -> Result<(), String> {
    let (host, path) = target;
    let mut key = [0u8; 16];
    crate::crypto::os_random(&mut key);
    let protocol = protocol.map_or(String::new(), |p| {
        format!("Sec-WebSocket-Protocol: {p}\r\n")
    });
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n{protocol}\r\n",
        base64(&key)
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("lost {peer}: {e}"))?;
    // Read the answer's head, a byte at a time: it is short, and the first
    // frame may follow right behind it.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err(format!("that isn't {peer}'s WebSocket server"));
        }
        head.push(
            stream
                .read_u8()
                .await
                .map_err(|e| format!("lost {peer}: {e}"))?,
        );
    }
    if !String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101") {
        return Err(format!("{peer}'s WebSocket server refused the connection"));
    }
    Ok(())
}

/// The next whole message. A broken connection is an error; a close is a
/// message, so the caller can say what its code means.
pub async fn read_message(
    stream: &mut (impl AsyncRead + Unpin),
    peer: &str,
) -> Result<Message, String> {
    let mut message = Vec::new();
    loop {
        let (fin, opcode, payload) = read_frame(stream, peer).await?;
        match opcode {
            // Continuation, text, binary.
            0..=2 => {
                if message.len() + payload.len() > MAX_MESSAGE {
                    return Err(format!("{peer} sent too much at once"));
                }
                message.extend_from_slice(&payload);
                if fin {
                    return Ok(Message::Text(
                        String::from_utf8_lossy(&message).into_owned(),
                    ));
                }
            }
            8 => return Ok(Message::Close(payload)),
            9 => return Ok(Message::Ping(payload)),
            _ => {}
        }
    }
}

async fn read_frame(
    stream: &mut (impl AsyncRead + Unpin),
    peer: &str,
) -> Result<(bool, u8, Vec<u8>), String> {
    let lost = |e: std::io::Error| format!("lost {peer}: {e}");
    let b0 = stream.read_u8().await.map_err(lost)?;
    let b1 = stream.read_u8().await.map_err(lost)?;
    let len = match b1 & 0x7f {
        126 => u64::from(stream.read_u16().await.map_err(lost)?),
        127 => stream.read_u64().await.map_err(lost)?,
        n => u64::from(n),
    };
    if len > MAX_MESSAGE as u64 {
        return Err(format!("{peer} sent too much at once"));
    }
    let mask = if b1 & 0x80 != 0 {
        let mut m = [0u8; 4];
        stream.read_exact(&mut m).await.map_err(lost)?;
        Some(m)
    } else {
        None
    };
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload).await.map_err(lost)?;
    if let Some(m) = mask {
        payload
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= m[i % 4]);
    }
    Ok((b0 & 0x80 != 0, b0 & 0x0f, payload))
}

/// One frame from us: always final, always masked (clients must mask).
pub async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    opcode: u8,
    payload: &[u8],
    peer: &str,
) -> Result<(), String> {
    stream
        .write_all(&encode_frame(opcode, payload))
        .await
        .map_err(|e| format!("lost {peer}: {e}"))
}

fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut mask = [0u8; 4];
    crate::crypto::os_random(&mut mask);
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Standard base64 with padding, as the handshake and obs-websocket use.
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// The handshake against a server whose answer is `answer`.
    async fn handshake_against(answer: &[u8]) -> Result<(), String> {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        server.write_all(answer).await.unwrap();
        handshake(&mut client, ("host", "/"), None, "Test").await
    }

    /// Only a switch to WebSocket completes it: a login page or a 404 is
    /// refused, and an answer whose head never ends is cut off.
    #[tokio::test]
    async fn only_a_switch_to_websocket_completes_the_handshake() {
        assert!(
            handshake_against(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
                .await
                .is_ok()
        );
        let refused = handshake_against(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
        assert!(refused.unwrap_err().contains("refused"));
        let endless = handshake_against(&[b'x'; 9_000]).await;
        assert!(endless.unwrap_err().contains("isn't"));
    }

    /// A frame announcing more than the cap is refused from its header,
    /// before a byte of it is buffered.
    #[tokio::test]
    async fn a_frame_bigger_than_the_cap_is_refused_before_it_is_read() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let mut head = vec![0x81, 127];
        head.extend_from_slice(&(MAX_MESSAGE as u64 + 1).to_be_bytes());
        server.write_all(&head).await.unwrap();
        let err = read_message(&mut client, "Test")
            .await
            .err()
            .expect("refused");
        assert!(err.contains("too much"), "{err}");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn frames_are_masked_and_sized() {
        let small = encode_frame(1, b"hi");
        assert_eq!(small[0], 0x81);
        assert_eq!(small[1], 0x80 | 2);
        let mask = [small[2], small[3], small[4], small[5]];
        assert_eq!([small[6] ^ mask[0], small[7] ^ mask[1]], *b"hi");
        let mid = encode_frame(1, &[0u8; 300]);
        assert_eq!(mid[1], 0x80 | 126);
        assert_eq!(u16::from_be_bytes([mid[2], mid[3]]), 300);
    }

    /// A server's frames (unmasked) come back whole, and pings and closes
    /// are told apart from text.
    #[tokio::test]
    async fn messages_are_read_whole() {
        let mut wire: Vec<u8> = Vec::new();
        // "he" then "llo" as a text frame and its continuation.
        wire.extend_from_slice(&[0x01, 2]);
        wire.extend_from_slice(b"he");
        wire.extend_from_slice(&[0x80, 3]);
        wire.extend_from_slice(b"llo");
        wire.extend_from_slice(&[0x89, 1, b'p']);
        wire.extend_from_slice(&[0x88, 2, 0x0f, 0xa1]);
        let mut stream = wire.as_slice();
        assert!(
            matches!(read_message(&mut stream, "X").await, Ok(Message::Text(t)) if t == "hello")
        );
        assert!(matches!(read_message(&mut stream, "X").await, Ok(Message::Ping(p)) if p == b"p"));
        assert!(
            matches!(read_message(&mut stream, "X").await, Ok(Message::Close(c)) if c == [0x0f, 0xa1])
        );
        assert!(read_message(&mut stream, "X")
            .await
            .is_err_and(|e| e.starts_with("lost X")));
    }
}
