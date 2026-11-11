//! Delay transition settings.
//!
//! Turning a delay on (or raising it) normally jumps viewers back by the
//! delay, so they see the last seconds again. With the transition on, each
//! destination plays a screen instead, until the delay has built up behind
//! it, then picks up exactly where viewers left off: nothing replayed,
//! nothing missed. The screen has the crash-protection screen's look (theme
//! and colours) with its own words. Destinations whose video the screens
//! can't stand in for (AV1, 10-bit HEVC) jump as before.
//!
//! This file owns the `delay_transition.*` config keys; the screen is
//! played by `crate::crash_hold::play_transition`.

use crate::crash_protection::{self, CrashProtection};
use std::io::{self, Write};

/// Config and form keys share this prefix, like `crash_protection.`.
pub const KEY_PREFIX: &str = "delay_transition.";
const HEADLINE_DEFAULT: &str = "Back in a moment";
const SUBLINE_DEFAULT: &str = "The stream picks up right where it left off";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayTransition {
    pub enabled: bool,
    pub headline: String,
    pub subline: String,
}

impl Default for DelayTransition {
    fn default() -> Self {
        Self {
            enabled: false,
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

    /// The screen a transition plays: the reconnect screen's look, with
    /// the transition's own words.
    pub fn screen(&self, look: &CrashProtection) -> CrashProtection {
        CrashProtection {
            headline: self.headline.clone(),
            subline: self.subline.clone(),
            ..look.clone()
        }
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
            r#"{{"enabled":{},"headline":{},"subline":{},"defaults":{{"headline":{},"subline":{}}}}}"#,
            self.enabled,
            crate::config::json_str(&self.headline),
            crate::config::json_str(&self.subline),
            crate::config::json_str(&defaults.headline),
            crate::config::json_str(&defaults.subline),
        )
    }

    fn entries(&self) -> [(&'static str, String); 3] {
        [
            ("enabled", self.enabled.to_string()),
            ("headline", self.headline.clone()),
            ("subline", self.subline.clone()),
        ]
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
        settings.set("headline", "  Be right back\n");
        let mut lines = Vec::new();
        settings.write_lines(&mut lines).unwrap();
        let text = String::from_utf8(lines).unwrap();
        assert_eq!(
            text,
            "delay_transition.enabled=true\ndelay_transition.headline=Be right back\n"
        );
        let mut loaded = DelayTransition::default();
        for line in text.lines() {
            let (key, value) = line.split_once('=').unwrap();
            loaded.set(&key[KEY_PREFIX.len()..], value);
        }
        assert_eq!(loaded, settings);
    }

    #[test]
    fn the_screen_keeps_the_look_and_takes_its_own_words() {
        let look = CrashProtection {
            theme: SlateTheme::Halo,
            ..CrashProtection::default()
        };
        let transition = DelayTransition::default();
        let screen = transition.screen(&look);
        assert_eq!(screen.theme, SlateTheme::Halo);
        assert_eq!(screen.accent, look.accent);
        assert_eq!(screen.headline, HEADLINE_DEFAULT);
        assert_eq!(screen.subline, SUBLINE_DEFAULT);
    }
}
