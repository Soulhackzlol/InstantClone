//! Turn a chat bot command from Nightbot, StreamElements, Fossabot or
//! Streamlabs into the pieces of a "command from a website": the address it
//! fetches, the reply around the answer, and the command's name. Their
//! variables become ours: `$(user)` is `{user}`, `${1}` is `{arg1}`.
//!
//! Only commands that fetch an address convert. One that runs JavaScript
//! (`$(eval …)`) cannot, and says so; a variable with no match here is kept
//! as written and listed, so nothing is dropped silently.

/// Where the converted reply reads the fetched answer (the preset saves
/// its web request as `api`).
pub const ANSWER: &str = "{api.body}";

#[derive(Debug)]
pub struct Converted {
    /// `!name` when the paste was a whole "add command" line.
    pub command: Option<String>,
    pub url: String,
    /// The reply with the answer as `{api.body}`; `None` when only an
    /// address was pasted.
    pub reply: Option<String>,
    /// Which bot the syntax came from, for the dashboard's note.
    pub from: &'static str,
    /// Bot variables with no InstantClone match, kept as written.
    pub unknown: Vec<String>,
}

/// How each bot writes "fetch this address": (opening, closing, bot).
const FETCHES: &[(&str, u8, &str)] = &[
    ("$(urlfetch ", b')', "Nightbot"),
    ("$(customapi ", b')', "Fossabot"),
    ("${customapi.", b'}', "StreamElements"),
    ("{readapi.", b'}', "Streamlabs"),
    ("$readapi(", b')', "Streamlabs Chatbot"),
];

/// Chat lines that add or edit a command, before the command's name.
const ADD_COMMANDS: &[&str] = &[
    "!addcom",
    "!editcom",
    "!commands",
    "!command",
    "!cmd",
    "!addcmd",
    "!editcmd",
];

pub fn convert(input: &str) -> Result<Converted, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("paste a bot command or a web address".into());
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("$(eval") || lower.contains("${eval") {
        return Err(
            "this command runs JavaScript ($(eval)), which InstantClone doesn't. Paste just the address it calls"
                .into(),
        );
    }
    let (command, response) = split_command(text);
    let mut unknown = Vec::new();
    let Some(fetch) = find_fetch(response) else {
        let url = response.trim();
        if is_http(url) && !url.contains(char::is_whitespace) {
            let url = translate(url, &mut unknown);
            return Ok(Converted {
                command,
                url,
                reply: None,
                from: "a web address",
                unknown,
            });
        }
        return Err(
            "no web address in it: paste a $(urlfetch …), ${customapi.…} or https://… line".into(),
        );
    };
    // Nightbot's `$(urlfetch json URL)` asks for raw JSON, which is what we get anyway.
    let inner = fetch.inner.trim();
    let inner = inner.strip_prefix("json ").unwrap_or(inner).trim();
    let url = translate(inner, &mut unknown);
    if !is_http(&url) {
        return Err("the address it fetches doesn't start with http:// or https://".into());
    }
    let before = translate(&response[..fetch.start], &mut unknown);
    let after = translate(&response[fetch.end..], &mut unknown);
    let reply = format!("{before}{ANSWER}{after}").trim().to_string();
    Ok(Converted {
        command,
        url,
        reply: Some(reply),
        from: fetch.from,
        unknown,
    })
}

fn is_http(s: &str) -> bool {
    let s = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    s.starts_with("http://") || s.starts_with("https://")
}

/// `!commands add !rank -cd=5 <response>` to (`!rank`, `<response>`); any
/// other text is all response.
fn split_command(text: &str) -> (Option<String>, &str) {
    let first = word(text);
    if !ADD_COMMANDS.iter().any(|w| first.eq_ignore_ascii_case(w)) {
        return (None, text);
    }
    let mut rest = after(text, first);
    let verb = word(rest);
    if ["add", "edit", "new"]
        .iter()
        .any(|v| verb.eq_ignore_ascii_case(v))
    {
        rest = after(rest, verb);
    }
    rest = skip_flags(rest);
    let name = word(rest).trim_start_matches('!').to_lowercase();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return (None, text);
    }
    let rest = skip_flags(after(rest, word(rest)));
    (Some(format!("!{name}")), rest)
}

fn word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// `s` after its first word `w` (which `s` starts with, past whitespace).
fn after<'a>(s: &'a str, w: &str) -> &'a str {
    &s.trim_start()[w.len()..]
}

/// Nightbot's `-cd=5 -ul=moderator`.
fn skip_flags(mut s: &str) -> &str {
    loop {
        let w = word(s);
        if !(w.starts_with('-') && w.contains('=')) {
            return s;
        }
        s = after(s, w);
    }
}

struct Fetch<'a> {
    start: usize,
    end: usize,
    inner: &'a str,
    from: &'static str,
}

/// The first fetch expression in `s`.
fn find_fetch(s: &str) -> Option<Fetch<'_>> {
    // ASCII lowercasing keeps byte offsets, so they index `s` too.
    let lower = s.to_ascii_lowercase();
    let (start, open, close, from) = FETCHES
        .iter()
        .filter_map(|(open, close, from)| lower.find(open).map(|at| (at, *open, *close, *from)))
        .min_by_key(|f| f.0)?;
    let body = start + open.len();
    let len = closing(&s[body..], close)?;
    Some(Fetch {
        start,
        end: body + len + 1,
        inner: &s[body..body + len],
        from,
    })
}

/// Where the expression ends: the first `close` not inside a nested
/// `$(…)`, `${…}` or `{…}`.
fn closing(s: &str, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if matches!(bytes.get(i + 1), Some(b'(' | b'{')) => {
                depth += 1;
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b')' | b'}' if depth > 0 => depth -= 1,
            b if b == close => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Rewrite every bot variable in `s` as ours.
fn translate(s: &str, unknown: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = next_variable(rest) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (open, close) = match tail.as_bytes()[0] {
            b'$' => (
                2,
                if tail.as_bytes()[1] == b'(' {
                    b')'
                } else {
                    b'}'
                },
            ),
            _ => (1, b'}'),
        };
        let Some(len) = closing(&tail[open..], close) else {
            out.push_str(tail);
            return out;
        };
        let whole = &tail[..open + len + 1];
        out.push_str(&ours(&tail[open..open + len], whole, open == 1, unknown));
        rest = &tail[whole.len()..];
    }
    out.push_str(rest);
    out
}

/// Where the next `$(`, `${` or `{` starts.
fn next_variable(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    (0..bytes.len()).find(|&i| match bytes[i] {
        b'$' => matches!(bytes.get(i + 1), Some(b'(' | b'{')),
        b'{' => true,
        _ => false,
    })
}

/// One bot variable as ours. `bare`: written `{…}` (Streamlabs, or already
/// ours), which stays as written when unknown instead of being flagged.
fn ours(inner: &str, whole: &str, bare: bool, unknown: &mut Vec<String>) -> String {
    let inner = inner.trim();
    let (name, arg) = match inner.split_once(char::is_whitespace) {
        Some((name, arg)) => (name, Some(arg.trim())),
        None => (inner, None),
    };
    let name = name.to_ascii_lowercase();
    // Encoding helpers: InstantClone encodes values in addresses itself.
    if let ("urlencode" | "queryescape" | "pathescape" | "urlescape", Some(arg)) =
        (name.as_str(), arg)
    {
        return translate(arg, unknown);
    }
    let mapped = match name.as_str() {
        "user" | "user.name" | "user.display_name" | "sender" | "source" | "username"
        | "displayname" => "user",
        "touser" | "target" => "target",
        "query" | "querystring" | "args" => "args",
        "channel" | "channel.name" | "streamer" => "channel",
        n if n.len() == 2 && n.ends_with(':') && n.as_bytes()[0].is_ascii_digit() => "args",
        n if n.len() == 1 && (b'1'..=b'9').contains(&n.as_bytes()[0]) => {
            return format!("{{arg{n}}}")
        }
        _ if bare => return whole.to_string(),
        _ => {
            unknown.push(whole.to_string());
            return whole.to_string();
        }
    };
    format!("{{{mapped}}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Converted {
        convert(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn nightbot_add_line_gives_command_address_and_reply() {
        let c = ok("!commands add !Rank -cd=5 $(user), your rank: $(urlfetch https://api.example/rank/$(querystring)?region=eu)");
        assert_eq!(c.command.as_deref(), Some("!rank"));
        assert_eq!(c.url, "https://api.example/rank/{args}?region=eu");
        assert_eq!(c.reply.as_deref(), Some("{user}, your rank: {api.body}"));
        assert_eq!(c.from, "Nightbot");
        assert!(c.unknown.is_empty());
    }

    #[test]
    fn streamelements_nested_helpers_unwrap() {
        let c = ok("!cmd add rank ${customapi.https://api.example/v1/${pathescape ${1}}/stats}");
        assert_eq!(c.command.as_deref(), Some("!rank"));
        assert_eq!(c.url, "https://api.example/v1/{arg1}/stats");
        assert_eq!(c.reply.as_deref(), Some("{api.body}"));
        assert_eq!(c.from, "StreamElements");
    }

    #[test]
    fn fossabot_and_streamlabs_variables_map() {
        let c = ok("$(customapi https://x.example/followage/$(channel)/$(user))");
        assert_eq!(c.url, "https://x.example/followage/{channel}/{user}");
        assert_eq!(c.command, None);
        let c = ok("{readapi.https://x.example/q?u={user}&n={1}}");
        assert_eq!(c.url, "https://x.example/q?u={user}&n={arg1}");
        assert_eq!(c.from, "Streamlabs");
    }

    #[test]
    fn touser_and_json_fetch() {
        let c = ok("$(touser) is $(urlfetch json https://x.example/$(touser))");
        assert_eq!(c.url, "https://x.example/{target}");
        assert_eq!(c.reply.as_deref(), Some("{target} is {api.body}"));
    }

    #[test]
    fn a_plain_address_is_just_the_address() {
        let c = ok("  https://x.example/a?b=1  ");
        assert_eq!(c.url, "https://x.example/a?b=1");
        assert_eq!(c.reply, None);
    }

    #[test]
    fn unknown_variables_stay_and_are_listed() {
        let c = ok("$(count) asks: $(urlfetch https://x.example)");
        assert_eq!(c.reply.as_deref(), Some("$(count) asks: {api.body}"));
        assert_eq!(c.unknown, vec!["$(count)".to_string()]);
    }

    #[test]
    fn what_cannot_convert_says_why() {
        assert!(convert("$(eval const r = 1; r)")
            .unwrap_err()
            .contains("JavaScript"));
        assert!(convert("hello $(user)")
            .unwrap_err()
            .contains("no web address"));
        assert!(convert("$(urlfetch ftp://x.example)")
            .unwrap_err()
            .contains("http"));
        assert!(convert("$(urlfetch https://x.example").is_err());
        assert!(convert("   ").is_err());
    }
}
