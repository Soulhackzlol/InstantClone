//! Yeelight, as python-yeelight drives it. Each light needs "LAN Control"
//! switched on in the Yeelight app first.
//!
//! A scan is an SSDP-style search on the multicast group, port 1982;
//! lights answer with headers (`Location: yeelight://ip:55443`, `id`,
//! `name`, `model`). Commands are JSON lines over TCP on port 55443.

use super::{net, Brand, Device, Failure, State};
use crate::json::{self, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub const PORT: u16 = 55443;
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SEARCH_PORT: u16 = 1982;
const SEARCH: &[u8] =
    b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n";
const TIMEOUT: Duration = Duration::from_secs(3);
/// The longest line read from a light. Real answers and state notices are
/// well under 1 KB; the cap keeps a device that never ends a line from
/// filling memory.
const MAX_LINE: u64 = 16 * 1024;
/// Changes fade in this long, as the app does.
const FADE_MS: f64 = 300.0;

pub fn scan(wait: Duration) -> Vec<Device> {
    let Some(socket) = net::scan_socket() else {
        return Vec::new();
    };
    let _ = socket.send_to(SEARCH, (GROUP, SEARCH_PORT));
    net::collect(&socket, wait)
        .into_iter()
        .filter_map(|(from, data)| device_of(from, &String::from_utf8_lossy(&data)))
        .collect()
}

/// A search answer's headers, as a device.
fn device_of(from: Ipv4Addr, answer: &str) -> Option<Device> {
    let header = |name: &str| {
        answer.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    };
    let location = header("Location")?;
    let address = location
        .strip_prefix("yeelight://")
        .and_then(|rest| rest.split(':').next())
        .and_then(|ip| ip.parse().ok())
        .unwrap_or(from);
    let id = header("id").filter(|id| !id.is_empty())?;
    let name = match header("name").unwrap_or_default() {
        name if !name.is_empty() => name,
        _ => format!(
            "Yeelight {} ··{}",
            header("model").unwrap_or_default(),
            net::tail(&id)
        )
        .replace("  ", " "),
    };
    Some(Device {
        brand: Brand::Yeelight,
        address,
        id,
        name,
        plug: false,
    })
}

/// The commands for a state: power first (a light that's off ignores a
/// color), then color, then brightness.
pub fn commands(state: State) -> Vec<(&'static str, Vec<Value>)> {
    let smooth = || [json::str("smooth"), Value::Num(FADE_MS)];
    let power = if state.on { "on" } else { "off" };
    let mut out = vec![(
        "set_power",
        [vec![json::str(power)], smooth().to_vec()].concat(),
    )];
    if !state.on {
        return out;
    }
    if let Some([r, g, b]) = state.color {
        let rgb = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        out.push((
            "set_rgb",
            [vec![Value::Num(f64::from(rgb))], smooth().to_vec()].concat(),
        ));
    }
    if state.color.is_some() || state.brightness.is_some() {
        let bright = Value::Num(f64::from(super::percent(&state)));
        out.push(("set_bright", [vec![bright], smooth().to_vec()].concat()));
    }
    out
}

pub fn set(to: SocketAddr, state: State) -> Result<(), Failure> {
    let stream = TcpStream::connect_timeout(&to, TIMEOUT).map_err(|e| {
        Failure::Unreachable(format!(
            "the Yeelight at {} didn't answer ({e}). Is LAN Control on in the Yeelight app?",
            to.ip()
        ))
    })?;
    let lost = |e: std::io::Error| Failure::Unreachable(format!("lost the Yeelight: {e}"));
    stream.set_read_timeout(Some(TIMEOUT)).map_err(lost)?;
    let mut writer = stream.try_clone().map_err(lost)?;
    let mut reader = BufReader::new(stream);
    for (id, (method, params)) in commands(state).into_iter().enumerate() {
        let id = id as f64 + 1.0;
        let line = json::obj([
            ("id", Value::Num(id)),
            ("method", json::str(method)),
            ("params", Value::Arr(params)),
        ])
        .to_json();
        writer
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(lost)?;
        // The light also sends its state as it changes: read on to the
        // answer to this command, for as long as a silent light would get.
        // A device that keeps talking but never answers is given up on too.
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(Failure::Unreachable(
                    "the Yeelight never answered the command".to_string(),
                ));
            }
            let mut answer = String::new();
            let line = (&mut reader).take(MAX_LINE).read_line(&mut answer);
            if line.map_err(lost)? == 0 {
                return Err(Failure::Unreachable(
                    "the Yeelight closed the connection".to_string(),
                ));
            }
            let Ok(v) = json::parse(answer.trim()) else {
                continue;
            };
            if v.get("id").and_then(Value::as_f64) != Some(id) {
                continue;
            }
            if let Some(why) = v.path("error.message").and_then(Value::as_str) {
                return Err(Failure::Refused(format!("the Yeelight said no: {why}")));
            }
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn search_answers_become_devices() {
        let answer = "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nLocation: yeelight://192.168.1.239:55443\r\nid: 0x000000000015243f\r\nmodel: color\r\nname: \r\npower: on\r\n";
        let d = device_of(Ipv4Addr::new(10, 0, 0, 1), answer).unwrap();
        assert_eq!(d.address, Ipv4Addr::new(192, 168, 1, 239));
        assert_eq!(d.id, "0x000000000015243f");
        assert_eq!(d.name, "Yeelight color ··243f");
        assert!(device_of(d.address, "HTTP/1.1 200 OK\r\n").is_none());
    }

    #[test]
    fn a_color_powers_on_first_and_waits_for_each_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        let fake = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut seen = Vec::new();
            for id in 1..=3 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                seen.push(line.trim().to_string());
                // A state notice first, then the answer.
                writer
                    .write_all(b"{\"method\":\"props\",\"params\":{\"power\":\"on\"}}\r\n")
                    .unwrap();
                writer
                    .write_all(format!("{{\"id\":{id},\"result\":[\"ok\"]}}\r\n").as_bytes())
                    .unwrap();
            }
            seen
        });
        let state = State {
            on: true,
            color: Some([255, 0, 0]),
            brightness: Some(80),
        };
        assert_eq!(set(at, state), Ok(()));
        let seen = fake.join().unwrap();
        assert_eq!(
            seen[0],
            r#"{"id":1,"method":"set_power","params":["on","smooth",300]}"#
        );
        assert_eq!(
            seen[1],
            r#"{"id":2,"method":"set_rgb","params":[16711680,"smooth",300]}"#
        );
        assert_eq!(
            seen[2],
            r#"{"id":3,"method":"set_bright","params":[80,"smooth",300]}"#
        );
    }

    /// A device that chatters without ever answering, in lines that never
    /// end, used to hold the light's thread for as long as it kept talking.
    #[test]
    fn a_device_that_never_answers_is_given_up_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Until the app hangs up: a state notice, then an endless line.
            while stream.write_all(b"{\"method\":\"props\"}\r\n").is_ok()
                && stream.write_all(&[b'x'; 4096]).is_ok()
            {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let off = State {
            on: false,
            color: None,
            brightness: None,
        };
        let started = Instant::now();
        let result = set(at, off);
        assert!(matches!(result, Err(Failure::Unreachable(_))), "{result:?}");
        assert!(started.elapsed() < TIMEOUT * 2, "{:?}", started.elapsed());
    }
}
