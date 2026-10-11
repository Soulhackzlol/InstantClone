//! Govee's LAN API, as govee-local-api uses it. Each device needs "LAN
//! Control" switched on in the Govee Home app first.
//!
//! A scan goes to the multicast group on port 4001 and devices answer on
//! port 4002 of this PC; commands go to the device on port 4003, which
//! sends no answer to them.

use super::{net, Brand, Device, Failure, State};
use crate::json::{self, Value};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SCAN_PORT: u16 = 4001;
const ANSWER_PORT: u16 = 4002;
pub const COMMAND_PORT: u16 = 4003;
const SCAN: &[u8] = br#"{"msg":{"cmd":"scan","data":{"account_topic":"reserve"}}}"#;
/// Commands are never answered, so each goes out twice: a lost one on
/// Wi-Fi would otherwise go unnoticed.
const SENDS: usize = 2;
const BETWEEN: Duration = Duration::from_millis(60);

pub fn scan(wait: Duration) -> Vec<Device> {
    // Answers come to a fixed port. Another Govee tool on this PC may hold
    // it; then there's no hearing them.
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, ANSWER_PORT)) else {
        return Vec::new();
    };
    let _ = socket.send_to(SCAN, (GROUP, SCAN_PORT));
    net::collect(&socket, wait)
        .into_iter()
        .filter_map(|(from, data)| device_of(from, &data))
        .collect()
}

/// A scan answer: `{"msg":{"cmd":"scan","data":{"ip":..,"device":..,"sku":..}}}`.
fn device_of(from: Ipv4Addr, data: &[u8]) -> Option<Device> {
    let v = json::parse(&String::from_utf8_lossy(data)).ok()?;
    if v.path("msg.cmd").and_then(Value::as_str) != Some("scan") {
        return None;
    }
    let d = v.path("msg.data")?;
    let id = d.str_or("device", "").to_string();
    if id.is_empty() {
        return None;
    }
    let address = d.str_or("ip", "").parse().unwrap_or(from);
    let model = d.str_or("sku", "");
    Some(Device {
        brand: Brand::Govee,
        address,
        name: format!("Govee {model} ··{}", net::tail(&id)).replace("  ", " "),
        id,
        plug: false,
    })
}

fn command(cmd: &str, data: Value) -> String {
    json::obj([("msg", json::obj([("cmd", json::str(cmd)), ("data", data)]))]).to_json()
}

/// The commands for a state, in order: power, then color, then brightness.
pub fn messages(state: State) -> Vec<String> {
    let number = |n: u8| Value::Num(f64::from(n));
    let mut out = vec![command(
        "turn",
        json::obj([("value", number(u8::from(state.on)))]),
    )];
    if !state.on {
        return out;
    }
    if let Some([r, g, b]) = state.color {
        let color = json::obj([("r", number(r)), ("g", number(g)), ("b", number(b))]);
        out.push(command(
            "colorwc",
            json::obj([("color", color), ("colorTemInKelvin", number(0))]),
        ));
    }
    if state.color.is_some() || state.brightness.is_some() {
        out.push(command(
            "brightness",
            json::obj([("value", number(super::percent(&state)))]),
        ));
    }
    out
}

pub fn set(to: SocketAddr, state: State) -> Result<(), Failure> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| Failure::Unreachable(format!("couldn't open a network socket: {e}")))?;
    for message in messages(state) {
        for _ in 0..SENDS {
            socket
                .send_to(message.as_bytes(), to)
                .map_err(|e| Failure::Unreachable(format!("couldn't reach {}: {e}", to.ip())))?;
            std::thread::sleep(BETWEEN);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_answers_name_the_device() {
        let answer = br#"{"msg":{"cmd":"scan","data":{"ip":"192.168.1.23","device":"1F:80:C5:32:32:36:72:4E","sku":"H618E","bleVersionHard":"3.01.01","wifiVersionSoft":"1.02.03"}}}"#;
        let d = device_of(Ipv4Addr::new(192, 168, 1, 99), answer).unwrap();
        assert_eq!(
            d.address,
            Ipv4Addr::new(192, 168, 1, 23),
            "the address it says"
        );
        assert_eq!(d.id, "1F:80:C5:32:32:36:72:4E");
        assert_eq!(d.name, "Govee H618E ··724e");
        assert!(device_of(d.address, br#"{"msg":{"cmd":"devStatus","data":{}}}"#).is_none());
    }

    #[test]
    fn a_color_is_power_color_then_brightness() {
        let state = State {
            on: true,
            color: Some([255, 0, 0]),
            brightness: Some(60),
        };
        assert_eq!(
            messages(state),
            [
                r#"{"msg":{"cmd":"turn","data":{"value":1}}}"#,
                r#"{"msg":{"cmd":"colorwc","data":{"color":{"r":255,"g":0,"b":0},"colorTemInKelvin":0}}}"#,
                r#"{"msg":{"cmd":"brightness","data":{"value":60}}}"#,
            ]
        );
        let off = State {
            on: false,
            color: Some([1, 2, 3]),
            brightness: None,
        };
        assert_eq!(messages(off).len(), 1, "off is only off");
    }
}
