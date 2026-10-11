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

/// Where each fix lives in OBS, so the warning is something to act on. The
/// dashboard adds that OBS applies it the next time streaming starts.
const KEYFRAME_FIX: &str =
    "In OBS: Settings → Output → Streaming (Output Mode: Advanced) → Keyframe Interval: 2 s.";
const RESOLUTION_FIX: &str =
    "In OBS: Settings → Video → Output (Scaled) Resolution: 1080p or lower.";
const CODEC_FIX: &str = "In OBS: Settings → Output → Streaming → Video Encoder: an H.264 one.";

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
        "Keyframe interval measures {}. {} {} 2s. {KEYFRAME_FIX}",
        format_seconds(params.keyframe_interval_ms),
        join_names(&names),
        verb_for(names.len(), "expects", "expect"),
    ))
}

/// Canvas past a destination's ceiling. Only Kick states a hard cap we can
/// rely on, so it is the only platform checked here.
///
/// Measured on the short side, the way "1080p" is meant: a 1080x1920
/// portrait canvas is 1080p, not "1920p". The height alone stands in when
/// the width is not decoded.
fn resolution_issue(params: &StreamParams, live: &[&Destination]) -> Option<String> {
    let short_side = if params.width == 0 {
        params.height
    } else {
        params.width.min(params.height)
    };
    if short_side == 0 || short_side <= KICK_MAX_HEIGHT {
        return None;
    }
    if !live.iter().any(|d| d.platform == "kick") {
        return None;
    }
    Some(format!(
        "Video is {}p. Kick caps at {}p. {RESOLUTION_FIX}",
        short_side, KICK_MAX_HEIGHT
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
        "Video codec is {}. Kick needs H.264. {CODEC_FIX}",
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

    /// A healthy 1080p, 2 s keyframe, H.264 stream, changed by `change`.
    fn stream(change: impl Fn(&mut StreamParams)) -> StreamParams {
        let mut params = StreamParams {
            width: 1920,
            height: 1080,
            keyframe_interval_ms: 2_000,
            codec: VideoCodec::Avc,
        };
        change(&mut params);
        params
    }

    fn healthy() -> StreamParams {
        stream(|_| {})
    }

    fn long_keyframes() -> StreamParams {
        stream(|p| p.keyframe_interval_ms = 4_000)
    }

    /// Nothing is said when the stream is fine, when the measurement isn't
    /// in yet, or when nobody who would mind is being streamed to.
    #[test]
    fn stays_silent_when_nothing_is_wrong_or_nobody_minds() {
        let kick = [dest("kick", true)];
        let cases: [(&str, StreamParams, &[Destination]); 13] = [
            (
                "a healthy stream",
                healthy(),
                &[dest("twitch", true), dest("kick", true)],
            ),
            ("no destinations", healthy(), &[]),
            // The whole point of the tolerance: a user who correctly set 2 s
            // measures slightly over it because of encoder jitter.
            (
                "2 s with encoder jitter",
                stream(|p| p.keyframe_interval_ms = 2_100),
                &kick,
            ),
            // Pins the threshold: 2600 ms is the last silent value, so moving
            // it is a deliberate edit to KEYFRAME_WARN_MS, not a `<=` slip.
            (
                "the last silent interval",
                stream(|p| p.keyframe_interval_ms = 2_600),
                &kick,
            ),
            (
                "an interval not measured yet",
                stream(|p| p.keyframe_interval_ms = 0),
                &kick,
            ),
            (
                "a switched-off destination",
                long_keyframes(),
                &[dest("kick", false)],
            ),
            // A custom ingest has unknown requirements: guessing is worse.
            ("a custom server", long_keyframes(), &[dest("custom", true)]),
            // The sink accepts anything: warning about it would be noise.
            (
                "the test sink, with everything wrong",
                stream(|p| {
                    p.keyframe_interval_ms = 4_000;
                    p.height = 1440;
                    p.codec = VideoCodec::Av1;
                }),
                &[dest("sink", true)],
            ),
            (
                "1440p to Twitch only",
                stream(|p| p.height = 1440),
                &[dest("twitch", true)],
            ),
            // A 9:16 canvas is 1080p on its short side, which Kick accepts.
            // Reading the height alone called it "1920p" and warned falsely.
            (
                "a portrait 1080p canvas",
                stream(|p| (p.width, p.height) = (1080, 1920)),
                &kick,
            ),
            ("a size not decoded yet", stream(|p| p.height = 0), &kick),
            (
                "HEVC to YouTube only",
                stream(|p| p.codec = VideoCodec::Hevc),
                &[dest("youtube", true)],
            ),
            (
                "a codec not known yet",
                stream(|p| p.codec = VideoCodec::Unknown),
                &kick,
            ),
        ];
        for (case, params, dests) in cases {
            assert_eq!(compat_warning(&params, dests), None, "{case}");
        }
    }

    /// The keyframe warning starts just past the threshold and names each
    /// platform that expects 2 s once, however many destinations use it.
    #[test]
    fn a_long_keyframe_interval_names_who_expects_2s() {
        let mut second_twitch = dest("twitch", true);
        second_twitch.id = "twitch-2".into();
        let cases: [(StreamParams, &[Destination], &str); 3] = [
            (
                long_keyframes(),
                &[dest("kick", true), dest("youtube", true)],
                "Keyframe interval measures 4.0s. Kick and YouTube expect 2s.",
            ),
            (
                long_keyframes(),
                &[dest("twitch", true), second_twitch],
                "Keyframe interval measures 4.0s. Twitch expects 2s.",
            ),
            (
                stream(|p| p.keyframe_interval_ms = 2_601),
                &[dest("kick", true)],
                "Keyframe interval measures 2.6s. Kick expects 2s.",
            ),
        ];
        for (params, dests, said) in cases {
            assert_eq!(
                compat_warning(&params, dests),
                Some(format!("{said} {KEYFRAME_FIX}"))
            );
        }
    }

    /// Kick caps video at 1080p on its short side, whichever way up and
    /// whichever side was decoded, and takes H.264 only.
    #[test]
    fn kick_is_warned_about_tall_video_and_other_codecs() {
        let too_tall = format!("Video is 1440p. Kick caps at 1080p. {RESOLUTION_FIX}");
        let cases = [
            ("1440p", stream(|p| p.height = 1440), too_tall.clone()),
            (
                "portrait 1440p",
                stream(|p| (p.width, p.height) = (1440, 2560)),
                too_tall.clone(),
            ),
            (
                "a height without a width",
                stream(|p| (p.width, p.height) = (0, 1440)),
                too_tall,
            ),
            (
                "HEVC",
                stream(|p| p.codec = VideoCodec::Hevc),
                format!("Video codec is HEVC. Kick needs H.264. {CODEC_FIX}"),
            ),
        ];
        for (case, params, said) in cases {
            assert_eq!(
                compat_warning(&params, &[dest("kick", true)]),
                Some(said),
                "{case}"
            );
        }
    }

    /// Multiple problems collapse into ONE line - the clutter guard.
    #[test]
    fn multiple_issues_join_into_a_single_line() {
        let everything = stream(|p| {
            p.keyframe_interval_ms = 4_000;
            p.height = 1440;
            p.codec = VideoCodec::Av1;
        });
        let warning = compat_warning(&everything, &[dest("kick", true)]).unwrap();
        assert_eq!(
            warning,
            format!(
                "Keyframe interval measures 4.0s. Kick expects 2s. {KEYFRAME_FIX} · \
                 Video is 1440p. Kick caps at 1080p. {RESOLUTION_FIX} · \
                 Video codec is AV1. Kick needs H.264. {CODEC_FIX}"
            )
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
