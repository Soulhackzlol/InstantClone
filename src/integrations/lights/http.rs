//! Shelly relays and plugs, and WLED lights: both answer plain HTTP on the
//! local network, as aioshelly and python-wled talk to them.
//!
//! Neither answers a broadcast, so a scan knocks on port 80 of every
//! address on this PC's network (a /24) and asks whatever answers what it
//! is: Shelly at `/shelly`, WLED at `/json/info`.

use super::{net, Brand, Device, Failure, State};
use crate::json::{self, Value};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

const KNOCK: Duration = Duration::from_millis(300);
const ASK: Duration = Duration::from_millis(1500);
const COMMAND: Duration = Duration::from_secs(3);
/// How many addresses are knocked on at once.
const WORKERS: usize = 32;
const MAX_ANSWER: u64 = 64 * 1024;

pub fn scan() -> Vec<Device> {
    let Some(me) = net::local_ipv4() else {
        return Vec::new();
    };
    let [a, b, c, _] = me.octets();
    let hosts: Vec<Ipv4Addr> = (1..=254u8)
        .map(|d| Ipv4Addr::new(a, b, c, d))
        .filter(|ip| *ip != me)
        .collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = hosts
            .chunks(hosts.len().div_ceil(WORKERS))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|ip| identify(*ip))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    })
}

/// What answers on port 80 at `ip`, if it's a Shelly or a WLED.
fn identify(ip: Ipv4Addr) -> Option<Device> {
    TcpStream::connect_timeout(&SocketAddr::from((ip, 80)), KNOCK).ok()?;
    if let Some(v) = get(ip, "/shelly", ASK) {
        return shelly_of(ip, &v);
    }
    wled_of(ip, &get(ip, "/json/info", ASK)?)
}

/// The agent for a device on this network: plain HTTP, giving up after
/// `timeout`, with every status handed back as Ok for `answered` to read.
/// It never follows a redirect, so a device can only answer for itself,
/// never send the call on to another address.
fn lan_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn get(ip: Ipv4Addr, path: &str, timeout: Duration) -> Option<Value> {
    let mut resp = lan_agent(timeout)
        .get(&format!("http://{ip}{path}"))
        .call()
        .ok()?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER)
        .read_to_string()
        .ok()?;
    json::parse(&text).ok()
}

/// `/shelly`: generation 2 and later say `gen`; generation 1 says `type`.
fn shelly_of(ip: Ipv4Addr, v: &Value) -> Option<Device> {
    let mac = v.str_or("mac", "").to_string();
    if mac.is_empty() {
        return None;
    }
    let gen2 = v
        .get("gen")
        .and_then(Value::as_f64)
        .is_some_and(|g| g >= 2.0);
    let model = if gen2 {
        v.str_or("app", "")
    } else {
        v.str_or("type", "")
    };
    if !gen2 && model.is_empty() {
        return None;
    }
    let name = match v.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("Shelly {model} ··{}", net::tail(&mac)),
    };
    let brand = if gen2 { Brand::Shelly2 } else { Brand::Shelly1 };
    Some(Device {
        brand,
        address: ip,
        id: mac,
        name,
        plug: true,
    })
}

/// `/json/info`: WLED says so in `brand`.
fn wled_of(ip: Ipv4Addr, v: &Value) -> Option<Device> {
    if v.str_or("brand", "") != "WLED" {
        return None;
    }
    let mac = v.str_or("mac", "").to_string();
    if mac.is_empty() {
        return None;
    }
    let name = match v.str_or("name", "") {
        "" => format!("WLED ··{}", net::tail(&mac)),
        name => name.to_string(),
    };
    Some(Device {
        brand: Brand::Wled,
        address: ip,
        id: mac,
        name,
        plug: false,
    })
}

/// What the Shelly switch call is for this generation.
pub fn shelly_path(generation: u8, on: bool) -> String {
    if generation >= 2 {
        format!("/rpc/Switch.Set?id=0&on={on}")
    } else {
        format!("/relay/0?turn={}", if on { "on" } else { "off" })
    }
}

pub fn shelly(ip: Ipv4Addr, generation: u8, on: bool) -> Result<(), Failure> {
    let url = format!("http://{ip}{}", shelly_path(generation, on));
    let result = lan_agent(COMMAND).get(&url).call();
    answered(result, "Shelly", ip)
}

/// WLED's state: on, brightness (0 to 255) and segment 0's color, solid.
pub fn wled_body(state: State) -> String {
    let mut body = vec![("on".to_string(), Value::Bool(state.on))];
    if state.on {
        if state.color.is_some() || state.brightness.is_some() {
            // Percent to 0-255, rounded in whole numbers (50% is 128).
            let bri = (u32::from(super::percent(&state)) * 255 + 50) / 100;
            body.push(("bri".to_string(), Value::Num(f64::from(bri))));
        }
        if let Some(rgb) = state.color {
            let col = Value::Arr(vec![Value::Arr(
                rgb.iter().map(|c| Value::Num(f64::from(*c))).collect(),
            )]);
            let segment = json::obj([
                ("id", Value::Num(0.0)),
                ("col", col),
                ("fx", Value::Num(0.0)),
            ]);
            body.push(("seg".to_string(), Value::Arr(vec![segment])));
        }
    }
    Value::Obj(body).to_json()
}

pub fn wled(ip: Ipv4Addr, state: State) -> Result<(), Failure> {
    let result = lan_agent(COMMAND)
        .post(&format!("http://{ip}/json/state"))
        .header("Content-Type", "application/json")
        .send(wled_body(state));
    answered(result, "WLED", ip)
}

fn answered(
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    what: &str,
    ip: Ipv4Addr,
) -> Result<(), Failure> {
    // The agent hands back every status as Ok: read it here.
    match result.map(|resp| resp.status().as_u16()) {
        Ok(200..=299) => Ok(()),
        Ok(401) => Err(Failure::Refused(format!(
            "the {what} at {ip} has a password. Turn it off in its app or web page to use it here"
        ))),
        Ok(code) => Err(Failure::Refused(format!(
            "the {what} at {ip} said no (HTTP {code})"
        ))),
        Err(e) => Err(Failure::Unreachable(format!(
            "the {what} at {ip} didn't answer ({e})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelly_generations_are_told_apart() {
        let ip = Ipv4Addr::new(192, 168, 1, 40);
        let gen2 = json::parse(r#"{"name":"Ring light","id":"shellyplugsg3-abcdef","mac":"ABCDEF123456","model":"S3PL-00112EU","gen":3,"app":"PlugSG3","auth_en":false}"#).unwrap();
        let d = shelly_of(ip, &gen2).unwrap();
        assert_eq!(
            (d.brand, d.name.as_str(), d.plug),
            (Brand::Shelly2, "Ring light", true)
        );
        let gen1 =
            json::parse(r#"{"type":"SHPLG-S","mac":"A4CF12F45678","auth":false,"fw":"20230913"}"#)
                .unwrap();
        let d = shelly_of(ip, &gen1).unwrap();
        assert_eq!(
            (d.brand, d.name.as_str()),
            (Brand::Shelly1, "Shelly SHPLG-S ··5678")
        );
        assert_eq!(shelly_path(1, true), "/relay/0?turn=on");
        assert_eq!(shelly_path(3, false), "/rpc/Switch.Set?id=0&on=false");
    }

    #[test]
    fn wled_is_known_by_its_brand_and_set_by_segment() {
        let ip = Ipv4Addr::new(192, 168, 1, 41);
        let info = json::parse(
            r#"{"ver":"0.14.0","name":"Desk strip","brand":"WLED","mac":"3c71bf0a1b2c"}"#,
        )
        .unwrap();
        assert_eq!(wled_of(ip, &info).unwrap().name, "Desk strip");
        assert!(wled_of(ip, &json::parse(r#"{"brand":"Other","mac":"x"}"#).unwrap()).is_none());
        let red = State {
            on: true,
            color: Some([255, 0, 0]),
            brightness: Some(50),
        };
        assert_eq!(
            wled_body(red),
            r#"{"on":true,"bri":128,"seg":[{"id":0,"col":[[255,0,0]],"fx":0}]}"#
        );
        assert_eq!(
            wled_body(State {
                on: false,
                color: None,
                brightness: None
            }),
            r#"{"on":false}"#
        );
    }

    /// The agent hands every status back as Ok: a device answering 401 or
    /// 500 must still read as refused, not as switched. A redirect is an
    /// answer too, never followed to the address it names.
    #[test]
    fn a_device_saying_no_is_not_reported_as_done() {
        use std::io::{Read, Write};
        let answer = |status: &'static str| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                // Followed, the redirect would meet a closed port instead.
                let reply = format!(
                    "HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:9/\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                sock.write_all(reply.as_bytes()).unwrap();
            });
            let result = lan_agent(COMMAND)
                .get(&format!("http://127.0.0.1:{port}/relay/0?turn=on"))
                .call();
            answered(result, "Shelly", Ipv4Addr::LOCALHOST)
        };
        assert_eq!(answer("200 OK"), Ok(()));
        assert!(
            matches!(answer("401 Unauthorized"), Err(Failure::Refused(why)) if why.contains("password")),
            "a password-protected device"
        );
        assert!(
            matches!(answer("500 Internal Server Error"), Err(Failure::Refused(why)) if why.contains("HTTP 500")),
            "a device error"
        );
        assert!(
            matches!(answer("302 Found"), Err(Failure::Refused(why)) if why.contains("HTTP 302")),
            "a redirect, not followed"
        );
    }
}
