//! Minimal JSON value: parse, walk, serialize.
//!
//! The rest of the crate hand-writes the JSON it emits and never needed to
//! read structured JSON back. Integrations do: the dashboard posts whole
//! integrations (triggers, nested steps) and web requests hand back bodies
//! whose fields a step can use. A few hundred lines here beat a serde
//! dependency for that, and keep the crate's no-JSON-crate stance.
//!
//! Objects keep their key order, so a value read and written back comes out
//! the way it went in.

use std::fmt::Write as _;

/// Nesting limit for parsing: deep enough for any integration, shallow
/// enough that hostile input cannot overflow the stack.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// The value under `key` when this is an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Walk a dotted path: object keys and array indexes (`items.0.name`).
    pub fn path(&self, path: &str) -> Option<&Value> {
        path.split('.').try_fold(self, |node, part| match node {
            Value::Obj(_) => node.get(part),
            Value::Arr(items) => part.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        })
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// String field with a default, for reading optional settings.
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(Value::as_str).unwrap_or(default)
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    /// Non-negative integer field, clamped to `u64`, with a default.
    pub fn u64_or(&self, key: &str, default: u64) -> u64 {
        match self.get(key).and_then(Value::as_f64) {
            Some(n) if n.is_finite() && n >= 0.0 => n.min(u64::MAX as f64) as u64,
            _ => default,
        }
    }

    /// Text a template can show: strings as-is, scalars printed, and
    /// containers as compact JSON.
    pub fn to_display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Str(s) => s.clone(),
            other => other.to_json(),
        }
    }

    /// Compact JSON text.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Num(n) => write_number(*n, out),
            Value::Str(s) => write_string(s, out),
            Value::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Value::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Builder shorthand for objects: `obj([("a", Value::Bool(true))])`.
pub fn obj<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Obj(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

pub fn str(s: impl Into<String>) -> Value {
    Value::Str(s.into())
}

fn write_number(n: f64, out: &mut String) {
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        let _ = write!(out, "{}", n as i64);
    } else {
        let _ = write!(out, "{n}");
    }
}

/// Append `s` as a quoted JSON string.
pub fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one JSON document (RFC 8259). Trailing data is an error.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(format!("unexpected data at byte {}", parser.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.pos))
    }

    fn eat(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            self.fail(&format!("expected '{}'", byte as char))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            self.fail("invalid literal")
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return self.fail("nested too deep");
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.fail("expected a value"),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Obj(fields));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.eat(b':')?;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Obj(fields));
                }
                _ => return self.fail("expected ',' or '}'"),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return self.fail("expected ',' or ']'"),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return self.fail("invalid number"),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.fail("invalid number");
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.fail("invalid number");
            }
            self.digits();
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        text.parse::<f64>()
            .map(Value::Num)
            .or_else(|_| self.fail("invalid number"))
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            // Copy the run of plain bytes up to the next quote or escape in
            // one go; the input is a &str, so the run is valid UTF-8.
            let run_start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(std::str::from_utf8(&self.bytes[run_start..self.pos]).unwrap_or(""));
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    self.escape(&mut out)?;
                }
                Some(_) => return self.fail("control character in string"),
                None => return self.fail("unterminated string"),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), String> {
        let Some(b) = self.peek() else {
            return self.fail("unterminated escape");
        };
        self.pos += 1;
        match b {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                let code = if (0xD800..0xDC00).contains(&first) {
                    // A high surrogate must be followed by `\u` + low surrogate.
                    if !self.bytes[self.pos..].starts_with(b"\\u") {
                        return self.fail("lone surrogate");
                    }
                    self.pos += 2;
                    let second = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&second) {
                        return self.fail("invalid surrogate pair");
                    }
                    0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                } else {
                    first
                };
                match char::from_u32(code) {
                    Some(c) => out.push(c),
                    None => return self.fail("invalid unicode escape"),
                }
            }
            _ => return self.fail("invalid escape"),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let Some(digits) = self.bytes.get(self.pos..self.pos + 4) else {
            return self.fail("short unicode escape");
        };
        let text = std::str::from_utf8(digits).unwrap_or("");
        let code = u32::from_str_radix(text, 16).or_else(|_| self.fail("bad unicode escape"))?;
        self.pos += 4;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_documents() {
        let v =
            parse(r#" {"a": [1, 2.5, -3e2], "b": {"c": "x\ny"}, "d": null, "e": true} "#).unwrap();
        assert_eq!(v.path("a.1").and_then(Value::as_f64), Some(2.5));
        assert_eq!(v.path("a.2").and_then(Value::as_f64), Some(-300.0));
        assert_eq!(v.path("b.c").and_then(Value::as_str), Some("x\ny"));
        assert_eq!(v.get("d"), Some(&Value::Null));
        assert!(v.bool_or("e", false));
    }

    #[test]
    fn round_trips_and_keeps_key_order() {
        let text = r#"{"z":1,"a":"é \"q\" \\ \u0001","list":[true,false,null],"o":{}}"#;
        let v = parse(text).unwrap();
        assert_eq!(v.to_json(), text);
        assert!(crate::config::is_valid_json(&v.to_json()));
    }

    #[test]
    fn decodes_surrogate_pairs() {
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str(), Some("😀"));
        assert!(parse(r#""\ud83d""#).is_err());
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "01",
            "1.",
            "\"\u{1}\"",
            "tru",
            "[1] x",
            "{\"a\":1,}",
        ] {
            assert!(parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn refuses_runaway_nesting() {
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn integers_print_without_a_fraction() {
        assert_eq!(Value::Num(30000.0).to_json(), "30000");
        assert_eq!(Value::Num(1.5).to_json(), "1.5");
        assert_eq!(Value::Num(f64::NAN).to_json(), "null");
    }

    #[test]
    fn typed_getters_fall_back() {
        let v = parse(r#"{"n": 5, "neg": -1, "s": "x"}"#).unwrap();
        assert_eq!(v.u64_or("n", 0), 5);
        assert_eq!(v.u64_or("neg", 7), 7);
        assert_eq!(v.u64_or("missing", 9), 9);
        assert_eq!(v.str_or("s", "d"), "x");
        assert_eq!(v.str_or("n", "d"), "d");
    }
}
