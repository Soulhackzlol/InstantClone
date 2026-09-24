//! Enhanced Broadcasting config built locally, for streams with no Twitch
//! destination.
//!
//! OBS asks the service's config URL which tracks to encode. With a Twitch
//! destination `InstantClone` proxies Twitch's answer; without one there is
//! no one to ask, so the answer is built here from what OBS reports about
//! its own canvases (`preferences.canvases` in the request body):
//!
//! - one track for the main canvas, at the Output resolution the streamer
//!   picked in OBS (Settings > Video), since only one horizontal track is
//!   ever forwarded to a non-Twitch platform;
//! - one track for OBS's "Additional canvas" (Settings > Stream > Enhanced
//!   Broadcasting), usually a vertical canvas such as Aitum Vertical, when
//!   OBS sends one. This is what feeds destinations set to Vertical.
//!
//! A track must never name a canvas OBS didn't send: OBS refuses to start
//! the stream on an out-of-range `canvas_index`.

/// One entry of OBS's `preferences.canvases`: the size OBS outputs that
/// canvas at, and its frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObsCanvas {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl ObsCanvas {
    pub fn is_vertical(&self) -> bool {
        self.height > self.width
    }
}

/// Share of the bandwidth budget the main track gets when a second canvas
/// is encoded too. Vertical feeds are smaller, so 60 / 40 keeps both sharp.
const MAIN_TRACK_SHARE_PCT: u64 = 60;
const MIN_TRACK_KBPS: u32 = 500;
/// Budget when nobody set one. The main track alone gets what a 1080p60
/// stream usually runs at; with a vertical track the 60 / 40 split keeps
/// the main track at that same 6000. Every kbps here leaves the streamer's
/// upload once per destination, so higher is not better.
const DEFAULT_MAIN_ONLY_KBPS: u32 = 6000;
const DEFAULT_WITH_ADDITIONAL_KBPS: u32 = 10000;
/// Lowest cap honoured: split 60/40 it gives 750 + 500, so neither track
/// drops under `MIN_TRACK_KBPS` and the total never exceeds the cap.
const MIN_BUDGET_KBPS: u32 = 1250;
const MAX_BUDGET_KBPS: u32 = 50000;

/// Default bandwidth budget for these canvases, in kbps.
pub fn default_budget_kbps(canvases: &[ObsCanvas]) -> u32 {
    if canvases.len() > 1 {
        DEFAULT_WITH_ADDITIONAL_KBPS
    } else {
        DEFAULT_MAIN_ONLY_KBPS
    }
}

/// The streamer's "Maximum Streaming Bandwidth" from OBS's Enhanced
/// Broadcasting settings, in kbps, when they set one (OBS sends null for
/// Auto).
pub fn requested_budget_kbps(body: &str) -> Option<u32> {
    number_field(body, "maximum_aggregate_bitrate")
        .filter(|kbps| *kbps > 0)
        .map(|kbps| kbps.clamp(MIN_BUDGET_KBPS, MAX_BUDGET_KBPS))
}

/// Whether an Enhanced Broadcasting config asks OBS to encode anything
/// from a second canvas (`"canvas_index": 1` or higher, any spacing).
pub fn names_additional_canvas(config: &str) -> bool {
    config
        .match_indices("\"canvas_index\"")
        .any(|(at, _)| number_field(&config[at..], "canvas_index").is_some_and(|i| i > 0))
}

/// Canvases OBS listed in its config request, in `canvas_index` order.
/// Empty when the body has none (older OBS, or a hand-made request).
pub fn parse_canvases(body: &str) -> Vec<ObsCanvas> {
    // A canvas's position is its canvas_index, so stop at the first one we
    // can't read instead of skipping it and shifting every later one down.
    array_objects(body, "canvases")
        .into_iter()
        .map_while(parse_canvas)
        .collect()
}

// PCI vendor ids, as OBS reports them in `capabilities.gpu[].vendor_id`.
const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD: u32 = 0x1002;
const VENDOR_INTEL: u32 = 0x8086;

/// Hardware encoder family ("nvenc", "amd" or "qsv") for the GPU OBS
/// composites on, from `capabilities.gpu` and
/// `preferences.composition_gpu_index`. None when OBS reported no GPU we
/// know an encoder for, and the caller falls back to x264.
///
/// This matters: several tracks of x264 at 1080p60 overload most CPUs,
/// which is what Enhanced Broadcasting without Twitch did before.
pub fn gpu_encoder_family(body: &str) -> Option<&'static str> {
    let gpus = array_objects(body, "gpu");
    let index = number_field(body, "composition_gpu_index").unwrap_or(0) as usize;
    let gpu = gpus.get(index).or_else(|| gpus.first())?;
    match number_field(gpu, "vendor_id")? {
        VENDOR_NVIDIA => Some("nvenc"),
        VENDOR_AMD => Some("amd"),
        VENDOR_INTEL => Some("qsv"),
        _ => None,
    }
}

/// The objects directly inside the array under `"key"`. Enough JSON for
/// OBS's request, whose arrays of interest hold flat numeric objects.
fn array_objects<'a>(body: &'a str, key: &str) -> Vec<&'a str> {
    let needle = format!("\"{key}\"");
    let Some(at) = body.find(&needle) else {
        return Vec::new();
    };
    let rest = &body[at + needle.len()..];
    let Some(open) = rest.trim_start().strip_prefix(':').map(str::trim_start) else {
        return Vec::new();
    };
    if !open.starts_with('[') {
        return Vec::new();
    }
    let mut objects = Vec::new();
    let mut depth = 0;
    let mut object_start = None;
    for (index, c) in open.char_indices() {
        match c {
            '[' | '{' => {
                depth += 1;
                if c == '{' && depth == 2 {
                    object_start = Some(index);
                }
            }
            ']' | '}' => {
                if c == '}' && depth == 2 {
                    if let Some(start) = object_start.take() {
                        objects.push(&open[start..=index]);
                    }
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    objects
}

fn parse_canvas(object: &str) -> Option<ObsCanvas> {
    let canvas = ObsCanvas {
        width: number_field(object, "width")?,
        height: number_field(object, "height")?,
        fps_num: number_field(object, "numerator").unwrap_or(30),
        fps_den: number_field(object, "denominator").unwrap_or(1).max(1),
    };
    (canvas.width > 0 && canvas.height > 0 && canvas.fps_num > 0).then_some(canvas)
}

/// The unsigned number after `"key":`. The quotes keep `"width"` from
/// matching `"canvas_width"`.
fn number_field(object: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\"");
    let after = &object[object.find(&needle)? + needle.len()..];
    let value = after.trim_start().strip_prefix(':')?.trim_start();
    let digits_end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    value[..digits_end].parse().ok()
}

/// Encoder id plus its settings JSON for one track.
pub struct TrackEncoder {
    pub encoder_type: &'static str,
    pub settings_json: String,
}

/// What the local config asks OBS to encode.
pub struct LocalPlan {
    pub main: ObsCanvas,
    pub additional: Option<ObsCanvas>,
}

impl LocalPlan {
    /// Whether a destination set to Vertical has a 9:16 track to forward.
    /// A portrait main canvas counts too.
    pub fn feeds_vertical(&self) -> bool {
        self.main.is_vertical() || self.additional.is_some_and(|c| c.is_vertical())
    }

    /// "1920x1080 from the main canvas + 1080x1920 from OBS's Additional
    /// canvas", for the log.
    pub fn describe(&self) -> String {
        let main = format!(
            "{}x{} from the main canvas",
            self.main.width, self.main.height
        );
        match self.additional {
            Some(c) => format!(
                "{main} + {}x{} from OBS's Additional canvas",
                c.width, c.height
            ),
            None => main,
        }
    }
}

/// The config JSON, or None when OBS sent no canvases (the caller then
/// keeps the static ladder). `encoder_for(kbps)` supplies the chosen
/// encoder's id and settings at a given bitrate.
pub fn build(
    canvases: &[ObsCanvas],
    bandwidth_kbps: u32,
    ingest_port: u16,
    encoder_for: impl Fn(u32) -> TrackEncoder,
) -> Option<(String, LocalPlan)> {
    let main = *canvases.first()?;
    let additional = canvases.get(1).copied();
    let main_kbps = match additional {
        Some(_) => share(bandwidth_kbps, MAIN_TRACK_SHARE_PCT),
        None => bandwidth_kbps.max(MIN_TRACK_KBPS),
    };
    let mut tracks = vec![track_json(&main, 0, &encoder_for(main_kbps))];
    if let Some(canvas) = additional {
        let kbps = share(bandwidth_kbps, 100 - MAIN_TRACK_SHARE_PCT);
        tracks.push(track_json(&canvas, 1, &encoder_for(kbps)));
    }
    let config = format!(
        r#"{{"meta":{{"service":"InstantClone","schema_version":"2024-06-04","config_id":"instantclone-local-{id}"}},"ingest_endpoints":[{{"protocol":"RTMP","url_template":"rtmp://localhost:{port}/live/{{stream_key}}"}}],"encoder_configurations":[{tracks}],"audio_configurations":{{"live":[{{"codec":"aac","track_id":0,"channels":2,"settings":{{"bitrate":160}}}}]}}}}"#,
        id = unix_seconds(),
        port = ingest_port,
        tracks = tracks.join(","),
    );
    Some((config, LocalPlan { main, additional }))
}

fn track_json(canvas: &ObsCanvas, canvas_index: u32, encoder: &TrackEncoder) -> String {
    format!(
        r#"{{"type":"{enc}","width":{w},"height":{h},"framerate":{{"numerator":{num},"denominator":{den}}},"canvas_index":{idx},"settings":{s}}}"#,
        enc = encoder.encoder_type,
        w = canvas.width,
        h = canvas.height,
        num = canvas.fps_num,
        den = canvas.fps_den,
        idx = canvas_index,
        s = encoder.settings_json,
    )
}

fn share(total_kbps: u32, percent: u64) -> u32 {
    // At most 100% of a u32, so it always fits back.
    u32::try_from(u64::from(total_kbps) * percent / 100)
        .unwrap_or(total_kbps)
        .max(MIN_TRACK_KBPS)
}

/// OBS treats each `config_id` as a fresh session, like Twitch's.
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real OBS 32.2 request: main canvas 2560x1440 output
    /// at 1920x1080, plus an Aitum Vertical canvas at 1080x1920.
    const REQUEST: &str = r#"{"service":"IVS","schema_version":"2025-01-25","authentication":"key",
        "preferences":{"vod_track_audio":true,"composition_gpu_index":0,"audio_samples_per_sec":48000,
        "canvases":[
            {"width":1920,"height":1080,"canvas_width":2560,"canvas_height":1440,"framerate":{"numerator":60,"denominator":1}},
            {"width":1080,"height":1920,"canvas_width":1080,"canvas_height":1920,"framerate":{"numerator":30,"denominator":1}}
        ]}}"#;

    fn x264(kbps: u32) -> TrackEncoder {
        TrackEncoder {
            encoder_type: "obs_x264",
            settings_json: format!(r#"{{"bitrate":{kbps}}}"#),
        }
    }

    #[test]
    fn reads_output_size_and_frame_rate_of_each_canvas() {
        let canvases = parse_canvases(REQUEST);
        assert_eq!(
            canvases,
            vec![
                ObsCanvas {
                    width: 1920,
                    height: 1080,
                    fps_num: 60,
                    fps_den: 1
                },
                ObsCanvas {
                    width: 1080,
                    height: 1920,
                    fps_num: 30,
                    fps_den: 1
                },
            ]
        );
        assert!(canvases[1].is_vertical());
    }

    #[test]
    fn picks_the_hardware_encoder_of_the_composition_gpu() {
        let nvidia = r#"{"capabilities":{"gpu":[{"model":"RTX 4070","vendor_id":4318,"device_id":10114}]},"preferences":{"composition_gpu_index":0}}"#;
        assert_eq!(gpu_encoder_family(nvidia), Some("nvenc"));
        // Laptop: Intel iGPU listed first, OBS composites on the NVIDIA one.
        let hybrid = r#"{"capabilities":{"gpu":[{"vendor_id":32902},{"vendor_id":4318}]},"preferences":{"composition_gpu_index":1}}"#;
        assert_eq!(gpu_encoder_family(hybrid), Some("nvenc"));
        let amd = r#"{"capabilities":{"gpu":[{"vendor_id":4098}]}}"#;
        assert_eq!(gpu_encoder_family(amd), Some("amd"));
    }

    #[test]
    fn unknown_or_missing_gpu_means_software() {
        assert_eq!(
            gpu_encoder_family(r#"{"capabilities":{"gpu":[{"vendor_id":5140}]}}"#),
            None
        );
        assert_eq!(gpu_encoder_family(r#"{"capabilities":{}}"#), None);
        // An index past the list falls back to the first GPU.
        let stale = r#"{"capabilities":{"gpu":[{"vendor_id":4318}]},"preferences":{"composition_gpu_index":3}}"#;
        assert_eq!(gpu_encoder_family(stale), Some("nvenc"));
    }

    #[test]
    fn an_unreadable_canvas_never_shifts_the_ones_after_it() {
        let body = r#"{"canvases":[{"width":-1,"height":1080},{"width":1080,"height":1920}]}"#;
        assert!(
            parse_canvases(body).is_empty(),
            "canvas 0 unreadable: no local config"
        );
        let body = r#"{"canvases":[{"width":1920,"height":1080},{"width":"x"},{"width":1080,"height":1920}]}"#;
        assert_eq!(parse_canvases(body).len(), 1, "stops before the bad one");
    }

    #[test]
    fn requests_without_canvases_parse_to_nothing() {
        assert!(parse_canvases(r#"{"preferences":{}}"#).is_empty());
        assert!(parse_canvases(r#"{"preferences":{"canvases":[]}}"#).is_empty());
        assert!(parse_canvases("not json").is_empty());
    }

    #[test]
    fn two_canvases_give_one_track_each() {
        let (config, plan) = build(&parse_canvases(REQUEST), 10_000, 1935, x264).unwrap();
        assert_eq!(config.matches("\"canvas_index\"").count(), 2);
        assert!(config.contains(r#""width":1920,"height":1080,"framerate":{"numerator":60,"denominator":1},"canvas_index":0,"settings":{"bitrate":6000}"#));
        assert!(config.contains(r#""width":1080,"height":1920,"framerate":{"numerator":30,"denominator":1},"canvas_index":1,"settings":{"bitrate":4000}"#));
        assert!(config.contains("rtmp://localhost:1935/live/{stream_key}"));
        assert!(plan.additional.is_some());
    }

    #[test]
    fn one_canvas_never_names_a_second() {
        let only_main = &parse_canvases(REQUEST)[..1];
        let (config, plan) = build(only_main, 8_000, 1935, x264).unwrap();
        assert!(
            !config.contains(r#""canvas_index":1"#),
            "OBS would refuse to stream"
        );
        assert!(
            config.contains(r#""settings":{"bitrate":8000}"#),
            "the one track gets the whole budget"
        );
        assert!(plan.additional.is_none());
    }

    #[test]
    fn default_budget_keeps_the_main_track_at_6000() {
        let canvases = parse_canvases(REQUEST);
        assert_eq!(default_budget_kbps(&canvases[..1]), 6000);
        let (config, _) = build(&canvases, default_budget_kbps(&canvases), 1935, x264).unwrap();
        assert!(config.contains(r#""canvas_index":0,"settings":{"bitrate":6000}"#));
    }

    #[test]
    fn obs_bandwidth_cap_is_read_and_auto_is_ignored() {
        assert_eq!(
            requested_budget_kbps(r#"{"preferences":{"maximum_aggregate_bitrate":8000}}"#),
            Some(8000)
        );
        assert_eq!(
            requested_budget_kbps(r#"{"preferences":{"maximum_aggregate_bitrate":null}}"#),
            None
        );
        assert_eq!(
            requested_budget_kbps(r#"{"preferences":{"maximum_aggregate_bitrate":100}}"#),
            Some(1250)
        );
    }

    #[test]
    fn spots_a_second_canvas_in_any_config() {
        assert!(names_additional_canvas(
            r#"[{"canvas_index": 0},{"canvas_index" : 1}]"#
        ));
        assert!(!names_additional_canvas(
            r#"[{"canvas_index":0},{"canvas_index":0}]"#
        ));
        assert!(!names_additional_canvas(r#"{"encoder_configurations":[]}"#));
    }

    #[test]
    fn plan_says_what_it_encodes_and_whether_it_feeds_vertical() {
        let canvases = parse_canvases(REQUEST);
        let (_, both) = build(&canvases, 10_000, 1935, x264).unwrap();
        assert!(both.feeds_vertical());
        assert_eq!(
            both.describe(),
            "1920x1080 from the main canvas + 1080x1920 from OBS's Additional canvas"
        );
        let (_, main_only) = build(&canvases[..1], 6_000, 1935, x264).unwrap();
        assert!(!main_only.feeds_vertical());
        assert_eq!(main_only.describe(), "1920x1080 from the main canvas");
        let (_, portrait_main) = build(&canvases[1..], 6_000, 1935, x264).unwrap();
        assert!(
            portrait_main.feeds_vertical(),
            "a portrait main canvas counts"
        );
    }

    #[test]
    fn no_canvases_means_no_local_config() {
        assert!(build(&[], 8_000, 1935, x264).is_none());
    }
}
