//! LIFX: its binary LAN protocol over UDP on port 56700, as aiolifx packs
//! it. Every packet starts with a 36-byte little-endian header; a scan
//! broadcasts "get the light's state", which answers with its label.

use super::{net, Brand, Device, Failure, State};
use std::net::SocketAddr;
use std::time::Duration;

pub const PORT: u16 = 56700;
const HEADER: usize = 36;
/// The protocol number every packet carries.
const PROTOCOL: u16 = 1024;
/// Any non-zero source: lights then answer us directly, not by broadcast.
const SOURCE: u32 = 0x1C1A_5E7D;
const LIGHT_GET: u16 = 101;
const LIGHT_SET_COLOR: u16 = 102;
const LIGHT_STATE: u16 = 107;
const LIGHT_SET_POWER: u16 = 117;
const ACKNOWLEDGEMENT: u16 = 45;
/// The white point colors are mixed at.
const KELVIN: u16 = 3500;
const TRIES: usize = 3;
const EACH: Duration = Duration::from_millis(400);

/// A packet: header (frame, frame address, protocol header) and payload.
fn packet(kind: u16, target: [u8; 8], ack: bool, payload: &[u8]) -> Vec<u8> {
    let tagged = target == [0; 8];
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&((HEADER + payload.len()) as u16).to_le_bytes());
    let flags = PROTOCOL | (1 << 12) | (u16::from(tagged) << 13);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&SOURCE.to_le_bytes());
    out.extend_from_slice(&target);
    out.extend_from_slice(&[0; 6]);
    // ack_required, and res_required for a question.
    out.push(if ack { 0b10 } else { 0b01 });
    out.push(0); // sequence
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out.extend_from_slice(payload);
    out
}

fn kind_of(data: &[u8]) -> Option<u16> {
    (data.len() >= HEADER).then(|| u16::from_le_bytes([data[32], data[33]]))
}

pub fn scan(wait: Duration) -> Vec<Device> {
    let Some(socket) = net::scan_socket() else {
        return Vec::new();
    };
    let ask = packet(LIGHT_GET, [0; 8], false, &[]);
    for to in net::broadcast_addresses() {
        let _ = socket.send_to(&ask, (to, PORT));
    }
    net::collect(&socket, wait)
        .into_iter()
        .filter_map(|(address, data)| {
            let (id, label) = light_state(&data)?;
            Some(Device {
                brand: Brand::Lifx,
                address,
                name: if label.is_empty() {
                    format!("LIFX ··{}", net::tail(&id))
                } else {
                    label
                },
                id,
                plug: false,
            })
        })
        .collect()
}

/// A LightState answer: the light's id (its MAC, as hex) and label.
fn light_state(data: &[u8]) -> Option<(String, String)> {
    if kind_of(data)? != LIGHT_STATE || data.len() < HEADER + 44 {
        return None;
    }
    let id: String = data[8..14].iter().map(|b| format!("{b:02x}")).collect();
    let label = &data[HEADER + 12..HEADER + 44];
    let end = label.iter().position(|&b| b == 0).unwrap_or(label.len());
    Some((
        id,
        String::from_utf8_lossy(&label[..end]).trim().to_string(),
    ))
}

/// The 8-byte target of a light: its MAC, then two zero bytes.
fn target(id: &str) -> Option<[u8; 8]> {
    let mut out = [0u8; 8];
    for (i, slot) in out.iter_mut().take(6).enumerate() {
        *slot = u8::from_str_radix(id.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// SetColor's payload: a reserved byte, hue, saturation, brightness and
/// kelvin (each 0 to 65535), then the fade in milliseconds.
fn color_payload(rgb: [u8; 3], brightness: u8) -> Vec<u8> {
    let (hue, saturation, _) = super::to_hsv(rgb);
    let scale = |x: f64| (x * 65535.0).round().clamp(0.0, 65535.0) as u16;
    let mut out = vec![0u8];
    out.extend_from_slice(&scale(hue / 360.0).to_le_bytes());
    out.extend_from_slice(&scale(saturation).to_le_bytes());
    out.extend_from_slice(&scale(f64::from(brightness) / 100.0).to_le_bytes());
    out.extend_from_slice(&KELVIN.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

fn power_payload(on: bool) -> Vec<u8> {
    let mut out = (if on { u16::MAX } else { 0 }).to_le_bytes().to_vec();
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

pub fn set(to: SocketAddr, id: &str, state: State) -> Result<(), Failure> {
    let target =
        target(id).ok_or_else(|| Failure::Refused("pick the LIFX light again".to_string()))?;
    let acked = |r: &[u8]| kind_of(r) == Some(ACKNOWLEDGEMENT);
    if let Some(rgb) = state.color.filter(|_| state.on) {
        let color = packet(
            LIGHT_SET_COLOR,
            target,
            true,
            &color_payload(rgb, super::percent(&state)),
        );
        net::request(to, &color, TRIES, EACH, acked)?;
    }
    let power = packet(LIGHT_SET_POWER, target, true, &power_payload(state.on));
    net::request(to, &power, TRIES, EACH, acked).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_matches_the_lan_protocol() {
        let p = packet(LIGHT_GET, [0; 8], false, &[]);
        assert_eq!(p.len(), 36);
        assert_eq!(u16::from_le_bytes([p[0], p[1]]), 36, "size");
        // protocol 1024, addressable, tagged (a broadcast)
        assert_eq!(u16::from_le_bytes([p[2], p[3]]), 0x3400);
        assert_eq!(p[22], 0b01, "res_required");
        assert_eq!(u16::from_le_bytes([p[32], p[33]]), 101);

        let mac = target("d073d5010203").unwrap();
        assert_eq!(mac, [0xd0, 0x73, 0xd5, 1, 2, 3, 0, 0]);
        let p = packet(LIGHT_SET_COLOR, mac, true, &color_payload([255, 0, 0], 100));
        assert_eq!(p.len(), 36 + 13);
        assert_eq!(u16::from_le_bytes([p[2], p[3]]), 0x1400, "not tagged");
        assert_eq!(&p[8..14], &[0xd0, 0x73, 0xd5, 1, 2, 3]);
        assert_eq!(p[22], 0b10, "ack_required");
        // hue 0, saturation and brightness full, 3500 K, no fade
        assert_eq!(
            &p[36..49],
            &[0, 0, 0, 255, 255, 255, 255, 0xac, 0x0d, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_light_state_gives_id_and_label() {
        let mut answer = packet(
            LIGHT_STATE,
            target("d073d5010203").unwrap(),
            false,
            &[0; 52],
        );
        answer[HEADER + 12..HEADER + 17].copy_from_slice(b"Desk ");
        assert_eq!(
            light_state(&answer),
            Some(("d073d5010203".to_string(), "Desk".to_string()))
        );
        assert_eq!(light_state(&answer[..40]), None, "cut short");
        assert_eq!(target("d073"), None);
    }
}
