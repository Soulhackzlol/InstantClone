//! WiZ (Philips WiZ, and the many brands built on it): JSON over UDP on
//! port 38899, as pywizlight does. A scan broadcasts the app's
//! "registration" question; every bulb and plug answers with its MAC.

use super::{net, Brand, Device, Failure, State};
use crate::json::{self, Value};
use std::net::SocketAddr;
use std::time::Duration;

pub const PORT: u16 = 38899;
/// pywizlight's discovery message: a phone registering, which every WiZ
/// device answers. `register:false` keeps it from actually registering.
const REGISTRATION: &[u8] = br#"{"method":"registration","params":{"phoneMac":"AAAAAAAAAAAA","register":false,"phoneIp":"1.2.3.4","id":"1"}}"#;
const TRIES: usize = 4;
const EACH: Duration = Duration::from_millis(500);

pub fn scan(wait: Duration) -> Vec<Device> {
    let Some(socket) = net::scan_socket() else {
        return Vec::new();
    };
    for to in net::broadcast_addresses() {
        let _ = socket.send_to(REGISTRATION, (to, PORT));
    }
    let answers = net::collect(&socket, wait);
    let mut found: Vec<Device> = answers
        .into_iter()
        .filter_map(|(address, data)| {
            let mac = mac_of(&data)?;
            Some(Device {
                brand: Brand::Wiz,
                address,
                name: format!("WiZ ··{}", net::tail(&mac)),
                id: mac,
                plug: false,
            })
        })
        .collect();
    // Each answers every broadcast address: ask each device once.
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    // Which are plugs: their module name says "SOCKET".
    for device in &mut found {
        device.plug =
            module_name((device.address, PORT).into()).is_some_and(|m| m.contains("SOCKET"));
    }
    found
}

fn mac_of(data: &[u8]) -> Option<String> {
    let v = json::parse(&String::from_utf8_lossy(data)).ok()?;
    let mac = v.path("result.mac").and_then(Value::as_str)?;
    (!mac.is_empty()).then(|| mac.to_string())
}

fn module_name(to: SocketAddr) -> Option<String> {
    let ask = br#"{"method":"getSystemConfig","params":{}}"#;
    let answer = net::request(to, ask, 2, Duration::from_millis(300), |r| {
        String::from_utf8_lossy(r).contains("getSystemConfig")
    })
    .ok()?;
    let v = json::parse(&String::from_utf8_lossy(&answer)).ok()?;
    v.path("result.moduleName")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `setPilot`, as the WiZ app sends it. Dimming below 10 isn't allowed.
pub fn message(state: State) -> String {
    let mut params = vec![("state".to_string(), Value::Bool(state.on))];
    if state.on {
        if let Some([r, g, b]) = state.color {
            for (name, c) in [("r", r), ("g", g), ("b", b)] {
                params.push((name.to_string(), Value::Num(f64::from(c))));
            }
        }
        if state.color.is_some() || state.brightness.is_some() {
            let dimming = super::percent(&state).clamp(10, 100);
            params.push(("dimming".to_string(), Value::Num(f64::from(dimming))));
        }
    }
    json::obj([
        ("method", json::str("setPilot")),
        ("params", Value::Obj(params)),
    ])
    .to_json()
}

pub fn set(to: SocketAddr, state: State) -> Result<(), Failure> {
    let answer = net::request(to, message(state).as_bytes(), TRIES, EACH, |r| {
        String::from_utf8_lossy(r).contains("setPilot")
    })?;
    let v = json::parse(&String::from_utf8_lossy(&answer)).unwrap_or(Value::Null);
    match v.path("error.message").and_then(Value::as_str) {
        Some(why) => Err(Failure::Refused(format!("the WiZ light said no: {why}"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    #[test]
    fn colors_go_out_as_the_app_sends_them() {
        let red = State {
            on: true,
            color: Some([255, 0, 0]),
            // Under WiZ's 10% floor: it goes out as 10.
            brightness: Some(5),
        };
        assert_eq!(
            message(red),
            r#"{"method":"setPilot","params":{"state":true,"r":255,"g":0,"b":0,"dimming":10}}"#
        );
        let off = State {
            on: false,
            color: None,
            brightness: None,
        };
        assert_eq!(
            message(off),
            r#"{"method":"setPilot","params":{"state":false}}"#
        );
    }

    #[test]
    fn a_bulb_answering_success_is_set() {
        let bulb = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = bulb.local_addr().unwrap();
        let fake = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = bulb.recv_from(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("\"state\":true"));
            bulb.send_to(
                br#"{"method":"setPilot","env":"pro","result":{"success":true}}"#,
                from,
            )
            .unwrap();
        });
        let on = State {
            on: true,
            color: None,
            brightness: None,
        };
        assert_eq!(set(at, on), Ok(()));
        fake.join().unwrap();
    }

    #[test]
    fn discovery_answers_give_the_mac() {
        let answer = br#"{"method":"registration","env":"pro","result":{"mac":"a8bb50a4f94d","success":true}}"#;
        assert_eq!(mac_of(answer).as_deref(), Some("a8bb50a4f94d"));
        assert_eq!(mac_of(b"{}"), None);
    }
}
