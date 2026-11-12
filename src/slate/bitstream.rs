//! H.264 and HEVC bit-level writing: fixed-width and Exp-Golomb fields,
//! RBSP trailing bits, and NAL unit framing with emulation prevention.

/// MSB-first bit writer for one RBSP.
#[derive(Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    current: u8,
    filled: u8,
    /// Placeholder bits a `continuing` writer starts with, which `append`
    /// leaves out.
    lead_in: u8,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bit(&mut self, on: bool) {
        self.current = (self.current << 1) | on as u8;
        self.filled += 1;
        if self.filled == 8 {
            self.bytes.push(self.current);
            self.current = 0;
            self.filled = 0;
        }
    }

    /// Low `count` bits of `value`, most significant first. u(n).
    pub fn bits(&mut self, value: u32, count: u8) {
        for shift in (0..count).rev() {
            self.bit((value >> shift) & 1 == 1);
        }
    }

    /// Unsigned Exp-Golomb, ue(v).
    pub fn ue(&mut self, value: u32) {
        let code = value as u64 + 1;
        let length = 64 - code.leading_zeros() as u8;
        for _ in 1..length {
            self.bit(false);
        }
        for shift in (0..length).rev() {
            self.bit((code >> shift) & 1 == 1);
        }
    }

    /// Signed Exp-Golomb, se(v): 1, -1, 2, -2, ... map to 1, 2, 3, 4, ...
    pub fn se(&mut self, value: i32) {
        let mapped = if value > 0 {
            value as u32 * 2 - 1
        } else {
            value.unsigned_abs() * 2
        };
        self.ue(mapped);
    }

    /// How many bits have been written.
    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + usize::from(self.filled)
    }

    /// An empty writer for bits that will be appended to `writer`: it
    /// starts at the same place within a byte, so whatever aligns to a
    /// byte in it (I_PCM samples) aligns the same once appended.
    pub fn continuing(writer: &BitWriter) -> Self {
        let mut continuing = Self::new();
        for _ in 0..writer.filled {
            continuing.bit(false);
        }
        continuing.lead_in = writer.filled;
        continuing
    }

    /// Every bit of `other` after its lead-in, in order, after this
    /// writer's own. `other` must continue from where this writer is.
    pub fn append(&mut self, other: &BitWriter) {
        debug_assert_eq!(self.filled, other.lead_in, "appended from another position");
        let whole_bytes = other.bytes.iter().map(|byte| (*byte, 8));
        let last = std::iter::once((other.current, other.filled));
        let mut skip = other.lead_in;
        for (byte, count) in whole_bytes.chain(last) {
            let take = count - skip.min(count);
            self.bits(u32::from(byte), take);
            skip -= skip.min(count);
        }
    }

    /// Every bit of `other`, after this writer's own, wherever within a
    /// byte this writer is: for bits written on their own and joined later
    /// (a slice body after a header written for one stream). `other` must
    /// not depend on where bytes start, so it holds no I_PCM samples.
    pub fn append_unaligned(&mut self, other: &BitWriter) {
        debug_assert_eq!(other.lead_in, 0, "written to continue another writer");
        for byte in &other.bytes {
            self.bits(u32::from(*byte), 8);
        }
        self.bits(u32::from(other.current), other.filled);
    }

    /// Zero bits up to the next byte boundary (pcm_alignment_zero_bit).
    pub fn align_with_zeros(&mut self) {
        while self.filled != 0 {
            self.bit(false);
        }
    }

    /// Whole bytes; the writer must be byte-aligned.
    pub fn bytes(&mut self, data: &[u8]) {
        debug_assert_eq!(self.filled, 0, "raw bytes need byte alignment");
        self.bytes.extend_from_slice(data);
    }

    /// Close the RBSP with rbsp_stop_one_bit and alignment zeros.
    pub fn finish(mut self) -> Vec<u8> {
        self.bit(true);
        self.into_aligned_bytes()
    }

    /// Pad with zeros to a byte boundary and return the bytes, for an RBSP
    /// whose stop bit is already written (a CABAC flush writes its own).
    pub fn into_aligned_bytes(mut self) -> Vec<u8> {
        self.align_with_zeros();
        self.bytes
    }
}

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR_SLICE: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

pub const HEVC_NAL_TRAIL_R: u8 = 1;
pub const HEVC_NAL_IDR_N_LP: u8 = 20;
pub const HEVC_NAL_VPS: u8 = 32;
pub const HEVC_NAL_SPS: u8 = 33;
pub const HEVC_NAL_PPS: u8 = 34;

/// H.264 NAL unit: header byte plus the escaped RBSP.
pub fn nal_unit(nal_ref_idc: u8, nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    escaped(&[(nal_ref_idc << 5) | nal_type], rbsp)
}

/// HEVC NAL unit: two header bytes (layer 0, temporal id 0) plus the
/// escaped RBSP.
pub fn hevc_nal_unit(nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    escaped(&[nal_type << 1, 1], rbsp)
}

/// `header` plus the RBSP with emulation-prevention bytes inserted, so no
/// 00 00 0x (x <= 3) start-code pattern appears inside the payload.
fn escaped(header: &[u8], rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + rbsp.len() + rbsp.len() / 64 + 1);
    out.extend_from_slice(header);
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// One AVCC sample: each NAL prefixed with its 4-byte big-endian length.
pub fn length_prefixed(nals: &[&[u8]]) -> Vec<u8> {
    let total: usize = nals.iter().map(|nal| nal.len() + 4).sum();
    let mut out = Vec::with_capacity(total);
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(fill: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
        let mut writer = BitWriter::new();
        fill(&mut writer);
        writer.finish()
    }

    #[test]
    fn exp_golomb_matches_the_spec_table() {
        // ue: 0 -> 1, 1 -> 010, 2 -> 011, 3 -> 00100. Then the stop bit.
        assert_eq!(written(|w| w.ue(0)), vec![0b1100_0000]);
        assert_eq!(written(|w| w.ue(1)), vec![0b0101_0000]);
        assert_eq!(written(|w| w.ue(3)), vec![0b0010_0100]);
        // se: 1 -> ue(1), -1 -> ue(2).
        assert_eq!(written(|w| w.se(1)), written(|w| w.ue(1)));
        assert_eq!(written(|w| w.se(-1)), written(|w| w.ue(2)));
        assert_eq!(written(|w| w.se(0)), written(|w| w.ue(0)));
    }

    #[test]
    fn bits_are_msb_first() {
        assert_eq!(written(|w| w.bits(0b101, 3)), vec![0b1011_0000]);
        assert_eq!(
            written(|w| w.bits(0xDEAD_BEEF, 32)),
            vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80]
        );
    }

    /// Bits written to a continuing writer land exactly as if written to
    /// the original, byte alignment included.
    #[test]
    fn a_continuing_writer_appends_as_if_written_in_place() {
        let mut direct = BitWriter::new();
        let mut via = BitWriter::new();
        for w in [&mut direct, &mut via] {
            w.bits(0b101, 3);
        }
        let fill = |w: &mut BitWriter| {
            w.ue(4);
            w.align_with_zeros();
            w.bytes(&[0xAB, 0xCD]);
            w.bits(0b11, 2);
        };
        fill(&mut direct);
        let mut scratch = BitWriter::continuing(&via);
        fill(&mut scratch);
        via.append(&scratch);
        assert_eq!(via.bit_len(), direct.bit_len());
        assert_eq!(via.finish(), direct.finish());
    }

    #[test]
    fn emulation_prevention_breaks_start_code_patterns() {
        assert_eq!(
            nal_unit(3, NAL_SPS, &[0, 0, 1, 0, 0, 0, 0]),
            vec![0x67, 0, 0, 3, 1, 0, 0, 3, 0, 0]
        );
        assert_eq!(nal_unit(0, NAL_SLICE, &[0, 0, 4]), vec![0x01, 0, 0, 4]);
        assert_eq!(
            hevc_nal_unit(HEVC_NAL_SPS, &[0, 0, 2]),
            vec![0x42, 0x01, 0, 0, 3, 2]
        );
    }
}
