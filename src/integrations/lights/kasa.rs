//! TP-Link Kasa plugs and bulbs that still speak the original local
//! protocol, as python-kasa's XOR transport does: JSON "encrypted" with an
//! autokey XOR starting at 171. A scan broadcasts `get_sysinfo` over UDP
//! on port 9999; commands go over TCP on 9999, each prefixed by its length.
//!
//! Newer Kasa firmware and Tapo devices need a TP-Link account login
//! (their KLAP protocol) and aren't found by this scan.

use super::{net, Brand, Device, Failure, State};
use crate::json::{self, Value};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

pub const PORT: u16 = 9999;
const KEY: u8 = 171;
const SYSINFO: &str = r#"{"system":{"get_sysinfo":{}}}"#;
const TIMEOUT: Duration = Duration::from_secs(3);
/// A Kasa answer is a few kilobytes at most.
const MAX_ANSWER: usize = 64 * 1024;
const LIGHT_SERVICE: &str = "smartlife.iot.smartbulb.lightingservice";

fn encrypt(plain: &str) -> Vec<u8> {
    let mut key = KEY;
    plain
        .bytes()
        .map(|b| {
            key ^= b;
            key
        })
        .collect()
}

fn decrypt(cipher: &[u8]) -> String {
    let mut key = KEY;
    let plain: Vec<u8> = cipher
        .iter()
        .map(|&c| {
            let p = key ^ c;
            key = c;
            p
        })
        .collect();
    String::from_utf8_lossy(&plain).into_owned()
}

pub fn scan(wait: Duration) -> Vec<Device> {
    let Some(socket) = net::scan_socket() else {
        return Vec::new();
    };
    // Over UDP the length prefix is left out.
    let ask = encrypt(SYSINFO);
    for to in net::broadcast_addresses() {
        let _ = socket.send_to(&ask, (to, PORT));
    }
    net::collect(&socket, wait)
        .into_iter()
        .filter_map(|(address, data)| device_of(address, &decrypt(&data)))
        .collect()
}

/// A `get_sysinfo` answer as a device: a plug, or a bulb.
fn device_of(address: Ipv4Addr, answer: &str) -> Option<Device> {
    let v = json::parse(answer).ok()?;
    let info = v.path("system.get_sysinfo")?;
    let kind =
        format!("{}{}", info.str_or("type", ""), info.str_or("mic_type", "")).to_ascii_uppercase();
    let brand = if kind.contains("PLUG") {
        Brand::KasaPlug
    } else if kind.contains("BULB") {
        Brand::KasaBulb
    } else {
        return None;
    };
    let id = info.str_or("deviceId", "").to_string();
    if id.is_empty() {
        return None;
    }
    let name = match info.str_or("alias", "") {
        "" => format!("Kasa {}", info.str_or("model", ""))
            .trim()
            .to_string(),
        alias => alias.to_string(),
    };
    Some(Device {
        brand,
        address,
        id,
        name,
        plug: brand == Brand::KasaPlug,
    })
}

/// The command for a state: a plug's relay, or a bulb's light state.
pub fn request(plug: bool, state: State) -> String {
    if plug {
        let relay = json::obj([("state", Value::Num(f64::from(u8::from(state.on))))]);
        return json::obj([("system", json::obj([("set_relay_state", relay)]))]).to_json();
    }
    let mut light = vec![
        (
            "on_off".to_string(),
            Value::Num(f64::from(u8::from(state.on))),
        ),
        ("transition_period".to_string(), Value::Num(300.0)),
    ];
    if state.on {
        if let Some(rgb) = state.color {
            let (hue, saturation, _) = super::to_hsv(rgb);
            light.push(("hue".to_string(), Value::Num(hue.round())));
            light.push((
                "saturation".to_string(),
                Value::Num((saturation * 100.0).round()),
            ));
            light.push(("color_temp".to_string(), Value::Num(0.0)));
        }
        if state.color.is_some() || state.brightness.is_some() {
            light.push((
                "brightness".to_string(),
                Value::Num(f64::from(super::percent(&state))),
            ));
        }
        light.push(("ignore_default".to_string(), Value::Num(1.0)));
    }
    json::obj([(
        LIGHT_SERVICE,
        json::obj([("transition_light_state", Value::Obj(light))]),
    )])
    .to_json()
}

pub fn set(to: SocketAddr, plug: bool, state: State) -> Result<(), Failure> {
    let answer = call(to, &request(plug, state))?;
    let v = json::parse(&answer).unwrap_or(Value::Null);
    // Every module answers with err_code 0 when it did it.
    let code = |path: &str| v.path(path).and_then(Value::as_f64);
    let result = if plug {
        code("system.set_relay_state.err_code")
    } else {
        code(&format!("{LIGHT_SERVICE}.transition_light_state.err_code"))
    };
    match result {
        Some(0.0) => Ok(()),
        _ => Err(Failure::Refused(
            "the Kasa device didn't take it".to_string(),
        )),
    }
}

/// One request over TCP: length-prefixed both ways.
fn call(to: SocketAddr, plain: &str) -> Result<String, Failure> {
    let lost = |e: std::io::Error| {
        Failure::Unreachable(format!(
            "the Kasa device at {} didn't answer ({e})",
            to.ip()
        ))
    };
    let mut stream = TcpStream::connect_timeout(&to, TIMEOUT).map_err(lost)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(lost)?;
    let body = encrypt(plain);
    let mut message = (body.len() as u32).to_be_bytes().to_vec();
    message.extend_from_slice(&body);
    stream.write_all(&message).map_err(lost)?;
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).map_err(lost)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_ANSWER {
        return Err(Failure::Refused(
            "the Kasa device sent too much".to_string(),
        ));
    }
    let mut answer = vec![0u8; length];
    stream.read_exact(&mut answer).map_err(lost)?;
    Ok(decrypt(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The bytes python-kasa's XorEncryption makes of the discovery query.
    #[test]
    fn xor_matches_python_kasa() {
        let cipher = encrypt(SYSINFO);
        let hex: String = cipher.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "d0f281f88bff9af7d5ef94b6d1b4c09fec95e68fe187e8caf08bf68bf6"
        );
        assert_eq!(decrypt(&cipher), SYSINFO);
    }

    #[test]
    fn sysinfo_tells_plugs_from_bulbs() {
        let plug = r#"{"system":{"get_sysinfo":{"alias":"Desk lamp","type":"IOT.SMARTPLUGSWITCH","deviceId":"8006AB","model":"HS103(US)","relay_state":0}}}"#;
        let d = device_of(Ipv4Addr::new(192, 168, 1, 5), plug).unwrap();
        assert_eq!(
            (d.brand, d.name.as_str(), d.plug),
            (Brand::KasaPlug, "Desk lamp", true)
        );
        let bulb = r#"{"system":{"get_sysinfo":{"alias":"","mic_type":"IOT.SMARTBULB","deviceId":"80121C","model":"KL130(US)","is_color":1}}}"#;
        let d = device_of(Ipv4Addr::new(192, 168, 1, 6), bulb).unwrap();
        assert_eq!(
            (d.brand, d.name.as_str()),
            (Brand::KasaBulb, "Kasa KL130(US)")
        );
        assert!(device_of(
            d.address,
            r#"{"system":{"get_sysinfo":{"type":"IOT.SMARTCAM","deviceId":"x"}}}"#
        )
        .is_none());
    }

    #[test]
    fn a_plug_switches_over_tcp_with_its_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        let fake = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut length = [0u8; 4];
            stream.read_exact(&mut length).unwrap();
            let mut body = vec![0u8; u32::from_be_bytes(length) as usize];
            stream.read_exact(&mut body).unwrap();
            let asked = decrypt(&body);
            let answer = encrypt(r#"{"system":{"set_relay_state":{"err_code":0}}}"#);
            stream
                .write_all(&(answer.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&answer).unwrap();
            asked
        });
        let on = State {
            on: true,
            color: None,
            brightness: None,
        };
        assert_eq!(set(at, true, on), Ok(()));
        assert_eq!(
            fake.join().unwrap(),
            r#"{"system":{"set_relay_state":{"state":1}}}"#
        );
    }

    #[test]
    fn a_bulb_color_is_hue_saturation_brightness() {
        let red = State {
            on: true,
            color: Some([255, 0, 0]),
            brightness: Some(70),
        };
        assert_eq!(
            request(false, red),
            r#"{"smartlife.iot.smartbulb.lightingservice":{"transition_light_state":{"on_off":1,"transition_period":300,"hue":0,"saturation":100,"color_temp":0,"brightness":70,"ignore_default":1}}}"#
        );
    }
}
