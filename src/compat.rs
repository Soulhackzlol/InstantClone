//! Encoder-compatibility checks: compare what OBS is actually sending
//! against what the *enabled* destinations accept.
//!
//! Design rule: silence when healthy. This module returns `None` far more
//! often than it returns a warning, and when it does warn it produces ONE
//! line naming the affected platforms - never a per-check badge. A wrong
//! or noisy warning is worse than no warning, so every rule here is
//! deliberately conservative: it fires only on settings that genuinely
//! break an ingest, with enough tolerance that a correctly-configured
//! stream never trips it.
//!
//! Everything is a pure function of (measured params, destinations), so
//! the rules are unit-testable without a live stream.

use crate::config::{platform_label, Destination};
use crate::h264::VideoCodec;

/// Measured parameters of the current publisher session. Zeroed between
/// sessions by `Controller::reset_codec_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    /// Primary-track pixel dimensions, 0 when not decoded yet (non-AVC
    /// codecs and multi-track sequence headers are not parsed).
    pub width: u32,
    pub height: u32,
    /// Mean IDR spacing in ms, 0 until enough keyframes have been seen.
    pub keyframe_interval_ms: u32,
    pub codec: VideoCodec,
}

/// Warn above this measured IDR spacing. Every ingest we support asks for
/// a 2 s keyframe interval; the 0.6 s of headroom absorbs encoder jitter
/// and the occasional long GOP so a correctly-configured 2 s stream (which
/// measures ~2.0-2.1 s in practice) never trips the check. OBS's "auto"
/// setting typically lands at 4 s, which is what this is here to catch.
const KEYFRAME_WARN_MS: u32 = 2_600;

/// Platforms that document a 2 s keyframe interval. "custom" is absent on
/// purpose - we have no idea what a user's own ingest wants, and guessing
/// would produce exactly the false positive this module is built to avoid.
const WANTS_2S_KEYFRAME: &[&str] = &["twitch", "youtube", "kick", "trovo", "restream"];

/// Kick's ingest ladder tops out at 1080p. Sending a taller canvas is
/// accepted but downscaled server-side at best, rejected at worst.
const KICK_MAX_HEIGHT: u32 = 1080;

/// Produce at most one line describing every compatibility problem with
/// the current stream, or `None` when there is nothing worth saying.
///
/// Only *enabled* destinations are considered - there is no reason to warn
/// about Kick's resolution cap for someone who does not stream to Kick.
/// The local test sink is skipped: it accepts anything by design.
pub fn compat_warning(params: &StreamParams, destinations: &[Destination]) -> Option<String> {
    let live: Vec<&Destination> = destinations
        .iter()
        .filter(|d| d.enabled && d.platform != "sink")
        .collect();
    if live.is_empty() {
        return None;
    }

    let issues: Vec<String> = [
        keyframe_issue(params, &live),
        resolution_issue(params, &live),
        codec_issue(params, &live),
    ]
    .into_iter()
    .flatten()
    .collect();

    if issues.is_empty() {
        None
    } else {
        Some(issues.join(" · "))
    }
}

/// IDR spacing well past the 2 s every platform asks for. This is the
/// check that earns its keep: a 4 s interval causes visible rebuffering
/// on the viewer side and breaks low-latency modes, and OBS ships it by
/// default under "auto".
fn keyframe_issue(params: &StreamParams, live: &[&Destination]) -> Option<String> {
    if params.keyframe_interval_ms == 0 || params.keyframe_interval_ms <= KEYFRAME_WARN_MS {
        return None;
    }
    let names = labels_matching(live, |slug| WANTS_2S_KEYFRAME.contains(&slug));
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "Keyframe interval measures {}. {} {} 2s.",
        format_seconds(params.keyframe_interval_ms),
        join_names(&names),
        verb_for(names.len(), "expects", "expect"),
    ))
}

/// Canvas taller than a destination's ceiling. Only Kick states a hard cap
/// we can rely on, so it is the only platform checked here.
fn resolution_issue(params: &StreamParams, live: &[&Destination]) -> Option<String> {
    if params.height == 0 || params.height <= KICK_MAX_HEIGHT {
        return None;
    }
    if !live.iter().any(|d| d.platform == "kick") {
        return None;
    }
    Some(format!(
        "Video is {}p. Kick caps at {}p.",
        params.height, KICK_MAX_HEIGHT
    ))
}

/// A codec the destination's ingest cannot decode.
///
/// Scoped to Kick on purpose. Twitch negotiates codecs itself under
/// Enhanced Broadcasting, and YouTube has added HEVC ingest paths, so
/// warning about either would risk a false positive. Kick's ingest is
/// H.264 and the failure is total, which makes it worth naming.
fn codec_issue(params: &StreamParams, live: &[&Destination]) -> Option<String> {
    if !matches!(
        params.codec,
        VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9
    ) {
        return None;
    }
    if !live.iter().any(|d| d.platform == "kick") {
        return None;
    }
    Some(format!(
        "Video codec is {}. Kick needs H.264.",
        params.codec.label()
    ))
}

/// Distinct platform labels among `live` whose slug satisfies `pred`,
/// in first-seen order. Deduplicated so two Twitch destinations don't
/// render as "Twitch and Twitch".
fn labels_matching(live: &[&Destination], pred: impl Fn(&str) -> bool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for d in live {
        if !pred(d.platform.as_str()) {
            continue;
        }
        let label = platform_label(&d.platform);
        if !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

/// "Kick", "Kick and YouTube", "Kick, YouTube and Trovo".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

fn verb_for(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// 4000 -> "4.0s". One decimal is enough to make "you set 4 s" obvious
/// without implying we measured to the millisecond.
fn format_seconds(ms: u32) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(platform: &str, enabled: bool) -> Destination {
        Destination {
            id: platform.into(),
            name: platform.into(),
            enabled,
            platform: platform.into(),
            stream_key: "k".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        }
    }

    fn params() -> StreamParams {
        StreamParams {
            width: 1920,
            height: 1080,
            keyframe_interval_ms: 2_000,
            codec: VideoCodec::Avc,
        }
    }

    #[test]
    fn healthy_stream_is_silent() {
        let dests = vec![dest("twitch", true), dest("kick", true)];
        assert_eq!(compat_warning(&params(), &dests), None);
    }

    #[test]
    fn no_destinations_is_silent() {
        assert_eq!(compat_warning(&params(), &[]), None);
    }

    /// The whole point of the tolerance: a user who correctly set 2 s
    /// measures slightly over it because of encoder jitter, and must not
    /// be nagged for it.
    #[test]
    fn two_second_keyframe_with_jitter_does_not_warn() {
        let mut p = params();
        p.keyframe_interval_ms = 2_100;
        assert_eq!(compat_warning(&p, &[dest("kick", true)]), None);
    }

    #[test]
    fn four_second_keyframe_warns_and_names_platforms() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        let dests = vec![dest("kick", true), dest("youtube", true)];
        assert_eq!(
            compat_warning(&p, &dests),
            Some("Keyframe interval measures 4.0s. Kick and YouTube expect 2s.".into())
        );
    }

    #[test]
    fn single_platform_uses_singular_verb() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        assert_eq!(
            compat_warning(&p, &[dest("kick", true)]),
            Some("Keyframe interval measures 4.0s. Kick expects 2s.".into())
        );
    }

    /// Two destinations on the same platform must not render as
    /// "Twitch and Twitch".
    #[test]
    fn duplicate_platforms_are_deduplicated() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        let mut second = dest("twitch", true);
        second.id = "twitch-2".into();
        let dests = vec![dest("twitch", true), second];
        assert_eq!(
            compat_warning(&p, &dests),
            Some("Keyframe interval measures 4.0s. Twitch expects 2s.".into())
        );
    }

    #[test]
    fn disabled_destinations_are_ignored() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        assert_eq!(compat_warning(&p, &[dest("kick", false)]), None);
    }

    /// The sink accepts anything - warning about it would be noise.
    #[test]
    fn sink_alone_never_warns() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        p.height = 1440;
        p.codec = VideoCodec::Av1;
        assert_eq!(compat_warning(&p, &[dest("sink", true)]), None);
    }

    /// A custom ingest has unknown requirements, so guessing is worse
    /// than staying quiet.
    #[test]
    fn custom_platform_has_no_keyframe_opinion() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        assert_eq!(compat_warning(&p, &[dest("custom", true)]), None);
    }

    #[test]
    fn unmeasured_keyframe_interval_is_silent() {
        let mut p = params();
        p.keyframe_interval_ms = 0;
        assert_eq!(compat_warning(&p, &[dest("kick", true)]), None);
    }

    #[test]
    fn tall_canvas_warns_only_when_kick_is_enabled() {
        let mut p = params();
        p.height = 1440;
        assert_eq!(compat_warning(&p, &[dest("twitch", true)]), None);
        assert_eq!(
            compat_warning(&p, &[dest("kick", true)]),
            Some("Video is 1440p. Kick caps at 1080p.".into())
        );
    }

    #[test]
    fn undecoded_resolution_is_silent() {
        let mut p = params();
        p.height = 0;
        assert_eq!(compat_warning(&p, &[dest("kick", true)]), None);
    }

    #[test]
    fn non_h264_warns_for_kick_only() {
        let mut p = params();
        p.codec = VideoCodec::Hevc;
        assert_eq!(compat_warning(&p, &[dest("youtube", true)]), None);
        assert_eq!(
            compat_warning(&p, &[dest("kick", true)]),
            Some("Video codec is HEVC. Kick needs H.264.".into())
        );
    }

    #[test]
    fn unknown_codec_is_silent() {
        let mut p = params();
        p.codec = VideoCodec::Unknown;
        assert_eq!(compat_warning(&p, &[dest("kick", true)]), None);
    }

    /// Multiple problems collapse into ONE line - the clutter guard.
    #[test]
    fn multiple_issues_join_into_a_single_line() {
        let mut p = params();
        p.keyframe_interval_ms = 4_000;
        p.height = 1440;
        p.codec = VideoCodec::Av1;
        let warning = compat_warning(&p, &[dest("kick", true)]).unwrap();
        assert_eq!(
            warning,
            "Keyframe interval measures 4.0s. Kick expects 2s. · \
             Video is 1440p. Kick caps at 1080p. · \
             Video codec is AV1. Kick needs H.264."
        );
        assert_eq!(warning.lines().count(), 1);
    }

    #[test]
    fn join_names_reads_naturally() {
        assert_eq!(join_names(&["Kick"]), "Kick");
        assert_eq!(join_names(&["Kick", "YouTube"]), "Kick and YouTube");
        assert_eq!(
            join_names(&["Kick", "YouTube", "Trovo"]),
            "Kick, YouTube and Trovo"
        );
    }
}
