//! Minimal AMF0. We need to parse `connect`, `releaseStream`, `FCPublish`,
//! `createStream`, `publish`, `deleteStream` from clients, and emit
//! `_result` and `onStatus` responses. That's the entire surface.
//!
//! Value markers we care about:
//!   0x00 number     — 8-byte big-endian f64
//!   0x01 boolean    — 1 byte
//!   0x02 string     — 2-byte length BE + UTF-8 bytes
//!   0x03 object     — repeated (key:string-no-marker, value:any) ending in
//!                     0x00 0x00 0x09 (empty-key + end-marker)
//!   0x05 null
//!   0x06 undefined
//!   0x08 ECMA array — like object with a 4-byte count prefix (ignored)
//!   0x09 object-end (only valid inside object/ecma array)

use bytes::{BufMut, BytesMut};
use std::collections::HashMap;
use std::io::{self, ErrorKind};

#[derive(Debug, Clone)]
pub enum Amf0 {
    Number(f64),
    Boolean(bool),
    String(String),
    Object(HashMap<String, Amf0>),
    Null,
    Undefined,
    EcmaArray(HashMap<String, Amf0>),
}

impl Amf0 {
    pub fn as_str(&self) -> Option<&str> {
        if let Amf0::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Amf0::Number(n) = self {
            Some(*n)
        } else {
            None
        }
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Amf0>> {
        match self {
            Amf0::Object(o) | Amf0::EcmaArray(o) => Some(o),
            _ => None,
        }
    }
}

/// Maximum nesting depth we'll honour when decoding AMF0. Real RTMP
/// command/data payloads from OBS, ffmpeg, Twitch etc. never go more
/// than 3-4 levels deep. A malicious peer sending `0x03 0x03 0x03 …` ad
/// infinitum would otherwise blow the stack and crash the ingest task
/// (or, worse with `ingest_bind_all=true`, the whole process from a
/// LAN attacker).
const AMF0_MAX_DEPTH: u32 = 16;

pub fn decode_all(mut data: &[u8]) -> io::Result<Vec<Amf0>> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let (v, rest) = decode_one(data, 0)?;
        out.push(v);
        data = rest;
    }
    Ok(out)
}

fn decode_one(data: &[u8], depth: u32) -> io::Result<(Amf0, &[u8])> {
    if depth > AMF0_MAX_DEPTH {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "amf0: nesting too deep",
        ));
    }
    if data.is_empty() {
        return Err(io::Error::new(ErrorKind::UnexpectedEof, "amf0: empty"));
    }
    let marker = data[0];
    let rest = &data[1..];
    match marker {
        0x00 => {
            need(rest, 8)?;
            let n = f64::from_be_bytes(rest[..8].try_into().unwrap());
            Ok((Amf0::Number(n), &rest[8..]))
        }
        0x01 => {
            need(rest, 1)?;
            Ok((Amf0::Boolean(rest[0] != 0), &rest[1..]))
        }
        0x02 => {
            let (s, rest) = decode_string(rest)?;
            Ok((Amf0::String(s), rest))
        }
        0x03 => {
            let (m, rest) = decode_object_body(rest, depth + 1)?;
            Ok((Amf0::Object(m), rest))
        }
        0x05 => Ok((Amf0::Null, rest)),
        0x06 => Ok((Amf0::Undefined, rest)),
        0x08 => {
            need(rest, 4)?;
            let _count = u32::from_be_bytes(rest[..4].try_into().unwrap());
            let (m, rest) = decode_object_body(&rest[4..], depth + 1)?;
            Ok((Amf0::EcmaArray(m), rest))
        }
        m => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("amf0: unsupported marker {:#x}", m),
        )),
    }
}

fn decode_string(data: &[u8]) -> io::Result<(String, &[u8])> {
    need(data, 2)?;
    let len = u16::from_be_bytes([data[0], data[1]]) as usize;
    need(&data[2..], len)?;
    let s = std::str::from_utf8(&data[2..2 + len])
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "amf0: utf8"))?
        .to_owned();
    Ok((s, &data[2 + len..]))
}

fn decode_object_body(mut data: &[u8], depth: u32) -> io::Result<(HashMap<String, Amf0>, &[u8])> {
    let mut map = HashMap::new();
    loop {
        // Key has no string marker — it's an inline length-prefixed string.
        let (key, rest) = decode_string(data)?;
        data = rest;
        if key.is_empty() && data.first() == Some(&0x09) {
            return Ok((map, &data[1..]));
        }
        let (val, rest) = decode_one(data, depth)?;
        map.insert(key, val);
        data = rest;
    }
}

fn need(data: &[u8], n: usize) -> io::Result<()> {
    if data.len() < n {
        Err(io::Error::new(ErrorKind::UnexpectedEof, "amf0: short"))
    } else {
        Ok(())
    }
}

// ---- Encoding ----

pub fn enc_string(out: &mut BytesMut, s: &str) {
    out.put_u8(0x02);
    out.put_u16(s.len() as u16);
    out.put_slice(s.as_bytes());
}

pub fn enc_number(out: &mut BytesMut, n: f64) {
    out.put_u8(0x00);
    out.put_f64(n);
}

pub fn enc_null(out: &mut BytesMut) {
    out.put_u8(0x05);
}

pub fn enc_object(out: &mut BytesMut, pairs: &[(&str, &Amf0)]) {
    out.put_u8(0x03);
    for (k, v) in pairs {
        out.put_u16(k.len() as u16);
        out.put_slice(k.as_bytes());
        enc_value(out, v);
    }
    // empty-key + end marker
    out.put_u16(0);
    out.put_u8(0x09);
}

pub fn enc_value(out: &mut BytesMut, v: &Amf0) {
    match v {
        Amf0::Number(n) => enc_number(out, *n),
        Amf0::Boolean(b) => {
            out.put_u8(0x01);
            out.put_u8(if *b { 1 } else { 0 });
        }
        Amf0::String(s) => enc_string(out, s),
        Amf0::Null | Amf0::Undefined => enc_null(out),
        Amf0::Object(m) | Amf0::EcmaArray(m) => {
            let pairs: Vec<(&str, &Amf0)> = m.iter().map(|(k, v)| (k.as_str(), v)).collect();
            enc_object(out, &pairs);
        }
    }
}

/// Encode an AMF0 Strict Array (marker 0x0A) of strings. Used for the
/// Enhanced RTMP `fourCcList` property in the connect command, where
/// publishers declare which non-legacy codecs they're capable of
/// sending (HEVC, AV1, VP9, Opus, AC-3, FLAC). Strict Array is the
/// semantically correct AMF0 container for a fixed-length, integer-
/// indexed sequence.
pub fn enc_strict_array_str(out: &mut BytesMut, items: &[&str]) {
    out.put_u8(0x0A);
    out.put_u32(items.len() as u32);
    for s in items {
        enc_string(out, s);
    }
}

/// Open an AMF0 object (marker 0x03). Pair with `enc_object_end` and
/// `enc_object_key` to write objects whose values aren't all uniform
/// types — useful when one of the values is a Strict Array that the
/// generic `enc_object(&[(&str, &Amf0)])` builder can't represent.
pub fn enc_object_begin(out: &mut BytesMut) {
    out.put_u8(0x03);
}

/// Write one AMF0 object key (length-prefixed UTF-8, NO string marker —
/// object keys are raw, distinct from the 0x02 string-value marker).
pub fn enc_object_key(out: &mut BytesMut, key: &str) {
    out.put_u16(key.len() as u16);
    out.put_slice(key.as_bytes());
}

/// Close an AMF0 object — empty key + 0x09 end-of-object marker.
pub fn enc_object_end(out: &mut BytesMut) {
    out.put_u16(0);
    out.put_u8(0x09);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_roundtrip() {
        let mut buf = BytesMut::new();
        enc_number(&mut buf, 42.5);
        let decoded = decode_all(&buf).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].as_f64(), Some(42.5));
    }

    #[test]
    fn string_roundtrip_handles_utf8() {
        let mut buf = BytesMut::new();
        enc_string(&mut buf, "hola café");
        let decoded = decode_all(&buf).unwrap();
        assert_eq!(decoded[0].as_str(), Some("hola café"));
    }

    #[test]
    fn null_roundtrip() {
        let mut buf = BytesMut::new();
        enc_null(&mut buf);
        let decoded = decode_all(&buf).unwrap();
        assert!(matches!(decoded[0], Amf0::Null));
    }

    #[test]
    fn boolean_decode() {
        let decoded = decode_all(&[0x01, 0x01]).unwrap();
        assert!(matches!(decoded[0], Amf0::Boolean(true)));
        let decoded = decode_all(&[0x01, 0x00]).unwrap();
        assert!(matches!(decoded[0], Amf0::Boolean(false)));
    }

    #[test]
    fn object_roundtrip_preserves_keys() {
        let mut buf = BytesMut::new();
        let app = Amf0::String("live".into());
        let ver = Amf0::Number(2.0);
        enc_object(&mut buf, &[("app", &app), ("ver", &ver)]);
        let decoded = decode_all(&buf).unwrap();
        let obj = decoded[0].as_object().expect("decoded as object");
        assert_eq!(obj.get("app").and_then(|v| v.as_str()), Some("live"));
        assert_eq!(obj.get("ver").and_then(|v| v.as_f64()), Some(2.0));
    }

    #[test]
    fn unsupported_marker_returns_error() {
        // marker 0xAA — we don't implement that one
        let r = decode_all(&[0xAA, 0x00]);
        assert!(r.is_err());
    }

    #[test]
    fn truncated_string_returns_error_not_panic() {
        // string marker says 100 bytes follow, but only 4 are present
        let r = decode_all(&[0x02, 0x00, 100, 0xAB, 0xCD]);
        assert!(r.is_err());
    }

    #[test]
    fn ecma_array_decodes_like_object() {
        // marker 0x08 (ECMA array): 4-byte count prefix, then same
        // body layout as Object (key + value pairs, end marker).
        let mut buf: Vec<u8> = Vec::new();
        buf.push(0x08); // marker
        buf.extend_from_slice(&3u32.to_be_bytes()); // count = 3 (ignored)
                                                    // one entry: key="x" value=number 1.0
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.push(b'x');
        buf.push(0x00); // number marker
        buf.extend_from_slice(&1.0f64.to_be_bytes());
        // end marker
        buf.extend_from_slice(&[0x00, 0x00, 0x09]);

        let decoded = decode_all(&buf).unwrap();
        let obj = decoded[0]
            .as_object()
            .expect("ecma array exposes as object");
        assert_eq!(obj.get("x").and_then(|v| v.as_f64()), Some(1.0));
    }

    #[test]
    fn empty_object_roundtrip() {
        let mut buf = BytesMut::new();
        enc_object(&mut buf, &[]);
        let decoded = decode_all(&buf).unwrap();
        let obj = decoded[0].as_object().expect("empty object");
        assert!(obj.is_empty());
    }

    #[test]
    fn multiple_values_in_one_buffer() {
        // A single buffer can hold a sequence of top-level values,
        // which `decode_all` returns as a Vec. This mirrors what RTMP
        // command messages actually look like ("connect", txn, params).
        let mut buf = BytesMut::new();
        enc_string(&mut buf, "connect");
        enc_number(&mut buf, 1.0);
        enc_null(&mut buf);
        let decoded = decode_all(&buf).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].as_str(), Some("connect"));
        assert_eq!(decoded[1].as_f64(), Some(1.0));
        assert!(matches!(decoded[2], Amf0::Null));
    }

    #[test]
    fn deeply_nested_object_rejected() {
        // Hand-craft a deeply nested object payload — each 0x03 starts a
        // new object body. AMF0_MAX_DEPTH=16, so a 17-deep chain trips the
        // guard. The empty-key + 0x09 end markers are appended in reverse
        // to keep the payload syntactically closeable (the decoder bails
        // on the depth check before getting that far).
        // 17 opening Object markers, then 17 (empty-key + end) trailers.
        let mut payload: Vec<u8> = vec![0x03; 17];
        for _ in 0..17 {
            payload.extend_from_slice(&[0x00, 0x00, 0x09]);
        }
        let r = decode_all(&payload);
        assert!(
            r.is_err(),
            "deeply nested objects must be rejected to prevent stack-blow"
        );
    }
}
