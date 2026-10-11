//! What the brands share: this PC's place on the network, broadcasting a
//! question and hearing every answer, and a UDP request sent again until
//! it is answered.

use super::Failure;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// The largest datagram any of these devices sends.
const MAX_DATAGRAM: usize = 4096;

/// This PC's address on the network it reaches the internet through.
/// Connecting a UDP socket only picks the route: nothing is sent.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(v4) if !v4.ip().is_loopback() && !v4.ip().is_unspecified() => Some(*v4.ip()),
        _ => None,
    }
}

/// Where a broadcast goes: everywhere this PC's network reaches, and that
/// network's own broadcast address. Windows sends 255.255.255.255 out of
/// one adapter only, which with a VPN or a virtual network can be the
/// wrong one. Home networks are /24 almost always.
pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    if let Some(ip) = local_ipv4() {
        let [a, b, c, _] = ip.octets();
        out.push(Ipv4Addr::new(a, b, c, 255));
    }
    out
}

/// A socket for a scan: any port, allowed to broadcast, waiting `wait` at
/// most for each answer.
pub fn scan_socket() -> Option<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.set_broadcast(true).ok()?;
    socket.set_multicast_ttl_v4(2).ok()?;
    Some(socket)
}

/// Every datagram that arrives until `wait` has passed, with who sent it.
pub fn collect(socket: &UdpSocket, wait: Duration) -> Vec<(Ipv4Addr, Vec<u8>)> {
    let deadline = Instant::now() + wait;
    let mut out = Vec::new();
    let mut buf = [0u8; MAX_DATAGRAM];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
            return out;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => out.push((*from.ip(), buf[..n].to_vec())),
            Ok(_) => {}
            // A timeout, or Windows reporting an earlier send's ICMP
            // "port unreachable" here: keep listening until the deadline.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(_) => return out,
        }
    }
}

/// Send `message` to `to` until `answered` likes a reply: `tries` times,
/// waiting `each` for an answer every time. UDP gets lost now and then,
/// on Wi-Fi more than anywhere.
pub fn request(
    to: SocketAddr,
    message: &[u8],
    tries: usize,
    each: Duration,
    answered: impl Fn(&[u8]) -> bool,
) -> Result<Vec<u8>, Failure> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| Failure::Unreachable(format!("couldn't open a network socket: {e}")))?;
    let mut buf = [0u8; MAX_DATAGRAM];
    for _ in 0..tries {
        socket
            .send_to(message, to)
            .map_err(|e| Failure::Unreachable(format!("couldn't reach {}: {e}", to.ip())))?;
        let deadline = Instant::now() + each;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
                break;
            }
            match socket.recv_from(&mut buf) {
                Ok((n, from)) if from.ip() == to.ip() && answered(&buf[..n]) => {
                    return Ok(buf[..n].to_vec())
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                Err(_) => break,
            }
        }
    }
    Err(Failure::Unreachable(format!(
        "nothing answered at {}",
        to.ip()
    )))
}

/// The last four characters of an id, for a name: "WiZ light ··f94d".
pub fn tail(id: &str) -> String {
    let chars: Vec<char> = id.chars().filter(char::is_ascii_alphanumeric).collect();
    let start = chars.len().saturating_sub(4);
    chars[start..]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_sent_again_until_answered() {
        let device = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = device.local_addr().unwrap();
        let fake = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            // Drop the first one, as a lossy Wi-Fi might.
            let _ = device.recv_from(&mut buf).unwrap();
            let (n, from) = device.recv_from(&mut buf).unwrap();
            device.send_to(&buf[..n], from).unwrap();
        });
        let answer = request(at, b"ping", 3, Duration::from_millis(300), |r| r == b"ping");
        assert_eq!(answer, Ok(b"ping".to_vec()));
        fake.join().unwrap();
    }

    #[test]
    fn silence_counts_as_unreachable() {
        let quiet = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = quiet.local_addr().unwrap();
        let answer = request(at, b"hello", 2, Duration::from_millis(50), |_| true);
        assert!(matches!(answer, Err(Failure::Unreachable(_))), "{answer:?}");
    }

    #[test]
    fn names_end_with_the_id() {
        assert_eq!(tail("a8:bb:50:A4:F9:4D"), "f94d");
        assert_eq!(tail("ab"), "ab");
    }
}
