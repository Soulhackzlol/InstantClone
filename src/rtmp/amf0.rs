//! Minimal AMF0. We need to parse `connect`, `releaseStream`, `FCPublish`,
//! `createStream`, `publish`, `deleteStream` from clients, and emit
//! `_result` and `onStatus` responses. That's the entire surface.
//!
//! Value markers we care about:
//!   0x00 number     - 8-byte big-endian f64
//!   0x01 boolean    - 1 byte
//!   0x02 string     - 2-byte length BE + UTF-8 bytes
//!   0x03 object     - repeated (key:string-no-marker, value:any) ending in
//!                     0x00 0x00 0x09 (empty-key + end-marker)
//!   0x05 null
//!   0x06 undefined
//!   0x08 ECMA array - like object with a 4-byte count prefix (ignored)
//!   0x09 object-end (only valid inside object/ecma array)
//!   0x0A strict array - 4-byte BE count, then N values back-to-back.
//!                       OBS-style clients send `fourCcList` this way
//!                       in the connect properties (Enhanced RTMP). We
//!                       parse it so the value isn't a decode hard-stop
//!                       - none of the handshake logic actually needs
//!                       to read the list's contents, but the in-tree
//!                       sink and any cross-talk between proxies has
//!                       to be able to step over it.
//!   0x0B date       - 8-byte f64 ms since epoch + 2-byte time zone (unused)
//!   0x0C long string - 4-byte length BE + UTF-8 bytes, for text over 64 KiB.
//!                      Servers may use either in a reply; one unknown
//!                      marker fails the whole command, so both decode.

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
    StrictArray(Vec<Amf0>),
    /// Milliseconds since the Unix epoch. The wire also carries a 2-byte
    /// time zone that the spec reserves (always 0), so it is not kept.
    Date(f64),
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
        0x0A => {
            need(rest, 4)?;
            let count = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            // Cap the declared count at the number of bytes remaining; a
            // malicious peer could otherwise advertise a huge count and
            // walk us into an OOM by pushing into a Vec we pre-grew.
            // Each AMF0 value is at least 1 byte (the marker), so the
            // remaining buffer length is a strict upper bound.
            let mut data = &rest[4..];
            let cap = count.min(data.len());
            let mut items = Vec::with_capacity(cap);
            for _ in 0..count {
                let (v, next) = decode_one(data, depth + 1)?;
                items.push(v);
                data = next;
            }
            Ok((Amf0::StrictArray(items), data))
        }
        0x0B => {
            // Date: f64 ms since the epoch, then a reserved 2-byte time zone.
            need(rest, 10)?;
            let ms = f64::from_be_bytes(rest[..8].try_into().unwrap());
            Ok((Amf0::Date(ms), &rest[10..]))
        }
        0x0C => {
            // Long string: like 0x02 but with a 4-byte length, for text
            // over 64 KiB. Surfaces as a plain String - callers never care
            // which length prefix it came with.
            need(rest, 4)?;
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let body = &rest[4..];
            need(body, len)?;
            Ok((Amf0::String(utf8(&body[..len])?), &body[len..]))
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
    Ok((utf8(&data[2..2 + len])?, &data[2 + len..]))
}

fn utf8(bytes: &[u8]) -> io::Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "amf0: utf8"))
}

fn decode_object_body(mut data: &[u8], depth: u32) -> io::Result<(HashMap<String, Amf0>, &[u8])> {
    let mut map = HashMap::new();
    loop {
        // Key has no string marker - it's an inline length-prefixed string.
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
        Amf0::Date(ms) => {
            out.put_u8(0x0B);
            out.put_f64(*ms);
            out.put_i16(0); // time zone: reserved, always 0
        }
        Amf0::StrictArray(items) => {
            out.put_u8(0x0A);
            out.put_u32(items.len() as u32);
            for it in items {
                enc_value(out, it);
            }
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
/// types - useful when one of the values is a Strict Array that the
/// generic `enc_object(&[(&str, &Amf0)])` builder can't represent.
pub fn enc_object_begin(out: &mut BytesMut) {
    out.put_u8(0x03);
}

/// Write one AMF0 object key (length-prefixed UTF-8, NO string marker -
/// object keys are raw, distinct from the 0x02 string-value marker).
pub fn enc_object_key(out: &mut BytesMut, key: &str) {
    out.put_u16(key.len() as u16);
    out.put_slice(key.as_bytes());
}

/// Close an AMF0 object - empty key + 0x09 end-of-object marker.
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
        // marker 0xAA - we don't implement that one
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
    fn strict_array_of_strings_roundtrip() {
        // Enhanced-RTMP fourCcList shape: a Strict Array of 4-char
        // codec identifier strings. This test guards the e2e path -
        // before 0x0A decoding landed, the in-tree sink rejected our
        // own connect with `amf0: unsupported marker 0xa`.
        let mut buf = BytesMut::new();
        enc_strict_array_str(&mut buf, &["avc1", "hvc1", "mp4a"]);
        let decoded = decode_all(&buf).unwrap();
        match &decoded[0] {
            Amf0::StrictArray(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0].as_str(), Some("avc1"));
                assert_eq!(items[1].as_str(), Some("hvc1"));
                assert_eq!(items[2].as_str(), Some("mp4a"));
            }
            other => panic!("expected StrictArray, got {:?}", other),
        }
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
    fn date_and_long_string_decode_instead_of_failing_the_command() {
        // Servers are free to put a Date (0x0B) or a long string (0x0C) in
        // a `_result` / `onStatus`, and one unknown marker fails decode_all
        // for the whole command: an egress connect then dies on a reply it
        // did not even need, and an ingest connection is dropped.
        let mut buf = vec![0x0B];
        buf.extend_from_slice(&1_700_000_000_000.0f64.to_be_bytes());
        buf.extend_from_slice(&[0x00, 0x00]); // time zone, reserved
        buf.push(0x0C);
        buf.extend_from_slice(&5u32.to_be_bytes());
        buf.extend_from_slice(b"hello");
        buf.extend_from_slice(&[0x05]); // something after both
        let decoded = decode_all(&buf).unwrap();
        assert_eq!(decoded.len(), 3);
        assert!(matches!(decoded[0], Amf0::Date(ms) if ms == 1_700_000_000_000.0));
        assert_eq!(decoded[1].as_str(), Some("hello"));
        assert!(matches!(decoded[2], Amf0::Null));

        // A long string longer than 64 KiB, which is the point of 0x0C.
        let big = "x".repeat(70_000);
        let mut buf = vec![0x0C];
        buf.extend_from_slice(&(big.len() as u32).to_be_bytes());
        buf.extend_from_slice(big.as_bytes());
        assert_eq!(decode_all(&buf).unwrap()[0].as_str(), Some(big.as_str()));

        // And they re-encode to the same bytes a Date is sent as.
        let mut out = BytesMut::new();
        enc_value(&mut out, &Amf0::Date(2.0));
        let mut expected = vec![0x0B];
        expected.extend_from_slice(&2.0f64.to_be_bytes());
        expected.extend_from_slice(&[0, 0]);
        assert_eq!(&out[..], &expected[..]);
    }

    #[test]
    fn truncated_date_and_long_string_are_errors() {
        // Short on the time zone, short on the length, and a length that
        // claims more than is there: never read past the buffer.
        assert!(decode_all(&[0x0B, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(decode_all(&[0x0C, 0, 0, 0]).is_err());
        assert!(decode_all(&[0x0C, 0xFF, 0xFF, 0xFF, 0xFF, b'a']).is_err());
        assert!(
            decode_all(&[0x0C, 0, 0, 0, 2, 0xC3, 0x28]).is_err(),
            "bad UTF-8"
        );
    }

    /// `levels` containers nested inside each other, each holding exactly
    /// one child, built with the real wire syntax for each container type.
    fn nested(levels: usize, marker: u8) -> Vec<u8> {
        let (open, empty, close): (&[u8], &[u8], &[u8]) = match marker {
            0x03 => (
                &[0x03, 0x00, 0x01, b'k'],
                &[0x03, 0x00, 0x00, 0x09],
                &[0x00, 0x00, 0x09],
            ),
            0x08 => (
                &[0x08, 0, 0, 0, 1, 0x00, 0x01, b'k'],
                &[0x08, 0, 0, 0, 0, 0x00, 0x00, 0x09],
                &[0x00, 0x00, 0x09],
            ),
            _ => (&[0x0A, 0, 0, 0, 1], &[0x0A, 0, 0, 0, 0], &[]),
        };
        let mut out = open.repeat(levels - 1);
        out.extend_from_slice(empty);
        out.extend(close.repeat(levels - 1));
        out
    }

    #[test]
    fn nesting_past_the_depth_limit_is_rejected_for_every_container() {
        // A peer can nest containers without bound and recurse the decoder
        // off the stack. The guard allows AMF0_MAX_DEPTH + 1 levels (the
        // top-level value is depth 0) and refuses the next one with
        // InvalidData - not UnexpectedEof, which is what malformed bytes
        // produce, so this fails if the guard is removed.
        let max_levels = AMF0_MAX_DEPTH as usize + 1;
        for marker in [0x03, 0x08, 0x0A] {
            assert!(
                decode_all(&nested(max_levels, marker)).is_ok(),
                "marker {marker:#x}: {max_levels} levels are legal"
            );
            let err = decode_all(&nested(max_levels + 1, marker)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidData, "marker {marker:#x}");
        }
        // Mixed containers count toward the same limit.
        let mut mixed = nested(max_levels, 0x03);
        mixed.splice(0..0, [0x0A, 0, 0, 0, 1]);
        let err = decode_all(&mixed).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn a_strict_array_claiming_four_billion_items_errors_quickly() {
        // The count is capped by the bytes left before anything is
        // reserved, so this is a short error, not a 64 GB allocation.
        let mut buf = vec![0x0A, 0xFF, 0xFF, 0xFF, 0xFF];
        buf.extend_from_slice(&[0x05, 0x05, 0x05]);
        assert!(decode_all(&buf).is_err());
    }

    #[test]
    fn edge_values_decode_exactly() {
        let mut nan = vec![0x00];
        nan.extend_from_slice(&f64::NAN.to_be_bytes());
        assert!(decode_all(&nan).unwrap()[0].as_f64().unwrap().is_nan());
        assert_eq!(
            decode_all(&[0x02, 0x00, 0x00]).unwrap()[0].as_str(),
            Some("")
        );
        assert!(matches!(decode_all(&[0x06]).unwrap()[0], Amf0::Undefined));
        // An object whose end marker never comes is an error, whether the
        // bytes stop after a value or after the empty key.
        assert!(decode_all(&[0x03, 0x00, 0x01, b'a', 0x05]).is_err());
        assert!(decode_all(&[0x03, 0x00, 0x00]).is_err());
        // An empty key followed by anything but 0x09 is a value, not the end.
        assert!(decode_all(&[0x03, 0x00, 0x00, 0x05, 0x00, 0x00, 0x09]).is_ok());
    }

    #[test]
    fn enc_object_writes_known_bytes() {
        // The shape every _result / onStatus we send is built from.
        let mut out = BytesMut::new();
        enc_object(
            &mut out,
            &[("a", &Amf0::Number(1.0)), ("ok", &Amf0::Boolean(true))],
        );
        let mut expected = vec![0x03, 0x00, 0x01, b'a', 0x00];
        expected.extend_from_slice(&1.0f64.to_be_bytes());
        expected.extend_from_slice(&[0x00, 0x02, b'o', b'k', 0x01, 0x01]);
        expected.extend_from_slice(&[0x00, 0x00, 0x09]);
        assert_eq!(&out[..], &expected[..]);
    }

    /// Structural equality. Amf0 has no PartialEq (maps hold f64s), and
    /// the tests only need it here.
    fn same(a: &Amf0, b: &Amf0) -> bool {
        match (a, b) {
            (Amf0::Number(x), Amf0::Number(y)) | (Amf0::Date(x), Amf0::Date(y)) => {
                x.to_bits() == y.to_bits()
            }
            (Amf0::Boolean(x), Amf0::Boolean(y)) => x == y,
            (Amf0::String(x), Amf0::String(y)) => x == y,
            (Amf0::Null, Amf0::Null) | (Amf0::Undefined, Amf0::Undefined) => true,
            (Amf0::Object(x), Amf0::Object(y)) | (Amf0::EcmaArray(x), Amf0::EcmaArray(y)) => {
                x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
            }
            (Amf0::StrictArray(x), Amf0::StrictArray(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
            }
            _ => false,
        }
    }

    /// A connect command as an Enhanced-RTMP publisher sends it, plus the
    /// containers and markers servers use in replies. Returns the bytes
    /// and the offset where each top-level value ends.
    fn rich_connect_payload() -> (Vec<u8>, Vec<usize>) {
        let mut buf = BytesMut::new();
        let mut ends = Vec::new();
        enc_string(&mut buf, "connect");
        ends.push(buf.len());
        enc_number(&mut buf, 1.0);
        ends.push(buf.len());
        enc_object_begin(&mut buf);
        enc_object_key(&mut buf, "app");
        enc_string(&mut buf, "live");
        enc_object_key(&mut buf, "fpad");
        enc_value(&mut buf, &Amf0::Boolean(false));
        enc_object_key(&mut buf, "fourCcList");
        enc_strict_array_str(&mut buf, &["avc1", "hvc1"]);
        enc_object_key(&mut buf, "ecma");
        buf.extend_from_slice(&[0x08, 0, 0, 0, 2, 0x00, 0x01, b'u', 0x06]);
        buf.extend_from_slice(&[0x00, 0x01, b'd', 0x0B]);
        buf.extend_from_slice(&3.0f64.to_be_bytes());
        buf.extend_from_slice(&[0, 0, 0x00, 0x00, 0x09]);
        enc_object_key(&mut buf, "long");
        buf.extend_from_slice(&[0x0C, 0, 0, 0, 3, b'a', b'b', b'c']);
        enc_object_end(&mut buf);
        ends.push(buf.len());
        enc_null(&mut buf);
        ends.push(buf.len());
        (buf.to_vec(), ends)
    }

    #[test]
    fn every_truncation_of_a_connect_payload_is_an_error_or_a_clean_prefix() {
        // Commands arrive as whole messages, but a peer controls their
        // length. Cut anywhere inside a value, decoding must fail; cut
        // exactly between top-level values, it must return exactly the
        // values before the cut. Never a panic, never a half-built value.
        let (payload, ends) = rich_connect_payload();
        let full = decode_all(&payload).expect("the full payload decodes");
        assert_eq!(full.len(), 4);
        for cut in 0..payload.len() {
            match decode_all(&payload[..cut]) {
                Ok(values) => {
                    let boundary = cut == 0 || ends.contains(&cut);
                    assert!(boundary, "a cut at {cut} decoded mid-value");
                    assert!(values.iter().zip(&full).all(|(v, w)| same(v, w)));
                }
                Err(e) => assert!(!ends.contains(&cut), "a cut at {cut} failed: {e}"),
            }
        }
    }

    #[test]
    fn random_marker_soup_never_panics() {
        // Deterministic LCG, fixed seed. Markers are drawn from the full
        // range the decoder knows (plus a few it doesn't) so each branch
        // sees garbage lengths and counts.
        let mut state: u64 = 0xA3F0_2026;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        };
        for _ in 0..20_000 {
            let len = next() % 24;
            let blob: Vec<u8> = (0..len)
                .map(|_| match next() % 4 {
                    0 => next() % 0x0E,
                    1 => 0x00,
                    _ => next(),
                })
                .collect();
            let _ = decode_all(&blob);
        }
    }
}
