//! Crash protection settings.
//!
//! When OBS dies mid-stream, crash protection keeps every destination live
//! with a looping reconnect screen until OBS publishes again or the hold
//! time runs out. This file owns the persisted knobs (the
//! `crash_protection.*` config keys); the screen itself is drawn and
//! encoded by `crate::slate`.

use crate::slate::Rgb;
use std::io::{self, Write};

pub const HOLD_SECS_DEFAULT: u32 = 120;
pub const HOLD_SECS_MIN: u32 = 30;
pub const HOLD_SECS_MAX: u32 = 300;
pub const HEADLINE_MAX_CHARS: usize = 28;
pub const SUBLINE_MAX_CHARS: usize = 48;
const ACCENT_DEFAULT: Rgb = Rgb::new(0x5a, 0xc8, 0xfa);
const HEADLINE_DEFAULT: &str = "Reconnecting";
const SUBLINE_DEFAULT: &str = "The stream will be back in a moment";
/// Config and form keys share this prefix, like `hotkey.` and `midi.`.
pub const KEY_PREFIX: &str = "crash_protection.";

/// Look of the reconnect screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlateTheme {
    /// Minimal centred text with three pulsing dots.
    Whisper,
    /// 5x7 pixel font snapped to the 16 px macroblock grid.
    Arcade,
    /// Centred text under a dot sending out radar rings.
    Beacon,
    /// Centred text under a ring of eight dots, one lit and chasing round.
    Orbit,
    /// Broadcast lower third: text on the left with an accent bar and a
    /// sliding progress stripe.
    Studio,
}

impl SlateTheme {
    /// Every theme, in the order the dashboard offers them.
    pub const ALL: [SlateTheme; 5] = [
        SlateTheme::Whisper,
        SlateTheme::Beacon,
        SlateTheme::Orbit,
        SlateTheme::Studio,
        SlateTheme::Arcade,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SlateTheme::Whisper => "whisper",
            SlateTheme::Arcade => "arcade",
            SlateTheme::Beacon => "beacon",
            SlateTheme::Orbit => "orbit",
            SlateTheme::Studio => "studio",
        }
    }

    /// Name shown in the dashboard.
    pub fn name(self) -> &'static str {
        match self {
            SlateTheme::Whisper => "Whisper",
            SlateTheme::Arcade => "Arcade",
            SlateTheme::Beacon => "Beacon",
            SlateTheme::Orbit => "Orbit",
            SlateTheme::Studio => "Studio",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.id() == id)
    }

    /// Background used when the streamer hasn't picked one.
    pub fn default_background(self) -> Rgb {
        match self {
            SlateTheme::Whisper => Rgb::new(0x0e, 0x0f, 0x12),
            SlateTheme::Arcade => Rgb::new(0x13, 0x0f, 0x22),
            SlateTheme::Beacon => Rgb::new(0x0b, 0x12, 0x16),
            SlateTheme::Orbit => Rgb::new(0x10, 0x10, 0x16),
            SlateTheme::Studio => Rgb::new(0x0d, 0x10, 0x18),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashProtection {
    pub enabled: bool,
    /// How long destinations stay up on the reconnect screen before the
    /// stream ends cleanly. Clamped to HOLD_SECS_MIN..=HOLD_SECS_MAX.
    pub hold_secs: u32,
    pub theme: SlateTheme,
    pub accent: Rgb,
    /// None means the theme's own background.
    pub background: Option<Rgb>,
    pub headline: String,
    pub subline: String,
    /// Treat a clean disconnect (FCUnpublish/deleteStream) as a crash too.
    /// OBS closes cleanly on encoder errors, so this is the only way to
    /// protect against them; the streamer ends the hold with "End now".
    pub every_disconnect: bool,
}

impl Default for CrashProtection {
    fn default() -> Self {
        Self {
            enabled: false,
            hold_secs: HOLD_SECS_DEFAULT,
            theme: SlateTheme::Whisper,
            accent: ACCENT_DEFAULT,
            background: None,
            headline: HEADLINE_DEFAULT.into(),
            subline: SUBLINE_DEFAULT.into(),
            every_disconnect: false,
        }
    }
}

impl CrashProtection {
    /// Apply one `field=value` pair (the part after KEY_PREFIX). Malformed
    /// values are dropped so a hand-edited config or form can't load a
    /// colour the rasterizer can't parse or a hold time outside the range.
    pub fn set(&mut self, field: &str, value: &str) {
        match field {
            "enabled" => self.enabled = is_truthy(value),
            "hold_secs" => {
                if let Ok(secs) = value.trim().parse::<u32>() {
                    self.hold_secs = secs.clamp(HOLD_SECS_MIN, HOLD_SECS_MAX);
                }
            }
            "theme" => {
                if let Some(theme) = SlateTheme::from_id(value) {
                    self.theme = theme;
                }
            }
            "accent" => {
                if let Some(color) = Rgb::from_hex(value) {
                    self.accent = color;
                }
            }
            "background" if value.is_empty() => self.background = None,
            "background" => {
                if let Some(color) = Rgb::from_hex(value) {
                    self.background = Some(color);
                }
            }
            "headline" => self.headline = sanitize_text(value, HEADLINE_MAX_CHARS),
            "subline" => self.subline = sanitize_text(value, SUBLINE_MAX_CHARS),
            "every_disconnect" => self.every_disconnect = is_truthy(value),
            _ => {}
        }
    }

    /// Whether `other` draws the same reconnect screen. The hold time and
    /// what triggers a hold don't change the picture, so editing them
    /// never re-encodes a loop.
    pub fn same_screen(&self, other: &CrashProtection) -> bool {
        self.theme == other.theme
            && self.accent == other.accent
            && self.resolved_background() == other.resolved_background()
            && self.headline == other.headline
            && self.subline == other.subline
    }

    /// Background the screen actually draws with.
    pub fn resolved_background(&self) -> Rgb {
        self.background
            .unwrap_or_else(|| self.theme.default_background())
    }

    /// Write every field that differs from the default, one
    /// `crash_protection.<field>=<value>` line each, so an untouched
    /// install keeps its config file lean.
    pub fn write_lines(&self, out: &mut impl Write) -> io::Result<()> {
        let defaults = Self::default().entries();
        for ((field, value), (_, default_value)) in self.entries().into_iter().zip(defaults) {
            if value != default_value {
                writeln!(out, "{KEY_PREFIX}{field}={value}")?;
            }
        }
        Ok(())
    }

    /// JSON object for `GET /config`. Nothing here is secret. `themes`
    /// lists what the dashboard can offer, with each theme's default
    /// background, so the page never keeps its own copy.
    pub fn to_json(&self) -> String {
        let themes: Vec<String> = SlateTheme::ALL
            .iter()
            .map(|theme| {
                format!(
                    r#"{{"id":{},"name":{},"background":{}}}"#,
                    crate::config::json_str(theme.id()),
                    crate::config::json_str(theme.name()),
                    crate::config::json_str(&theme.default_background().to_hex()),
                )
            })
            .collect();
        format!(
            r#"{{"enabled":{},"hold_secs":{},"theme":{},"accent":{},"background":{},"headline":{},"subline":{},"every_disconnect":{},"defaults":{{"headline":{},"subline":{}}},"themes":[{}]}}"#,
            self.enabled,
            self.hold_secs,
            crate::config::json_str(self.theme.id()),
            crate::config::json_str(&self.accent.to_hex()),
            crate::config::json_str(&self.background.map(Rgb::to_hex).unwrap_or_default()),
            crate::config::json_str(&self.headline),
            crate::config::json_str(&self.subline),
            self.every_disconnect,
            crate::config::json_str(HEADLINE_DEFAULT),
            crate::config::json_str(SUBLINE_DEFAULT),
            themes.join(","),
        )
    }

    fn entries(&self) -> [(&'static str, String); 8] {
        [
            ("enabled", self.enabled.to_string()),
            ("hold_secs", self.hold_secs.to_string()),
            ("theme", self.theme.id().to_string()),
            ("accent", self.accent.to_hex()),
            (
                "background",
                self.background.map(Rgb::to_hex).unwrap_or_default(),
            ),
            ("headline", self.headline.clone()),
            ("subline", self.subline.clone()),
            ("every_disconnect", self.every_disconnect.to_string()),
        ]
    }
}

/// Checkbox values arrive as "true"/"false" or "on"/"" depending on the
/// caller, the same convention the other boolean settings use.
fn is_truthy(value: &str) -> bool {
    !matches!(value, "" | "false" | "0" | "off")
}

/// Screen text: no control characters (the config format is line-based),
/// surrounding whitespace trimmed, capped to what fits on the screen.
fn sanitize_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(settings: &CrashProtection) -> String {
        let mut out = Vec::new();
        settings.write_lines(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn defaults_write_nothing() {
        assert_eq!(written(&CrashProtection::default()), "");
    }

    #[test]
    fn changed_fields_round_trip_through_set() {
        let mut settings = CrashProtection::default();
        settings.set("enabled", "true");
        settings.set("hold_secs", "90");
        settings.set("theme", "arcade");
        settings.set("accent", "#FF6B6B");
        settings.set("background", "#101010");
        settings.set("headline", "Be right back");

        let mut loaded = CrashProtection::default();
        for line in written(&settings).lines() {
            let (key, value) = line.split_once('=').unwrap();
            loaded.set(key.strip_prefix(KEY_PREFIX).unwrap(), value);
        }
        assert_eq!(loaded, settings);
    }

    #[test]
    fn hold_secs_is_clamped_and_garbage_ignored() {
        let mut settings = CrashProtection::default();
        settings.set("hold_secs", "5");
        assert_eq!(settings.hold_secs, HOLD_SECS_MIN);
        settings.set("hold_secs", "9999");
        assert_eq!(settings.hold_secs, HOLD_SECS_MAX);
        settings.set("hold_secs", "soon");
        assert_eq!(settings.hold_secs, HOLD_SECS_MAX);
    }

    #[test]
    fn bad_colours_and_themes_are_dropped() {
        let mut settings = CrashProtection::default();
        settings.set("accent", "red");
        settings.set("accent", "#12345");
        settings.set("theme", "signal");
        assert_eq!(settings.accent, ACCENT_DEFAULT);
        assert_eq!(settings.theme, SlateTheme::Whisper);
    }

    #[test]
    fn empty_background_means_theme_default() {
        let mut settings = CrashProtection::default();
        settings.set("background", "#202020");
        settings.set("background", "");
        assert_eq!(settings.background, None);
        assert_eq!(
            settings.resolved_background(),
            SlateTheme::Whisper.default_background()
        );
    }

    #[test]
    fn text_loses_control_characters_and_is_capped() {
        let mut settings = CrashProtection::default();
        settings.set("headline", "  Back\nsoon\t ");
        assert_eq!(settings.headline, "Backsoon");
        settings.set("subline", &"x".repeat(100));
        assert_eq!(settings.subline.chars().count(), SUBLINE_MAX_CHARS);
    }

    #[test]
    fn json_carries_every_field() {
        let json = CrashProtection::default().to_json();
        for field in [
            "enabled",
            "hold_secs",
            "theme",
            "accent",
            "background",
            "headline",
            "subline",
            "every_disconnect",
            "themes",
        ] {
            assert!(
                json.contains(&format!("\"{field}\":")),
                "missing {field} in {json}"
            );
        }
        assert!(json.contains(r##""accent":"#5ac8fa""##));
    }

    #[test]
    fn every_theme_round_trips_by_id() {
        for theme in SlateTheme::ALL {
            let mut settings = CrashProtection::default();
            settings.set("theme", theme.id());
            assert_eq!(settings.theme, theme);
            let json = settings.to_json();
            assert!(
                json.contains(&format!(r#""id":"{}""#, theme.id())),
                "{json}"
            );
        }
    }
}
