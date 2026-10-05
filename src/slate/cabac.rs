//! CABAC arithmetic encoder for the reconnect screen's HEVC slices
//! (H.265 9.3.4.3 and the informative encoder of 9.3.5): context-coded,
//! bypass and terminate bins, plus the restart after PCM samples.
//!
//! The probability tables are the ones H.264 shares with HEVC.

use super::bitstream::BitWriter;

/// rangeTabLps (Table 9-52), indexed by pStateIdx then qRangeIdx.
const RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// transIdxLps (Table 9-53): the state after coding the less probable
/// symbol. After the more probable one the state just counts up to 62.
const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21,
    21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34,
    34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];
const LAST_ADAPTIVE_STATE: u8 = 62;

/// One context variable: probability state and most probable symbol.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    state: u8,
    mps: bool,
}

impl Context {
    /// The state a slice starts from, given the syntax element's
    /// initValue and the slice QP (9.3.2.2).
    pub fn new(init_value: u8, slice_qp: i32) -> Self {
        let slope = i32::from(init_value >> 4) * 5 - 45;
        let offset = (i32::from(init_value & 15) << 3) - 16;
        let state = (((slope * slice_qp.clamp(0, 51)) >> 4) + offset).clamp(1, 126);
        if state <= 63 {
            Self {
                state: (63 - state) as u8,
                mps: false,
            }
        } else {
            Self {
                state: (state - 64) as u8,
                mps: true,
            }
        }
    }
}

/// Arithmetic coder writing into a slice's RBSP after its header.
pub struct CabacWriter {
    out: BitWriter,
    low: u32,
    range: u32,
    outstanding: u32,
    is_first_bit: bool,
}

impl CabacWriter {
    /// Start coding slice data; `out` holds the byte-aligned slice header.
    pub fn new(out: BitWriter) -> Self {
        Self {
            out,
            low: 0,
            range: 510,
            outstanding: 0,
            is_first_bit: true,
        }
    }

    /// EncodeDecision.
    pub fn decision(&mut self, ctx: &mut Context, bin: bool) {
        let lps = u32::from(RANGE_TAB_LPS[ctx.state as usize][((self.range >> 6) & 3) as usize]);
        self.range -= lps;
        if bin == ctx.mps {
            ctx.state = (ctx.state + 1).min(LAST_ADAPTIVE_STATE);
        } else {
            self.low += self.range;
            self.range = lps;
            if ctx.state == 0 {
                ctx.mps = !ctx.mps;
            }
            ctx.state = TRANS_IDX_LPS[ctx.state as usize];
        }
        self.renormalize();
    }

    /// EncodeBypass for each of the low `count` bits of `value`, most
    /// significant first.
    pub fn bypass_bits(&mut self, value: u32, count: u8) {
        for shift in (0..count).rev() {
            self.low <<= 1;
            if (value >> shift) & 1 == 1 {
                self.low += self.range;
            }
            if self.low >= 1024 {
                self.put_bit(true);
                self.low -= 1024;
            } else if self.low < 512 {
                self.put_bit(false);
            } else {
                self.low -= 512;
                self.outstanding += 1;
            }
        }
    }

    /// EncodeTerminate. A 1 ends arithmetic coding (end of slice segment,
    /// or a PCM block about to follow), so it flushes the coder; the last
    /// bit written is a 1 that doubles as the RBSP stop bit.
    pub fn terminate(&mut self, bin: bool) {
        self.range -= 2;
        if !bin {
            self.renormalize();
            return;
        }
        self.low += self.range;
        self.range = 2;
        self.renormalize();
        self.put_bit((self.low >> 9) & 1 == 1);
        self.out.bits(((self.low >> 7) & 3) | 1, 2);
    }

    /// pcm_flag = 1, then the raw samples, then the coder restarts
    /// (9.3.2.5). Contexts carry on unchanged.
    pub fn pcm(&mut self, samples: &[u8]) {
        self.terminate(true);
        self.out.align_with_zeros();
        self.out.bytes(samples);
        *self = Self::new(std::mem::take(&mut self.out));
    }

    /// end_of_slice_segment_flag = 1 and the byte alignment after it.
    pub fn finish(mut self) -> Vec<u8> {
        self.terminate(true);
        self.out.into_aligned_bytes()
    }

    /// RenormE.
    fn renormalize(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(false);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(true);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    /// PutBit: the bit, then any outstanding bits as its complement.
    fn put_bit(&mut self, bit: bool) {
        if self.is_first_bit {
            self.is_first_bit = false;
        } else {
            self.out.bit(bit);
        }
        while self.outstanding > 0 {
            self.out.bit(!bit);
            self.outstanding -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 9.3.2.2 worked by hand at QP 26. 154: slope 0, offset 64, so state
    /// 64 (equiprobable, MPS 1). 139: (-5 * 26) >> 4 = -9 (an arithmetic
    /// shift), + 72 = 63 (equiprobable, MPS 0). 197: 390 >> 4 = 24, + 24 =
    /// 48, so pStateIdx 15 with MPS 0.
    #[test]
    fn contexts_start_where_the_spec_says() {
        let equal_one = Context::new(154, 26);
        assert_eq!((equal_one.state, equal_one.mps), (0, true));
        let equal_zero = Context::new(139, 26);
        assert_eq!((equal_zero.state, equal_zero.mps), (0, false));
        let zero_likely = Context::new(197, 26);
        assert_eq!((zero_likely.state, zero_likely.mps), (15, false));
    }

    /// A slice that is only end_of_slice_segment_flag = 1. A decoder reads
    /// 9 bits as its offset (509 here) and decodes a 1 because that is at
    /// least the range left after the terminate (510 - 2). The last of
    /// those 9 bits doubles as the stop bit; alignment zeros follow.
    #[test]
    fn an_empty_slice_flushes_to_its_stop_bit() {
        let bytes = CabacWriter::new(BitWriter::new()).finish();
        assert_eq!(bytes, vec![0b1111_1110, 0b1000_0000]);
    }
}
