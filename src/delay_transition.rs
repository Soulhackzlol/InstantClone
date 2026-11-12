//! Delay transition settings.
//!
//! Turning a delay on (or raising it) normally jumps viewers back by the
//! delay, so they see the last seconds again. With the transition on, each
//! destination plays a screen instead, until the delay has built up behind
//! it, then picks up exactly where viewers left off: nothing replayed,
//! nothing missed. The screen has its own style and words, in the
//! crash-protection screen's colours, and its words can name the delay
//! being turned on (`DELAY_TOKEN`). Destinations whose video the screens
//! can't stand in for (AV1, 10-bit HEVC) jump as before.
//!
//! This file owns the `delay_transition.*` config keys; the screen is
//! played by `crate::crash_hold::play_transition`.

use crate::crash_protection::{self, CrashProtection, SlateTheme};
use std::io::{self, Write};

/// Config and form keys share this prefix, like `crash_protection.`.
pub const KEY_PREFIX: &str = "delay_transition.";
/// Stands for the delay being turned on, in the screen's words: "30 s",
/// "1 min 30 s".
pub const DELAY_TOKEN: &str = "{delay}";
const HEADLINE_DEFAULT: &str = "Loading a {delay} delay";
const SUBLINE_DEFAULT: &str = "The stream picks up right where it left off";
const THEME_DEFAULT: SlateTheme = SlateTheme::Halo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayTransition {
    pub enabled: bool,
    pub theme: SlateTheme,
    pub headline: String,
    pub subline: String,
}

impl Default for DelayTransition {
    fn default() -> Self {
        Self {
            enabled: false,
            theme: THEME_DEFAULT,
            headline: HEADLINE_DEFAULT.into(),
            subline: SUBLINE_DEFAULT.into(),
        }
    }
}

impl DelayTransition {
    /// Apply one `field=value` pair (the part after KEY_PREFIX), cleaning
    /// text the way the crash-protection screen does.
    pub fn set(&mut self, field: &str, value: &str) {
        match field {
            "enabled" => self.enabled = crash_protection::is_truthy(value),
            "theme" => {
                if let Some(theme) = SlateTheme::from_id(value) {
                    self.theme = theme;
                }
            }
            "headline" => {
                self.headline =
                    crash_protection::sanitize_text(value, crash_protection::HEADLINE_MAX_CHARS)
            }
            "subline" => {
                self.subline =
                    crash_protection::sanitize_text(value, crash_protection::SUBLINE_MAX_CHARS)
            }
            _ => {}
        }
    }

    /// The screen a transition to a delay of `delay_ms` plays: its own
    /// style and words in the reconnect screen's colours (a background the
    /// streamer left on the default follows the transition's style).
    pub fn screen(&self, look: &CrashProtection, delay_ms: u32) -> CrashProtection {
        CrashProtection {
            theme: self.theme,
            headline: with_delay(&self.headline, delay_ms),
            subline: with_delay(&self.subline, delay_ms),
            ..look.clone()
        }
    }

    /// Whether the words name the delay, so each delay draws its own
    /// screen.
    pub fn names_the_delay(&self) -> bool {
        self.headline.contains(DELAY_TOKEN) || self.subline.contains(DELAY_TOKEN)
    }

    /// Config lines for every setting that differs from its default.
    pub fn write_lines(&self, out: &mut impl Write) -> io::Result<()> {
        let defaults = Self::default().entries();
        for ((field, value), (_, default_value)) in self.entries().into_iter().zip(defaults) {
            if value != default_value {
                writeln!(out, "{KEY_PREFIX}{field}={value}")?;
            }
        }
        Ok(())
    }

    /// JSON object for `GET /config`, with the default words so the page
    /// can offer to restore them.
    pub fn to_json(&self) -> String {
        let defaults = Self::default();
        format!(
            r#"{{"enabled":{},"theme":{},"headline":{},"subline":{},"defaults":{{"headline":{},"subline":{}}}}}"#,
            self.enabled,
            crate::config::json_str(self.theme.id()),
            crate::config::json_str(&self.headline),
            crate::config::json_str(&self.subline),
            crate::config::json_str(&defaults.headline),
            crate::config::json_str(&defaults.subline),
        )
    }

    fn entries(&self) -> [(&'static str, String); 4] {
        [
            ("enabled", self.enabled.to_string()),
            ("theme", self.theme.id().to_string()),
            ("headline", self.headline.clone()),
            ("subline", self.subline.clone()),
        ]
    }
}

/// `text` with `DELAY_TOKEN` written out as `delay_ms`.
pub fn with_delay(text: &str, delay_ms: u32) -> String {
    if text.contains(DELAY_TOKEN) {
        text.replace(DELAY_TOKEN, &delay_words(delay_ms))
    } else {
        text.to_string()
    }
}

/// A delay as viewers read it: "45 s", "1 min", "1 min 30 s". Whole
/// seconds, never less than one.
pub fn delay_words(delay_ms: u32) -> String {
    let secs = ((delay_ms + 500) / 1000).max(1);
    match (secs / 60, secs % 60) {
        (0, secs) => format!("{secs} s"),
        (mins, 0) => format!("{mins} min"),
        (mins, secs) => format!("{mins} min {secs} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash_protection::SlateTheme;

    #[test]
    fn only_changed_settings_are_written_and_they_load_back() {
        let mut settings = DelayTransition::default();
        let mut lines = Vec::new();
        settings.write_lines(&mut lines).unwrap();
        assert!(lines.is_empty(), "defaults write nothing");

        settings.set("enabled", "on");
        settings.set("theme", "halftone");
        settings.set("theme", "no-such-style");
        settings.set("headline", "  Be right back\n");
        let mut lines = Vec::new();
        settings.write_lines(&mut lines).unwrap();
        let text = String::from_utf8(lines).unwrap();
        assert_eq!(
            text,
            "delay_transition.enabled=true\ndelay_transition.theme=halftone\ndelay_transition.headline=Be right back\n"
        );
        let mut loaded = DelayTransition::default();
        for line in text.lines() {
            let (key, value) = line.split_once('=').unwrap();
            loaded.set(&key[KEY_PREFIX.len()..], value);
        }
        assert_eq!(loaded, settings);
    }

    #[test]
    fn the_screen_has_its_own_style_and_words_in_the_crash_colours() {
        let look = CrashProtection {
            theme: SlateTheme::Arcade,
            accent: crate::slate::Rgb::new(0xff, 0x6b, 0x6b),
            ..CrashProtection::default()
        };
        let transition = DelayTransition {
            theme: SlateTheme::Embers,
            ..DelayTransition::default()
        };
        let screen = transition.screen(&look, 30_000);
        assert_eq!(screen.theme, SlateTheme::Embers);
        assert_eq!(screen.accent, look.accent);
        assert_eq!(
            screen.resolved_background(),
            SlateTheme::Embers.default_background(),
            "a default background follows the transition's style"
        );
        assert_eq!(screen.headline, "Loading a 30 s delay");
        assert_eq!(screen.subline, SUBLINE_DEFAULT);
    }

    #[test]
    fn the_words_name_the_delay_being_turned_on() {
        for (delay_ms, words) in [
            (0, "1 s"),
            (2_400, "2 s"),
            (2_500, "3 s"),
            (45_000, "45 s"),
            (60_000, "1 min"),
            (90_000, "1 min 30 s"),
            (3_600_000, "60 min"),
        ] {
            assert_eq!(delay_words(delay_ms), words);
        }
        let mut transition = DelayTransition::default();
        assert!(transition.names_the_delay());
        transition.set("headline", "Going {delay} back, {delay}!");
        transition.set("subline", "Hold on");
        let screen = transition.screen(&CrashProtection::default(), 300_000);
        assert_eq!(screen.headline, "Going 5 min back, 5 min!");
        assert_eq!(screen.subline, "Hold on");
        transition.set("headline", "Back soon");
        assert!(!transition.names_the_delay());
    }
}
