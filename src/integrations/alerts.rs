//! What "Show on stream" steps put on the alerts browser source.
//!
//! The board keeps the last few alerts with an increasing id. The page in
//! OBS (`/alerts`) reads them over its own event stream and plays each new
//! id once, in order, so two alerts close together queue instead of
//! overlapping. Only what a step chose to show is here: it is on stream
//! anyway, which is why the page needs no login.
//!
//! How every card looks is the page's theme (`Look`): a ready-made style
//! with its own sound, then any of its parts changed, down to custom CSS.
//! It is sent along with the alerts, so a change shows in OBS at once.
//! Each alert can still bring its own accent color, sound and picture.

use super::alert_media::{self, Kind};
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The alerts browser source itself.
const PAGE: &str = include_str!("../../web/alerts.html");
/// Stands in the page for its own fingerprint (`page_id`).
const PAGE_ID_MARK: &str = "__ALERTS_PAGE_ID__";

/// Alerts kept for a page that (re)connects. Older ones are never replayed.
const KEEP: usize = 8;
/// How long an alert stays on the board: a page that reconnects after this
/// no longer plays it.
const FRESH: Duration = Duration::from_secs(30);
pub const MIN_SECONDS: u64 = 2;
pub const MAX_SECONDS: u64 = 30;
const MAX_TITLE: usize = 80;
const MAX_TEXT: usize = 300;
const MAX_LINK: usize = 500;
const MAX_CSS: usize = 20_000;
pub const DEFAULT_VOLUME: u8 = 70;
/// An alert's sound that plays the theme's.
pub const THEME_SOUND: &str = "theme";
/// The sounds the alerts page makes itself, with no file to find.
pub const SOUNDS: &[&str] = &[
    "chime", "pop", "bell", "rise", "coin", "sparkle", "whoosh", "fanfare", "zap", "blip",
];
/// The ready-made looks (web/alerts.html draws them).
pub const THEMES: &[&str] = &[
    "island", "signal", "banner", "bubble", "minimal", "paper", "cinema",
];
/// Fonts the page can load (from Google Fonts), or `system`.
pub const FONTS: &[&str] = &[
    "system",
    "Inter",
    "Poppins",
    "Montserrat",
    "Fredoka",
    "Rubik",
    "Bebas Neue",
    "Oswald",
    "Bangers",
    "Orbitron",
    "Press Start 2P",
    "Permanent Marker",
    "Roboto Mono",
    "Chakra Petch",
    "Playfair Display",
    "Special Elite",
];
pub const ANIMATIONS: &[&str] = &["slide", "pop", "fade", "zoom", "bounce", "drop"];

/// One alert as a "Show on stream" step (or the dashboard's preview)
/// puts it up.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub title: String,
    pub text: String,
    /// The viewer's own words (a cheer's, a resub's), or empty.
    pub message: String,
    pub seconds: u64,
    /// `#rrggbb`, or empty for the theme's accent.
    pub color: String,
    /// `theme`, one of `SOUNDS`, an upload (`media:...`), a web link to a
    /// sound file, or empty for none.
    pub sound: String,
    /// 0 to 100.
    pub volume: u8,
    /// An upload (`media:...`) or a web link to a picture or GIF, or empty.
    pub image: String,
    /// The alert design (an overlay made in the Studio) that shows it, by
    /// its file name; empty for the alerts page's theme.
    pub design: String,
}

impl Card {
    /// A card with just words, in the theme's colors, silent.
    pub fn plain(title: &str, text: &str, seconds: u64) -> Card {
        Card {
            title: title.to_string(),
            text: text.to_string(),
            message: String::new(),
            seconds,
            color: String::new(),
            sound: String::new(),
            volume: DEFAULT_VOLUME,
            image: String::new(),
            design: String::new(),
        }
    }

    /// What the page can safely show: lengths capped, one line each, and a
    /// color, sound or picture that isn't valid left out.
    fn cleaned(self) -> Card {
        Card {
            title: one_line(&self.title, MAX_TITLE),
            text: one_line(&self.text, MAX_TEXT),
            message: one_line(&self.message, MAX_TEXT),
            seconds: self.seconds.clamp(MIN_SECONDS, MAX_SECONDS),
            color: color_or_empty(&self.color),
            sound: if is_sound(&self.sound) {
                self.sound
            } else {
                String::new()
            },
            volume: self.volume.min(100),
            image: if is_picture(&self.image) {
                self.image
            } else {
                String::new()
            },
            design: if is_design(&self.design) {
                self.design
            } else {
                String::new()
            },
        }
    }
}

/// A fingerprint of the page this build serves. The page carries its own
/// and the feed says the current one: OBS keeps a browser source running
/// through an app update, and a page that sees they differ reloads itself
/// instead of drawing new looks with old code.
pub fn page_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        // FNV-1a: stable across builds of the same page, no dependency.
        let hash = PAGE.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        format!("{hash:016x}")
    })
}

/// The page as served, carrying its fingerprint.
pub fn page_html() -> String {
    PAGE.replace(PAGE_ID_MARK, page_id())
}

/// `#rrggbb`.
/// An overlay's file name, as the alerts page asks for it: letters,
/// numbers, `-` and `_` only.
pub fn is_design(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One of the ready-made designs (`alert-island`...), which every install
/// can make from its theme, unlike one a streamer made.
pub fn is_ready_design(name: &str) -> bool {
    name.strip_prefix("alert-")
        .is_some_and(|theme| THEMES.contains(&theme))
}

pub fn is_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn color_or_empty(text: &str) -> String {
    if is_color(text) {
        text.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// A plain `http(s)://` link the page can put in a `src` as it is.
pub fn is_web_link(text: &str) -> bool {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && text.len() <= MAX_LINK
            && !text
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || "\"'<>\\`{}".contains(c))
    })
}

/// Empty (no sound), the theme's, a built-in one, an upload, or a link to
/// a sound file.
pub fn is_sound(text: &str) -> bool {
    text.is_empty()
        || text == THEME_SOUND
        || SOUNDS.contains(&text)
        || alert_media::is_ref(text, Kind::Sound)
        || is_web_link(text)
}

/// An uploaded picture or a link to one.
pub fn is_picture(text: &str) -> bool {
    alert_media::is_ref(text, Kind::Picture) || is_web_link(text)
}

/// How every card on the alerts page looks: a theme, then any part of it
/// changed. Empty strings and `None` keep the theme's own.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// One of `THEMES`.
    pub theme: String,
    /// `bottom`, `top` or `middle`.
    pub position: String,
    /// `center`, `left` or `right`.
    pub align: String,
    /// Size in percent, 50 to 200.
    pub scale: u16,
    /// One of `FONTS`, or empty.
    pub font: String,
    /// Text, card background and accent: `#rrggbb` or empty.
    pub text: String,
    pub background: String,
    pub accent: String,
    /// The card background's opacity in percent.
    pub opacity: Option<u8>,
    /// Corner roundness in pixels.
    pub radius: Option<u8>,
    /// One of `ANIMATIONS`, or empty.
    pub animation: String,
    /// Where an alert's picture goes: `left`, `top`, or empty.
    pub picture: String,
    /// The sound alerts set to "The design's sound" play when their design
    /// has no theme of its own: one of `SOUNDS`, an upload, a link, or
    /// empty for the theme's own.
    pub sound: String,
    /// Extra CSS, put in the page as it is.
    pub css: String,
    /// The design (an overlay made in the Studio) an alert that names none
    /// plays, by its file name; empty for the theme's card.
    pub design: String,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            theme: "island".to_string(),
            position: "bottom".to_string(),
            align: "center".to_string(),
            scale: 100,
            font: String::new(),
            text: String::new(),
            background: String::new(),
            accent: String::new(),
            opacity: None,
            radius: None,
            animation: String::new(),
            picture: String::new(),
            sound: String::new(),
            css: String::new(),
            design: String::new(),
        }
    }
}

impl Look {
    /// Read a saved or sent look; anything unknown keeps the theme's.
    pub fn from_json(v: &Value) -> Look {
        let d = Look::default();
        let pick = |name: &str, allowed: &[&str], fallback: String| {
            let value = v.str_or(name, "");
            if allowed.contains(&value) {
                value.to_string()
            } else {
                fallback
            }
        };
        let number = |name: &str, max: f64| {
            v.get(name)
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite())
                .map(|n| n.clamp(0.0, max).round())
        };
        let sound = v.str_or("sound", "");
        Look {
            theme: pick("theme", THEMES, d.theme),
            position: pick("position", &["bottom", "top", "middle"], d.position),
            align: pick("align", &["center", "left", "right"], d.align),
            scale: number("scale", 200.0).map_or(d.scale, |n| (n as u16).max(50)),
            font: pick("font", FONTS, d.font),
            text: color_or_empty(v.str_or("text", "")),
            background: color_or_empty(v.str_or("background", "")),
            accent: color_or_empty(v.str_or("accent", "")),
            opacity: number("opacity", 100.0).map(|n| n as u8),
            radius: number("radius", 48.0).map(|n| n as u8),
            animation: pick("animation", ANIMATIONS, d.animation),
            picture: pick("picture", &["left", "top"], d.picture),
            // "The design's sound" here would point back at itself.
            sound: if sound != THEME_SOUND && is_sound(sound) {
                sound.to_string()
            } else {
                String::new()
            },
            css: v.str_or("css", "").chars().take(MAX_CSS).collect(),
            design: Some(v.str_or("design", ""))
                .filter(|d| is_design(d))
                .unwrap_or_default()
                .to_string(),
        }
    }

    pub fn to_json(&self) -> Value {
        let maybe = |n: Option<u8>| n.map_or(Value::Null, |n| Value::Num(f64::from(n)));
        json::obj([
            ("theme", json::str(&self.theme)),
            ("position", json::str(&self.position)),
            ("align", json::str(&self.align)),
            ("scale", Value::Num(f64::from(self.scale))),
            ("font", json::str(&self.font)),
            ("text", json::str(&self.text)),
            ("background", json::str(&self.background)),
            ("accent", json::str(&self.accent)),
            ("opacity", maybe(self.opacity)),
            ("radius", maybe(self.radius)),
            ("animation", json::str(&self.animation)),
            ("picture", json::str(&self.picture)),
            ("sound", json::str(&self.sound)),
            ("css", json::str(&self.css)),
            ("design", json::str(&self.design)),
        ])
    }
}

struct Alert {
    id: u64,
    card: Card,
    at: Instant,
}

pub struct Board {
    /// Changes with every start of the app, so a page that outlives a
    /// restart knows the ids started over.
    boot: String,
    inner: Mutex<Inner>,
    /// Alerts pages open right now: (in OBS, anywhere else).
    pages: Mutex<(usize, usize)>,
}

#[derive(Default)]
struct Inner {
    last_id: u64,
    alerts: VecDeque<Alert>,
    /// Bumped by the dock's Skip: the page ends the alert it is playing.
    skips: u64,
}

/// Counts one open alerts page for as long as it lives.
pub struct PageOpen<'a> {
    board: &'a Board,
    in_obs: bool,
}

impl Drop for PageOpen<'_> {
    fn drop(&mut self) {
        let mut pages = self.board.pages.lock();
        if self.in_obs {
            pages.0 = pages.0.saturating_sub(1);
        } else {
            pages.1 = pages.1.saturating_sub(1);
        }
    }
}

impl Board {
    pub fn new() -> Board {
        Board {
            boot: crate::crypto::random_token()[..8].to_string(),
            inner: Mutex::new(Inner::default()),
            pages: Mutex::new((0, 0)),
        }
    }

    /// Put an alert up (cleaned of anything the page can't show).
    pub fn show(&self, card: Card) {
        let mut inner = self.inner.lock();
        inner.last_id += 1;
        let alert = Alert {
            id: inner.last_id,
            card: card.cleaned(),
            at: Instant::now(),
        };
        if inner.alerts.len() >= KEEP {
            inner.alerts.pop_front();
        }
        inner.alerts.push_back(alert);
    }

    /// End the alert the pages are playing now (the next one comes on).
    pub fn skip(&self) {
        self.inner.lock().skips += 1;
    }

    /// An alerts page connected; it counts until the guard is dropped.
    pub fn page_opened(&self, in_obs: bool) -> PageOpen<'_> {
        let mut pages = self.pages.lock();
        if in_obs {
            pages.0 += 1;
        } else {
            pages.1 += 1;
        }
        PageOpen {
            board: self,
            in_obs,
        }
    }

    /// How many alerts pages are open, in OBS and anywhere else.
    pub fn pages_open(&self) -> (usize, usize) {
        *self.pages.lock()
    }

    /// The fresh alerts, oldest first, and the page's look. Unchanged
    /// between alerts, so the page's event stream only sends when
    /// something new is up.
    pub fn to_json(&self, look: &Look) -> String {
        let mut inner = self.inner.lock();
        inner.alerts.retain(|a| a.at.elapsed() < FRESH);
        let alerts = Value::Arr(inner.alerts.iter().map(alert_json).collect());
        json::obj([
            ("boot", json::str(&self.boot)),
            ("page", json::str(page_id())),
            ("look", look.to_json()),
            ("alerts", alerts),
            ("skips", Value::Num(inner.skips as f64)),
        ])
        .to_json()
    }
}

fn alert_json(a: &Alert) -> Value {
    let c = &a.card;
    json::obj([
        ("id", Value::Num(a.id as f64)),
        ("title", json::str(&c.title)),
        ("text", json::str(&c.text)),
        ("message", json::str(&c.message)),
        ("ms", Value::Num((c.seconds * 1000) as f64)),
        ("color", json::str(&c.color)),
        ("sound", json::str(&c.sound)),
        ("volume", Value::Num(f64::from(c.volume))),
        ("image", json::str(&c.image)),
        ("design", json::str(&c.design)),
    ])
}

/// `text` on one line (control characters as spaces), at most `max`
/// characters, trimmed.
pub(super) fn one_line(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    flat.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alerts_of(board: &Board) -> Vec<Value> {
        let doc = json::parse(&board.to_json(&Look::default())).unwrap();
        doc.get("alerts")
            .and_then(Value::as_array)
            .unwrap()
            .to_vec()
    }

    #[test]
    fn alerts_queue_with_rising_ids_and_sane_lengths() {
        let board = Board::new();
        board.show(Card::plain("Clip!", "by Ana\r\nnice", 1));
        board.show(Card::plain("", &"x".repeat(900), 99));
        let doc = json::parse(&board.to_json(&Look::default())).unwrap();
        assert_eq!(doc.str_or("boot", "").len(), 8);
        let list = alerts_of(&board);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].str_or("text", ""), "by Ana  nice");
        assert_eq!(list[0].u64_or("ms", 0), MIN_SECONDS * 1000);
        assert_eq!(list[1].u64_or("ms", 0), MAX_SECONDS * 1000);
        assert_eq!(list[1].str_or("text", "").len(), MAX_TEXT);
        assert!(list[1].u64_or("id", 0) > list[0].u64_or("id", 0));
    }

    #[test]
    fn only_the_last_few_are_kept() {
        let board = Board::new();
        for n in 0..20 {
            board.show(Card::plain("", &n.to_string(), 5));
        }
        let list = alerts_of(&board);
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].str_or("text", ""), "12");
    }

    /// The page puts the color, sound and picture straight into the card:
    /// only values it can use as they are get through.
    #[test]
    fn a_cards_color_sound_and_picture_are_checked() {
        let board = Board::new();
        let mut good = Card::plain("New follower", "Ana", 6);
        good.color = "#FF00AA".to_string();
        good.sound = "media:ding.mp3".to_string();
        good.image = "media:party.gif".to_string();
        good.design = "follow_alert-2".to_string();
        board.show(good);
        let mut bad = Card::plain("", "Ana", 6);
        bad.color = "red;background:url(x)".to_string();
        bad.sound = "javascript:alert(1)".to_string();
        bad.image = "https://x.example/a.gif\" onerror=\"x".to_string();
        bad.volume = 250;
        bad.design = "../dashboard".to_string();
        board.show(bad);
        let list = alerts_of(&board);
        assert_eq!(list[0].str_or("color", ""), "#ff00aa");
        assert_eq!(list[0].str_or("sound", ""), "media:ding.mp3");
        assert_eq!(list[0].str_or("image", ""), "media:party.gif");
        assert_eq!(list[0].str_or("design", ""), "follow_alert-2");
        assert_eq!(list[1].str_or("design", "x"), "");
        assert_eq!(
            (list[1].str_or("color", "x"), list[1].str_or("sound", "x")),
            ("", "")
        );
        assert_eq!(list[1].str_or("image", "x"), "");
        assert_eq!(list[1].u64_or("volume", 0), 100);
        assert!(is_sound("https://cdn.example/ding.mp3"));
        assert!(is_sound(THEME_SOUND));
        assert!(!is_sound("media:party.gif"));
        assert!(!is_picture("media:ding.mp3"));
        assert!(!is_web_link("https://"));
        assert!(!is_web_link("ftp://x.example/a.mp3"));
    }

    #[test]
    fn a_look_keeps_only_what_the_page_can_use() {
        let sent = json::parse(
            r##"{"theme":"signal","position":"top","align":"sideways","scale":900,
                "font":"Comic Sans","text":"#FFFFFF","background":"url(x)","opacity":40,
                "radius":-5,"animation":"spin","picture":"top","sound":"theme","css":".card{}"}"##,
        )
        .unwrap();
        let look = Look::from_json(&sent);
        assert_eq!(
            (
                look.theme.as_str(),
                look.position.as_str(),
                look.align.as_str()
            ),
            ("signal", "top", "center")
        );
        assert_eq!(
            (look.scale, look.opacity, look.radius),
            (200, Some(40), Some(0))
        );
        assert_eq!((look.font.as_str(), look.text.as_str()), ("", "#ffffff"));
        assert_eq!(
            (look.background.as_str(), look.animation.as_str()),
            ("", "")
        );
        assert_eq!((look.picture.as_str(), look.sound.as_str()), ("top", ""));
        assert_eq!(look.css, ".card{}");
        assert_eq!(Look::from_json(&Value::Null), Look::default());
        let back = Look::from_json(&look.to_json());
        assert_eq!(back, look);
    }

    /// The page served and the feed agree on the fingerprint, so a page
    /// only ever reloads after a real update.
    #[test]
    fn the_page_knows_its_own_fingerprint() {
        let html = page_html();
        assert!(!html.contains(PAGE_ID_MARK));
        assert!(html.contains(&format!("'{}'", page_id())));
        let doc = json::parse(&Board::new().to_json(&Look::default())).unwrap();
        assert_eq!(doc.str_or("page", ""), page_id());
    }

    /// A theme the dashboard offers is drawn by the page, coming in and
    /// going out its own way.
    #[test]
    fn every_theme_has_its_own_motion() {
        // The theme's own rule that names its entrance, wherever it sits
        // and in whatever order its properties come.
        for theme in THEMES {
            let head = format!("body.theme-{theme}{{");
            let motion = PAGE
                .match_indices(&head)
                .map(|(at, _)| &PAGE[at..at + PAGE[at..].find('}').unwrap_or(PAGE.len() - at)])
                .find(|rule| rule.contains("--theme-in:"));
            let rule = motion.unwrap_or_else(|| panic!("{theme} has no entrance"));
            assert!(rule.contains("--theme-out:"), "{theme} has no exit");
        }
    }

    #[test]
    fn open_pages_are_counted_while_they_stay() {
        let board = Board::new();
        let obs = board.page_opened(true);
        {
            let _browser = board.page_opened(false);
            assert_eq!(board.pages_open(), (1, 1));
        }
        assert_eq!(board.pages_open(), (1, 0));
        drop(obs);
        assert_eq!(board.pages_open(), (0, 0));
    }
}
