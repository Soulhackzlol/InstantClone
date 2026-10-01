//! Message templates: `{name}` inserts a value, `{name|text}` inserts
//! `text` instead when the value is empty or 0, and `{{name}}` writes a
//! literal `{name}`.
//!
//! Anything in braces that is not a variable name stays as written, so a
//! JSON body like `{"event":"{event}"}` needs no escaping: `{"` can never
//! start a name. An unknown variable renders empty rather than leaking
//! `{typo}` into a chat; the editor flags unknown names before they are
//! saved.

/// Where a template's values come from.
pub trait Vars {
    fn lookup(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> Vars for F {
    fn lookup(&self, name: &str) -> Option<String> {
        self(name)
    }
}

/// Longest variable name accepted; keeps a stray brace from scanning a
/// whole message.
const MAX_NAME: usize = 64;

pub fn render(template: &str, vars: &dyn Vars) -> String {
    render_escaped(template, vars, &|value| value.to_string())
}

/// `render`, passing every inserted value through `escape` first. Text the
/// user wrote, fallbacks included, is left as written: only values (which
/// can come from chat or a web answer) are escaped.
pub fn render_escaped(template: &str, vars: &dyn Vars, escape: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(len) = escaped(tail) {
            out.push_str(&tail[1..len - 1]);
            rest = &tail[len..];
            continue;
        }
        match placeholder(tail) {
            Some((name, fallback, len)) => {
                let value = vars.lookup(name).unwrap_or_default();
                match fallback {
                    Some(text) if is_blank(&value) => out.push_str(text),
                    _ => out.push_str(&escape(&value)),
                }
                rest = &tail[len..];
            }
            None => {
                out.push_str(&tail[..1]);
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Percent-encode a value for a URL path or query: everything but the
/// unreserved characters, so `!rank ana&key=x` stays one value.
pub fn url_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Escape a value for the inside of a JSON string, so a quote typed in chat
/// can't add fields to a JSON body.
pub fn json_string_content(value: &str) -> String {
    let mut quoted = String::new();
    crate::json::write_string(value, &mut quoted);
    quoted[1..quoted.len() - 1].to_string()
}

/// One line: no control characters, so a value can't start a new header.
pub fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Every variable name a template uses, in order, without repeats.
pub fn names(template: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let tail = &rest[i..];
        if let Some(len) = escaped(tail) {
            rest = &tail[len..];
            continue;
        }
        match placeholder(tail) {
            Some((name, _, len)) => {
                if !found.iter().any(|n| n == name) {
                    found.push(name.to_string());
                }
                rest = &tail[len..];
            }
            None => rest = &tail[1..],
        }
    }
    found
}

/// Length of a `{{name}}` escape at the start of `s`, braces included.
fn escaped(s: &str) -> Option<usize> {
    let (_, _, len) = placeholder(s.strip_prefix('{')?)?;
    s[1 + len..].starts_with('}').then_some(len + 2)
}

/// A value the `|fallback` form replaces.
fn is_blank(value: &str) -> bool {
    value.is_empty() || value == "0"
}

/// Parse `{name}` or `{name|fallback}` at the start of `s`. Returns the
/// name, the fallback and the byte length consumed.
fn placeholder(s: &str) -> Option<(&str, Option<&str>, usize)> {
    let body = s.strip_prefix('{')?;
    let name_len = body
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
        .count();
    if name_len == 0 || name_len > MAX_NAME {
        return None;
    }
    let name = &body[..name_len];
    let after = &body[name_len..];
    if after.starts_with('}') {
        return Some((name, None, 1 + name_len + 1));
    }
    let fallback_body = after.strip_prefix('|')?;
    let end = fallback_body.find('}')?;
    let fallback = &fallback_body[..end];
    Some((name, Some(fallback), 1 + name_len + 1 + end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str) -> Option<String> {
        match name {
            "delay" => Some("30s".into()),
            "off" => Some(String::new()),
            "zero" => Some("0".into()),
            "clip.url" => Some("https://clips.twitch.tv/x".into()),
            _ => None,
        }
    }

    #[test]
    fn inserts_values() {
        assert_eq!(render("Delay: {delay}", &vars), "Delay: 30s");
        assert_eq!(render("{clip.url}!", &vars), "https://clips.twitch.tv/x!");
    }

    #[test]
    fn fallback_covers_empty_and_zero() {
        assert_eq!(render("Delay: {off|off}", &vars), "Delay: off");
        assert_eq!(render("{zero|none}", &vars), "none");
        assert_eq!(render("{delay|off}", &vars), "30s");
        assert_eq!(render("{missing|n/a}", &vars), "n/a");
    }

    #[test]
    fn unknown_names_render_empty() {
        assert_eq!(render("a{nope}b", &vars), "ab");
    }

    #[test]
    fn json_bodies_pass_through() {
        let body = r#"{"event":"{delay}","n":{"x":1}}"#;
        assert_eq!(render(body, &vars), r#"{"event":"30s","n":{"x":1}}"#);
    }

    #[test]
    fn doubled_braces_write_a_literal_placeholder() {
        assert_eq!(render("{{delay}} is {delay}", &vars), "{delay} is 30s");
        assert_eq!(render("a }} b {{ c", &vars), "a }} b {{ c");
    }

    #[test]
    fn unterminated_braces_stay_as_written() {
        assert_eq!(render("{delay", &vars), "{delay");
        assert_eq!(render("{delay|off", &vars), "{delay|off");
        assert_eq!(render("{ spaced }", &vars), "{ spaced }");
    }

    #[test]
    fn escaping_touches_values_only() {
        let vars = |name: &str| (name == "q").then(|| "a b&c=d/é".to_string());
        assert_eq!(
            render_escaped("x?q={q}&r={none|a b}", &vars, &url_component),
            "x?q=a%20b%26c%3Dd%2F%C3%A9&r=a b"
        );
        let quote = |name: &str| (name == "q").then(|| "a\",\"admin\":true".to_string());
        assert_eq!(
            render_escaped(r#"{"n":"{q}"}"#, &quote, &json_string_content),
            r#"{"n":"a\",\"admin\":true"}"#
        );
        assert_eq!(one_line("a\r\nX-Evil: 1"), "a  X-Evil: 1");
    }

    #[test]
    fn lists_names_once_in_order() {
        assert_eq!(
            names("{b} {a|x} {b} {{c}} {\"j\":1}"),
            vec!["b".to_string(), "a".to_string()]
        );
    }
}
