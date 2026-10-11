//! Smart lights and plugs on the local network, found and driven with no
//! hub, account or cloud: WiZ, LIFX, Govee (with LAN Control on), Yeelight
//! (with LAN Control on), older TP-Link Kasa models, Shelly and WLED.
//!
//! Each brand is reached the way the open-source library Home Assistant
//! uses for it does: pywizlight, aiolifx, govee-local-api, python-yeelight,
//! python-kasa, aioshelly and python-wled.
//!
//! A step saves a device as a reference, `wiz:192.168.1.20#a8bb50a4f94d`:
//! the brand, the address it was last seen at, and its own id. A router can
//! hand a device a new address; when one stops answering, its brand is
//! scanned again for that id and the new address is used, and remembered
//! for this run.
//!
//! Everything here blocks: callers run it off the async threads.

mod govee;
mod http;
mod kasa;
mod lifx;
mod net;
mod wiz;
mod yeelight;

use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::Duration;

/// How long a scan listens for answers.
pub const SCAN_WAIT: Duration = Duration::from_millis(2500);
/// A rescan for a device that moved: shorter, it only needs that one.
const FIND_WAIT: Duration = Duration::from_millis(1800);
/// Flashing: how many times, and how long each on and off lasts.
const FLASHES: usize = 3;
const FLASH_STEP: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Brand {
    Wiz,
    Lifx,
    Govee,
    Yeelight,
    KasaPlug,
    KasaBulb,
    Shelly1,
    Shelly2,
    Wled,
}

const BRANDS: [Brand; 9] = [
    Brand::Wiz,
    Brand::Lifx,
    Brand::Govee,
    Brand::Yeelight,
    Brand::KasaPlug,
    Brand::KasaBulb,
    Brand::Shelly1,
    Brand::Shelly2,
    Brand::Wled,
];

impl Brand {
    fn id(self) -> &'static str {
        match self {
            Brand::Wiz => "wiz",
            Brand::Lifx => "lifx",
            Brand::Govee => "govee",
            Brand::Yeelight => "yeelight",
            Brand::KasaPlug => "kasa_plug",
            Brand::KasaBulb => "kasa_bulb",
            Brand::Shelly1 => "shelly1",
            Brand::Shelly2 => "shelly2",
            Brand::Wled => "wled",
        }
    }

    fn from_id(id: &str) -> Option<Brand> {
        BRANDS.into_iter().find(|b| b.id() == id)
    }

    fn label(self) -> &'static str {
        match self {
            Brand::Wiz => "WiZ",
            Brand::Lifx => "LIFX",
            Brand::Govee => "Govee",
            Brand::Yeelight => "Yeelight",
            Brand::KasaPlug | Brand::KasaBulb => "TP-Link Kasa",
            Brand::Shelly1 | Brand::Shelly2 => "Shelly",
            Brand::Wled => "WLED",
        }
    }

    /// Which scan finds this brand: one per way of finding devices.
    fn family(self) -> Family {
        match self {
            Brand::Wiz => Family::Wiz,
            Brand::Lifx => Family::Lifx,
            Brand::Govee => Family::Govee,
            Brand::Yeelight => Family::Yeelight,
            Brand::KasaPlug | Brand::KasaBulb => Family::Kasa,
            Brand::Shelly1 | Brand::Shelly2 | Brand::Wled => Family::Http,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    Wiz,
    Lifx,
    Govee,
    Yeelight,
    Kasa,
    Http,
}

const FAMILIES: [Family; 6] = [
    Family::Wiz,
    Family::Lifx,
    Family::Govee,
    Family::Yeelight,
    Family::Kasa,
    Family::Http,
];

/// A light or plug a scan found.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub brand: Brand,
    pub address: Ipv4Addr,
    /// The device's own id (a MAC address, mostly): it stays when the
    /// address changes.
    pub id: String,
    pub name: String,
    /// Only on and off, no color.
    pub plug: bool,
}

impl Device {
    pub fn reference(&self) -> String {
        format!("{}:{}#{}", self.brand.id(), self.address, self.id)
    }

    pub fn to_json(&self) -> Value {
        json::obj([
            ("ref", json::str(self.reference())),
            ("name", json::str(&self.name)),
            ("brand", json::str(self.brand.label())),
            ("address", json::str(self.address.to_string())),
            ("plug", Value::Bool(self.plug)),
        ])
    }
}

/// A saved reference, read back: brand, last address, id.
pub fn parse_ref(reference: &str) -> Option<(Brand, Ipv4Addr, String)> {
    let (brand, rest) = reference.trim().split_once(':')?;
    let (address, id) = rest.split_once('#')?;
    let id = id.trim();
    (!id.is_empty()).then_some(())?;
    Some((
        Brand::from_id(brand)?,
        address.parse().ok()?,
        id.to_string(),
    ))
}

/// What one command sets a device to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub on: bool,
    pub color: Option<[u8; 3]>,
    /// Percent, 1 to 100.
    pub brightness: Option<u8>,
}

/// What a step asks a device to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    On {
        brightness: Option<u8>,
    },
    Off,
    Color {
        rgb: [u8; 3],
        brightness: Option<u8>,
    },
    /// Blink the color a few times, then stay on in it.
    Flash {
        rgb: [u8; 3],
        brightness: Option<u8>,
    },
}

/// A color as people write it: `#ff2020`, `ff2020`, `#f22` or a name.
pub fn parse_color(text: &str) -> Option<[u8; 3]> {
    let text = text.trim().to_ascii_lowercase();
    let named = match text.as_str() {
        "red" => Some([255, 32, 32]),
        "orange" => Some([255, 110, 0]),
        "amber" => Some([255, 160, 0]),
        "yellow" => Some([255, 220, 0]),
        "green" => Some([0, 220, 70]),
        "cyan" => Some([0, 210, 255]),
        "blue" => Some([30, 80, 255]),
        "purple" => Some([150, 60, 255]),
        "pink" => Some([255, 70, 170]),
        "white" => Some([255, 255, 255]),
        "warm" | "warm white" => Some([255, 190, 120]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let hex = text.strip_prefix('#').unwrap_or(&text);
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
    match hex.len() {
        6 => {
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            Some([byte(0)?, byte(2)?, byte(4)?])
        }
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        _ => None,
    }
}

/// Why a command didn't work.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// Nothing answered at that address: the device may have moved.
    Unreachable(String),
    /// The device answered, and said no.
    Refused(String),
}

impl Failure {
    fn text(self) -> String {
        match self {
            Failure::Unreachable(why) | Failure::Refused(why) => why,
        }
    }
}

/// Where devices were last found this run, by id, when that isn't where
/// their step says.
static MOVED: Mutex<BTreeMap<String, Ipv4Addr>> = Mutex::new(BTreeMap::new());

/// Look for every supported device on the network, all brands at once.
pub fn scan(wait: Duration) -> Vec<Device> {
    let mut found: Vec<Device> = std::thread::scope(|scope| {
        let scans: Vec<_> = FAMILIES
            .into_iter()
            .map(|family| scope.spawn(move || scan_family(family, wait)))
            .collect();
        scans
            .into_iter()
            .flat_map(|scan| scan.join().unwrap_or_default())
            .collect()
    });
    found.sort_by(|a, b| (a.brand, &a.name, a.address).cmp(&(b.brand, &b.name, b.address)));
    found.dedup_by(|a, b| a.brand == b.brand && a.id == b.id);
    {
        let mut moved = MOVED.lock();
        for device in &found {
            moved.insert(device.id.clone(), device.address);
        }
    }
    found
}

fn scan_family(family: Family, wait: Duration) -> Vec<Device> {
    match family {
        Family::Wiz => wiz::scan(wait),
        Family::Lifx => lifx::scan(wait),
        Family::Govee => govee::scan(wait),
        Family::Yeelight => yeelight::scan(wait),
        Family::Kasa => kasa::scan(wait),
        Family::Http => http::scan(),
    }
}

/// Do what `command` says to the device a step saved. Returns a short
/// note when it had to find the device at a new address.
pub fn send(reference: &str, command: Command) -> Result<Option<String>, String> {
    let (brand, saved, id) = parse_ref(reference).ok_or("pick the light or plug again")?;
    let address = MOVED.lock().get(&id).copied().unwrap_or(saved);
    match run(brand, address, &id, command) {
        Ok(()) => Ok(None),
        Err(Failure::Refused(why)) => Err(why),
        Err(Failure::Unreachable(why)) => {
            // Moved? Scan its brand for the same id and try there.
            let found = scan_family(brand.family(), FIND_WAIT)
                .into_iter()
                .find(|d| d.brand == brand && d.id == id);
            let Some(device) = found else {
                return Err(format!(
                    "{why}. A scan didn't find it either: is it on, and on the same network as this PC?"
                ));
            };
            MOVED.lock().insert(id.clone(), device.address);
            run(brand, device.address, &id, command).map_err(Failure::text)?;
            Ok(Some(format!("found it at {}", device.address)))
        }
    }
}

fn run(brand: Brand, address: Ipv4Addr, id: &str, command: Command) -> Result<(), Failure> {
    let plug = matches!(brand, Brand::KasaPlug | Brand::Shelly1 | Brand::Shelly2);
    match command {
        Command::Off => set(
            brand,
            address,
            id,
            State {
                on: false,
                color: None,
                brightness: None,
            },
        ),
        Command::On { brightness } => set(
            brand,
            address,
            id,
            State {
                on: true,
                color: None,
                brightness,
            },
        ),
        Command::Color { rgb, brightness } => {
            let color = (!plug).then_some(rgb);
            set(
                brand,
                address,
                id,
                State {
                    on: true,
                    color,
                    brightness,
                },
            )
        }
        // A plug isn't flashed: switching what's plugged in on and off
        // fast can harm it. It just turns on.
        Command::Flash { brightness, .. } if plug => set(
            brand,
            address,
            id,
            State {
                on: true,
                color: None,
                brightness,
            },
        ),
        Command::Flash { rgb, brightness } => {
            let lit = State {
                on: true,
                color: Some(rgb),
                brightness,
            };
            let off = State {
                on: false,
                color: None,
                brightness: None,
            };
            let blink = || -> Result<(), Failure> {
                for _ in 0..FLASHES {
                    set(brand, address, id, lit)?;
                    std::thread::sleep(FLASH_STEP);
                    set(brand, address, id, off)?;
                    std::thread::sleep(FLASH_STEP);
                }
                Ok(())
            };
            // A flash cut short still ends lit: a failure between "off" and
            // "on" used to leave the light dark (an on-air light, say).
            let flashed = blink();
            let lit_again = set(brand, address, id, lit);
            flashed.and(lit_again)
        }
    }
}

fn set(brand: Brand, address: Ipv4Addr, id: &str, state: State) -> Result<(), Failure> {
    match brand {
        Brand::Wiz => wiz::set((address, wiz::PORT).into(), state),
        Brand::Lifx => lifx::set((address, lifx::PORT).into(), id, state),
        Brand::Govee => govee::set((address, govee::COMMAND_PORT).into(), state),
        Brand::Yeelight => yeelight::set((address, yeelight::PORT).into(), state),
        Brand::KasaPlug => kasa::set((address, kasa::PORT).into(), true, state),
        Brand::KasaBulb => kasa::set((address, kasa::PORT).into(), false, state),
        Brand::Shelly1 => http::shelly(address, 1, state.on),
        Brand::Shelly2 => http::shelly(address, 2, state.on),
        Brand::Wled => http::wled(address, state),
    }
}

/// Red, green and blue as hue (degrees), saturation and value (0 to 1).
fn to_hsv(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// The brightness a state asks for, in percent, or what its color says.
fn percent(state: &State) -> u8 {
    state.brightness.unwrap_or_else(|| {
        state
            .color
            .map_or(100, |rgb| (to_hsv(rgb).2 * 100.0).round() as u8)
            .max(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_round_trip() {
        let d = Device {
            brand: Brand::KasaPlug,
            address: Ipv4Addr::new(192, 168, 1, 20),
            id: "8006ABCD".into(),
            name: "Desk".into(),
            plug: true,
        };
        assert_eq!(d.reference(), "kasa_plug:192.168.1.20#8006ABCD");
        assert_eq!(
            parse_ref(&d.reference()),
            Some((Brand::KasaPlug, d.address, "8006ABCD".into()))
        );
        assert_eq!(parse_ref("wiz:not-an-ip#x"), None);
        assert_eq!(parse_ref("hue:192.168.1.2#x"), None, "unknown brand");
        assert_eq!(parse_ref("wiz:192.168.1.2#"), None, "no id");
    }

    #[test]
    fn colors_read_hex_short_hex_and_names() {
        assert_eq!(parse_color("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_color("ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_color("#f80"), Some([255, 136, 0]));
        assert_eq!(parse_color(" Red "), Some([255, 32, 32]));
        assert_eq!(parse_color("#ggg"), None);
        assert_eq!(parse_color("#ff80"), None);
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn hsv_matches_the_usual_conversion() {
        let close = |a: f64, b: f64| (a - b).abs() < 0.01;
        let (h, s, v) = to_hsv([255, 0, 0]);
        assert!(close(h, 0.0) && close(s, 1.0) && close(v, 1.0));
        let (h, ..) = to_hsv([0, 0, 255]);
        assert!(close(h, 240.0));
        let (h, s, v) = to_hsv([128, 128, 128]);
        assert!(close(h, 0.0) && close(s, 0.0) && close(v, 0.502));
        let (h, ..) = to_hsv([255, 0, 128]);
        assert!(close(h, 329.88));
    }

    #[test]
    fn brightness_is_asked_for_or_comes_from_the_color() {
        let state = |color, brightness| State {
            on: true,
            color,
            brightness,
        };
        assert_eq!(percent(&state(Some([255, 0, 0]), Some(40))), 40);
        assert_eq!(percent(&state(Some([128, 0, 0]), None)), 50);
        assert_eq!(percent(&state(None, None)), 100);
        assert_eq!(percent(&state(Some([0, 0, 0]), None)), 1, "never fully off");
    }
}
