//! Inter macroblocks for the soft screens' P pictures: the block as the
//! previous picture shows it (P_L0_16x16, zero motion) plus a residual.
//!
//! Soft light drifts a level or two between frames. Redrawing such a block
//! from its neighbours (intra) costs a full set of coefficients; carrying
//! it over and sending the small difference costs a few. Every motion
//! vector in these streams is zero, so each one's prediction (the median of
//! its neighbours') is zero too and its difference codes as two 1 bits.

use super::residual::{self, Block4, ChromaLevels, LevelTooLarge, TokenTable};
use super::{
    add_clipped, difference, is_within, macroblock_of, BitWriter, MbPosition, PictureCoder,
    CHROMA_SAMPLES, LUMA_BLOCK_AT, LUMA_SAMPLES, PCM_BITS, P_SLICE_INTRA_OFFSET, QP_LADDER, ZIGZAG,
};

const MB_TYPE_P_L0_16X16: u32 = 0;
/// coded_block_pattern by codeNum for inter macroblocks (Table 9-4).
#[rustfmt::skip]
const INTER_CBP_BY_CODE: [u32; 48] = [
    0, 16, 1, 2, 4, 8, 32, 3, 5, 10, 12, 15, 47, 7, 11, 13,
    14, 6, 9, 31, 35, 37, 42, 44, 33, 34, 36, 40, 39, 43, 45, 46,
    17, 18, 20, 24, 19, 21, 26, 28, 23, 27, 29, 30, 22, 25, 38, 41,
];

/// One inter macroblock at one QP: its levels, and the samples a decoder
/// rebuilds from them.
struct InterAttempt {
    qp: i32,
    luma: [Block4; 16],
    chroma: [Option<ChromaLevels>; 2],
    shown_luma: [u8; LUMA_SAMPLES],
    shown_chroma: [[u8; CHROMA_SAMPLES]; 2],
}

/// What coding one macroblock changes in the coder, so a candidate can be
/// tried and taken back.
struct MacroblockState {
    luma_counts: [u8; 16],
    chroma_counts: [[u8; 4]; 2],
    luma: [u8; LUMA_SAMPLES],
    chroma: [[u8; CHROMA_SAMPLES]; 2],
    qp: i32,
    ladder_step: usize,
}

impl PictureCoder<'_> {
    /// Code a macroblock of a P picture that can't be skipped: the cheaper
    /// of intra and, when the screen allows a tolerance, inter.
    pub(super) fn code_p_macroblock(&mut self, w: &mut BitWriter, index: usize) {
        let position = self.position(index);
        let before = self.save(position);
        let mut intra = BitWriter::continuing(w);
        self.code_intra(&mut intra, index, P_SLICE_INTRA_OFFSET);
        if self.tolerance == 0 {
            w.append(&intra);
            return;
        }
        let after_intra = self.save(position);
        self.restore(position, &before);
        match self.code_inter(position, w) {
            Some(inter) if inter.bit_len() < intra.bit_len() => w.append(&inter),
            _ => {
                self.restore(position, &after_intra);
                w.append(&intra);
            }
        }
    }

    /// The macroblock as the previous picture shows it plus a residual, at
    /// the coarsest QP on the ladder that lands within the tolerance, as
    /// bits continuing `slice`. `None` when none does more cheaply than
    /// raw samples.
    fn code_inter(&mut self, position: MbPosition, slice: &BitWriter) -> Option<BitWriter> {
        let source = macroblock_of(self.source, position);
        // Not coded yet, so the picture still holds the previous one here.
        let previous = macroblock_of(&self.shown, position);
        let ladder = QP_LADDER.iter().enumerate();
        for (step, qp) in ladder.skip(self.ladder_step.saturating_sub(1)) {
            let Some(attempt) = self.inter_attempt(*qp, &source, &previous) else {
                continue;
            };
            let mut scratch = BitWriter::continuing(slice);
            let codes_qp = match self.write_inter(&mut scratch, position, &attempt) {
                Ok(codes_qp) if scratch.bit_len() < PCM_BITS => codes_qp,
                _ => continue,
            };
            if codes_qp {
                self.qp = attempt.qp;
            }
            self.ladder_step = step;
            self.store(position, &attempt.shown_luma, &attempt.shown_chroma);
            return Some(scratch);
        }
        None
    }

    fn inter_attempt(
        &self,
        qp: i32,
        (source_luma, source_chroma): &([u8; LUMA_SAMPLES], [[u8; CHROMA_SAMPLES]; 2]),
        (previous_luma, previous_chroma): &([u8; LUMA_SAMPLES], [[u8; CHROMA_SAMPLES]; 2]),
    ) -> Option<InterAttempt> {
        let luma = residual::quantize_luma_4x4(&difference(source_luma, previous_luma), qp);
        let shown_luma = add_clipped(previous_luma, &residual::rebuild_luma_4x4(&luma, qp));
        if !is_within(source_luma, &shown_luma, self.tolerance) {
            return None;
        }
        let chroma_qp = residual::chroma_qp(qp);
        let mut chroma = [None, None];
        let mut shown_chroma = *previous_chroma;
        for plane in 0..2 {
            let residual = difference(&source_chroma[plane], &previous_chroma[plane]);
            let levels = residual::quantize_chroma(&residual, chroma_qp);
            let rebuilt = residual::rebuild_chroma(&levels, chroma_qp);
            shown_chroma[plane] = add_clipped(&previous_chroma[plane], &rebuilt);
            if !is_within(&source_chroma[plane], &shown_chroma[plane], self.tolerance) {
                return None;
            }
            chroma[plane] = (levels.has_dc() || levels.has_ac()).then_some(levels);
        }
        Some(InterAttempt {
            qp,
            luma,
            chroma,
            shown_luma,
            shown_chroma,
        })
    }

    /// A P_L0_16x16 macroblock (7.3.5): its type, a zero motion vector
    /// difference, the coded block pattern, then the coded 4x4 blocks.
    /// Answers whether it codes a QP (only macroblocks with levels do).
    fn write_inter(
        &mut self,
        w: &mut BitWriter,
        position: MbPosition,
        attempt: &InterAttempt,
    ) -> Result<bool, LevelTooLarge> {
        let quad_has_levels = |quad: usize| {
            LUMA_BLOCK_AT[4 * quad..4 * quad + 4]
                .iter()
                .any(|(column, row)| attempt.luma[4 * row + column].iter().any(|l| *l != 0))
        };
        let luma_pattern = (0..4)
            .filter(|quad| quad_has_levels(*quad))
            .fold(0, |pattern, quad| pattern | 1 << quad);
        let pattern = luma_pattern | super::chroma_pattern(&attempt.chroma) << 4;
        w.ue(MB_TYPE_P_L0_16X16);
        w.se(0); // mvd_l0, horizontal
        w.se(0); // mvd_l0, vertical
        let code = INTER_CBP_BY_CODE
            .iter()
            .position(|cbp| *cbp == pattern)
            .expect("every pattern has a code");
        w.ue(code as u32);
        self.set_counts(position, 0);
        if pattern == 0 {
            return Ok(false);
        }
        w.se(attempt.qp - self.qp); // mb_qp_delta
        for (blk, (column, row)) in LUMA_BLOCK_AT.iter().enumerate() {
            if luma_pattern & 1 << (blk / 4) == 0 {
                continue;
            }
            let block = &attempt.luma[4 * row + column];
            let levels = ZIGZAG.map(|at| block[at]);
            let n_c = self.luma_n_c(position, *column, *row);
            let total = residual::write_block(w, &levels, TokenTable::Neighbours(n_c))?;
            let at = self.luma_count_index(position, *column, *row);
            self.luma_counts[at] = total as u8;
        }
        self.write_chroma_residual(w, position, &attempt.chroma)?;
        Ok(true)
    }

    fn save(&self, position: MbPosition) -> MacroblockState {
        let (luma, chroma) = macroblock_of(&self.shown, position);
        MacroblockState {
            luma_counts: std::array::from_fn(|at| {
                self.luma_counts[self.luma_count_index(position, at % 4, at / 4)]
            }),
            chroma_counts: [0, 1].map(|plane| {
                std::array::from_fn(|at| {
                    self.chroma_counts[plane][self.chroma_count_index(position, at % 2, at / 2)]
                })
            }),
            luma,
            chroma,
            qp: self.qp,
            ladder_step: self.ladder_step,
        }
    }

    fn restore(&mut self, position: MbPosition, state: &MacroblockState) {
        for at in 0..16 {
            let index = self.luma_count_index(position, at % 4, at / 4);
            self.luma_counts[index] = state.luma_counts[at];
        }
        for plane in 0..2 {
            for at in 0..4 {
                let index = self.chroma_count_index(position, at % 2, at / 2);
                self.chroma_counts[plane][index] = state.chroma_counts[plane][at];
            }
        }
        self.store(position, &state.luma, &state.chroma);
        self.qp = state.qp;
        self.ladder_step = state.ladder_step;
    }
}
