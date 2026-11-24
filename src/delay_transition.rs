//! Delay transition settings.
//!
//! Turning a delay on (or raising it) jumps viewers back by the delay, so
//! they see the last seconds again. With the transition on, each
//! destination plays a screen over the jump for about a second ("Going back
//! 30 s"), and the delay is on at once. The screen has its own style,
//! colours and words, and its words can name the delay being turned on
//! (`DELAY_TOKEN`). The stream cuts straight to the screen and back.
//! Destinations whose video the screens can't stand in for (AV1, 10-bit
//! HEVC) jump as before. Only the streamer's own changes play a screen, not
//! a slipped delay put right.
//!
//! Turning a delay off (or lowering it) jumps viewers forward instead. With
//! `catch_up` on, the jump plays a short screen of its own words in the
//! same style.
//!
//! This file owns the `delay_transition.*` config keys; the screen is
//! played by `crate::crash_hold::play_transition`.

use crate::crash_protection::{self, CrashProtection, SlateTheme};
use crate::slate::Rgb;
use std::io::{self, Write};

/// Config and form keys share this prefix, like `crash_protection.`.
pub const KEY_PREFIX: &str = "delay_transition.";
/// Stands for the delay being turned on, in the screen's words: "30 s",
/// "1 min 30 s".
pub const DELAY_TOKEN: &str = "{delay}";
const HEADLINE_DEFAULT: &str = "Going back {delay}";
const SUBLINE_DEFAULT: &str = "Turning the delay on";
const CATCH_UP_HEADLINE_DEFAULT: &str = "Going back to the present";
const CATCH_UP_SUBLINE_DEFAULT: &str = "Catching up with the stream";
/// The words left as they came use these for a delay that only moves: up
/// from one already on, or down to one still on. The defaults above are
/// for a delay turned on or off, and would misread for these.
const RAISE_HEADLINE_DEFAULT: &str = "Delay up to {delay}";
const LOWER_HEADLINE_DEFAULT: &str = "Delay down to {delay}";
const MOVE_SUBLINE_DEFAULT: &str = "Back in a moment";
const THEME_DEFAULT: SlateTheme = SlateTheme::Halo;
const NO_DELAY_WORDS: &str = "no delay";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayTransition {
    pub enabled: bool,
    pub theme: SlateTheme,
    pub accent: Rgb,
    /// None means the style's own background.
    pub background: Option<Rgb>,
    pub headline: String,
    pub subline: String,
    /// A short screen of its own words when a delay goes off or down.
    pub catch_up: bool,
    pub catch_up_headline: String,
    pub catch_up_subline: String,
}

impl Default for DelayTransition {
    fn default() -> Self {
        Self {
            enabled: false,
            theme: THEME_DEFAULT,
            accent: CrashProtection::default().accent,
            background: None,
            headline: HEADLINE_DEFAULT.into(),
            subline: SUBLINE_DEFAULT.into(),
            catch_up: false,
            catch_up_headline: CATCH_UP_HEADLINE_DEFAULT.into(),
            catch_up_subline: CATCH_UP_SUBLINE_DEFAULT.into(),
        }
    }
}

impl DelayTransition {
    /// Apply one `field=value` pair (the part after KEY_PREFIX), cleaning
    /// text and dropping malformed colours the way the crash-protection
    /// screen does.
    pub fn set(&mut self, field: &str, value: &str) {
        match field {
            "enabled" => self.enabled = crash_protection::is_truthy(value),
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
            "headline" => self.headline = headline_text(value),
            "subline" => self.subline = subline_text(value),
            "catch_up" => self.catch_up = crash_protection::is_truthy(value),
            "catch_up_headline" => self.catch_up_headline = headline_text(value),
            "catch_up_subline" => self.catch_up_subline = subline_text(value),
            _ => {}
        }
    }

    /// The screen a transition to a delay of `delay_ms` plays, in its own
    /// style, colours and words (drawn by the crash screen's renderer).
    /// `was_on`: a delay was already on, so this one raises it.
    pub fn screen(&self, delay_ms: u32, was_on: bool) -> CrashProtection {
        let (headline, subline) = self.rising_words(was_on);
        self.worded(headline, subline, delay_ms)
    }

    /// The screen a delay going off or down to `delay_ms` plays: the same
    /// look with the catch-up words.
    pub fn catch_up_screen(&self, delay_ms: u32) -> CrashProtection {
        let (headline, subline) = self.falling_words(delay_ms > 0);
        self.worded(headline, subline, delay_ms)
    }

    /// The words for a delay going up. As they came, they fit the change:
    /// "Going back 30 s, Turning the delay on" for one turned on, "Delay up
    /// to 1 min" for one already on. Words the streamer wrote stay theirs.
    fn rising_words(&self, was_on: bool) -> (&str, &str) {
        if !was_on {
            return (&self.headline, &self.subline);
        }
        (
            unless_edited(&self.headline, HEADLINE_DEFAULT, RAISE_HEADLINE_DEFAULT),
            unless_edited(&self.subline, SUBLINE_DEFAULT, MOVE_SUBLINE_DEFAULT),
        )
    }

    /// The words for a delay going down, the same way: "Going back to the
    /// present" for one turned off, "Delay down to 15 s" for one that
    /// `stays_on`.
    fn falling_words(&self, stays_on: bool) -> (&str, &str) {
        if !stays_on {
            return (&self.catch_up_headline, &self.catch_up_subline);
        }
        (
            unless_edited(
                &self.catch_up_headline,
                CATCH_UP_HEADLINE_DEFAULT,
                LOWER_HEADLINE_DEFAULT,
            ),
            unless_edited(
                &self.catch_up_subline,
                CATCH_UP_SUBLINE_DEFAULT,
                MOVE_SUBLINE_DEFAULT,
            ),
        )
    }

    fn worded(&self, headline: &str, subline: &str, delay_ms: u32) -> CrashProtection {
        CrashProtection {
            theme: self.theme,
            accent: self.accent,
            background: self.background,
            headline: with_delay(headline, delay_ms),
            subline: with_delay(subline, delay_ms),
            ..CrashProtection::default()
        }
    }

    /// Whether either screen plays at all, so its loops are worth keeping.
    pub fn any_screen(&self) -> bool {
        self.enabled || self.catch_up
    }

    /// Whether the words name the delay, so each delay draws its own
    /// screen.
    pub fn names_the_delay(&self) -> bool {
        [false, true]
            .into_iter()
            .any(|was_on| names_it(self.rising_words(was_on)))
    }

    /// Whether the catch-up words name the delay.
    pub fn catch_up_names_the_delay(&self) -> bool {
        [false, true]
            .into_iter()
            .any(|stays_on| names_it(self.falling_words(stays_on)))
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
            concat!(
                r#"{{"enabled":{},"theme":{},"accent":{},"background":{},"headline":{},"subline":{},"#,
                r#""catch_up":{},"catch_up_headline":{},"catch_up_subline":{},"defaults":{{"#,
                r#""headline":{},"subline":{},"catch_up_headline":{},"catch_up_subline":{}}}}}"#
            ),
            self.enabled,
            crate::config::json_str(self.theme.id()),
            crate::config::json_str(&self.accent.to_hex()),
            crate::config::json_str(&self.background.map(Rgb::to_hex).unwrap_or_default()),
            crate::config::json_str(&self.headline),
            crate::config::json_str(&self.subline),
            self.catch_up,
            crate::config::json_str(&self.catch_up_headline),
            crate::config::json_str(&self.catch_up_subline),
            crate::config::json_str(&defaults.headline),
            crate::config::json_str(&defaults.subline),
            crate::config::json_str(&defaults.catch_up_headline),
            crate::config::json_str(&defaults.catch_up_subline),
        )
    }

    fn entries(&self) -> [(&'static str, String); 9] {
        [
            ("enabled", self.enabled.to_string()),
            ("theme", self.theme.id().to_string()),
            ("accent", self.accent.to_hex()),
            (
                "background",
                self.background.map(Rgb::to_hex).unwrap_or_default(),
            ),
            ("headline", self.headline.clone()),
            ("subline", self.subline.clone()),
            ("catch_up", self.catch_up.to_string()),
            ("catch_up_headline", self.catch_up_headline.clone()),
            ("catch_up_subline", self.catch_up_subline.clone()),
        ]
    }
}

/// `words`, or `instead` while they are still the `default` they came as.
fn unless_edited<'a>(words: &'a str, default: &str, instead: &'a str) -> &'a str {
    if words == default {
        instead
    } else {
        words
    }
}

/// Whether a headline and small line name the delay.
fn names_it((headline, subline): (&str, &str)) -> bool {
    headline.contains(DELAY_TOKEN) || subline.contains(DELAY_TOKEN)
}

/// Headline words cleaned the way the crash-protection screen's are.
fn headline_text(value: &str) -> String {
    crash_protection::sanitize_text(value, crash_protection::HEADLINE_MAX_CHARS)
}

fn subline_text(value: &str) -> String {
    crash_protection::sanitize_text(value, crash_protection::SUBLINE_MAX_CHARS)
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
/// seconds, never less than one; a delay turned off (which a catch-up
/// screen can name) reads "no delay".
pub fn delay_words(delay_ms: u32) -> String {
    if delay_ms == 0 {
        return NO_DELAY_WORDS.to_string();
    }
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
    fn the_screen_has_its_own_style_colours_and_words() {
        let mut transition = DelayTransition {
            theme: SlateTheme::Embers,
            ..DelayTransition::default()
        };
        transition.set("accent", "#FF6B6B");
        transition.set("accent", "red");
        let screen = transition.screen(30_000, false);
        assert_eq!(screen.theme, SlateTheme::Embers);
        assert_eq!(
            screen.accent,
            Rgb::new(0xff, 0x6b, 0x6b),
            "a bad colour is dropped"
        );
        assert_eq!(
            screen.resolved_background(),
            SlateTheme::Embers.default_background(),
            "no background chosen: the style's own"
        );
        transition.set("background", "#101010");
        assert_eq!(
            transition.screen(0, false).background,
            Some(Rgb::new(0x10, 0x10, 0x10))
        );
        transition.set("background", "");
        assert_eq!(transition.background, None);
        assert_eq!(screen.headline, "Going back 30 s");
        assert_eq!(screen.subline, SUBLINE_DEFAULT);
    }

    #[test]
    fn the_words_name_the_delay_being_turned_on() {
        for (delay_ms, words) in [
            (0, "no delay"),
            (300, "1 s"),
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
        for was_on in [false, true] {
            let screen = transition.screen(300_000, was_on);
            assert_eq!(screen.headline, "Going 5 min back, 5 min!", "written: kept");
            assert_eq!(screen.subline, "Hold on");
        }
        transition.set("headline", "Back soon");
        assert!(!transition.names_the_delay());
    }

    /// Words left as they came fit the change: a delay turned on or off
    /// reads as that, one only raised or lowered reads as the move. Each
    /// line the streamer edits stays as written.
    #[test]
    fn default_words_fit_how_the_delay_changes() {
        let mut transition = DelayTransition::default();
        let words = |screen: CrashProtection| (screen.headline, screen.subline);
        let pair = |headline: &str, subline: &str| (headline.to_string(), subline.to_string());
        assert_eq!(
            words(transition.screen(30_000, false)),
            pair("Going back 30 s", "Turning the delay on")
        );
        assert_eq!(
            words(transition.screen(60_000, true)),
            pair("Delay up to 1 min", "Back in a moment")
        );
        assert_eq!(
            words(transition.catch_up_screen(0)),
            pair("Going back to the present", "Catching up with the stream")
        );
        assert_eq!(
            words(transition.catch_up_screen(15_000)),
            pair("Delay down to 15 s", "Back in a moment")
        );
        assert!(
            transition.catch_up_names_the_delay(),
            "a lowered delay is named"
        );

        transition.set("subline", "Hang tight");
        assert_eq!(
            words(transition.screen(60_000, true)),
            pair("Delay up to 1 min", "Hang tight"),
            "an edited line stays, the other still fits"
        );
    }

    #[test]
    fn the_catch_up_screen_has_its_own_words_in_the_same_style() {
        let mut transition = DelayTransition::default();
        assert!(!transition.catch_up, "off until the streamer turns it on");
        transition.set("catch_up_headline", "Back to the present");
        transition.set("catch_up_subline", "Live again");
        assert!(
            !transition.catch_up_names_the_delay(),
            "written words name nothing"
        );
        transition.set("catch_up_headline", CATCH_UP_HEADLINE_DEFAULT);
        transition.set("theme", "embers");
        transition.set("catch_up", "on");
        transition.set("background", "#202020");
        transition.set("catch_up_subline", "Down to {delay}");
        let screen = transition.catch_up_screen(10_000);
        assert_eq!(screen.theme, SlateTheme::Embers);
        assert_eq!(screen.headline, "Delay down to 10 s");
        assert_eq!(screen.subline, "Down to 10 s");
        assert!(transition.catch_up_names_the_delay());

        let mut lines = Vec::new();
        transition.write_lines(&mut lines).unwrap();
        let text = String::from_utf8(lines).unwrap();
        let mut loaded = DelayTransition::default();
        for line in text.lines() {
            let (key, value) = line.split_once('=').unwrap();
            loaded.set(&key[KEY_PREFIX.len()..], value);
        }
        assert_eq!(loaded, transition, "the catch-up settings load back");
    }
}
