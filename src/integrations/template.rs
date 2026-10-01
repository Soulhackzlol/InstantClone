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

/// What an "Edit text" step does to its input, for answers that need a
/// little shaping before they go out: a web answer's first line, the part
/// between two markers, a rounded number. `a` and `b` are the operation's
/// two settings (what to find and what to put instead, a length, decimals).
/// Unknown operations leave the text as it is.
pub fn edit_text(op: &str, input: &str, a: &str, b: &str) -> String {
    match op {
        "first_line" => input
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string(),
        "between" => between(input, a, b).unwrap_or("").trim().to_string(),
        "replace" if !a.is_empty() => input.replace(a, b),
        "cut" => {
            let max = a.trim().parse::<usize>().unwrap_or(100).max(1);
            if input.chars().count() <= max {
                input.to_string()
            } else {
                let kept: String = input.chars().take(max).collect();
                format!("{}…", kept.trim_end())
            }
        }
        "round" => match input.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => {
                let decimals = a.trim().parse::<usize>().unwrap_or(0).min(6);
                format!("{n:.decimals$}")
            }
            _ => input.to_string(),
        },
        "digits" => group_digits(input.trim()).unwrap_or_else(|| input.to_string()),
        "upper" => input.to_uppercase(),
        "lower" => input.to_lowercase(),
        "random" => random_choice(input),
        _ => input.to_string(),
    }
}

/// The text after the first `start` and before the next `end`. A blank
/// `start` means from the beginning, a blank `end` to the end.
fn between<'a>(input: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = if start.is_empty() {
        0
    } else {
        input.find(start)? + start.len()
    };
    let rest = &input[from..];
    let to = if end.is_empty() {
        rest.len()
    } else {
        rest.find(end)?
    };
    Some(&rest[..to])
}

/// `12571578` as `12,571,578`; `None` for anything but a whole number.
fn group_digits(text: &str) -> Option<String> {
    let (sign, digits) = match text.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", text),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    Some(format!("{sign}{out}"))
}

/// One of the input's lines, or of its `|`-separated parts when it is a
/// single line: an 8-ball answer, a random greeting.
fn random_choice(input: &str) -> String {
    let separator = if input.contains('\n') { '\n' } else { '|' };
    let parts: Vec<&str> = input
        .split(separator)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    let mut bytes = [0u8; 4];
    crate::crypto::os_random(&mut bytes);
    parts[u32::from_le_bytes(bytes) as usize % parts.len()].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_text_shapes_answers() {
        assert_eq!(
            edit_text("first_line", "\n  Radiant \n450 RR", "", ""),
            "Radiant"
        );
        assert_eq!(
            edit_text("between", "rank: Gold 2 (34 RR)", "rank:", "("),
            "Gold 2"
        );
        assert_eq!(edit_text("between", "a=1&b=2", "b=", ""), "2");
        assert_eq!(edit_text("between", "no marker", "x=", ""), "");
        assert_eq!(edit_text("replace", "a-b-c", "-", " "), "a b c");
        assert_eq!(edit_text("replace", "abc", "", "x"), "abc");
        assert_eq!(edit_text("cut", "hello world", "5", ""), "hello…");
        assert_eq!(edit_text("cut", "hi", "5", ""), "hi");
        assert_eq!(edit_text("round", " 3.14159 ", "2", ""), "3.14");
        assert_eq!(edit_text("round", "2.5e1", "", ""), "25");
        assert_eq!(edit_text("round", "n/a", "2", ""), "n/a");
        assert_eq!(edit_text("digits", "12571578", "", ""), "12,571,578");
        assert_eq!(edit_text("digits", "-1000", "", ""), "-1,000");
        assert_eq!(edit_text("digits", "999", "", ""), "999");
        assert_eq!(edit_text("digits", "1.5", "", ""), "1.5");
        assert_eq!(edit_text("upper", "gg", "", ""), "GG");
        assert_eq!(edit_text("nope", "same", "", ""), "same");
    }

    #[test]
    fn random_picks_one_of_the_parts() {
        for _ in 0..20 {
            let pick = edit_text("random", "Yes | No | Ask again", "", "");
            assert!(
                ["Yes", "No", "Ask again"].contains(&pick.as_str()),
                "{pick}"
            );
        }
        assert_eq!(edit_text("random", "only\n\n", "", ""), "only");
        assert_eq!(edit_text("random", " | ", "", ""), "");
    }

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
