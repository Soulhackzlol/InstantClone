//! Inter macroblocks for P pictures: a block of the previous picture,
//! moved or not (P_L0_16x16), plus a residual.
//!
//! Soft light drifts a level or two between frames. Redrawing such a block
//! from its neighbours (intra) costs a full set of coefficients; carrying
//! it over and sending the small difference costs a few. Moving things
//! cost less still when the block is taken from where they were: a spark
//! from a few pixels lower, a halftone dot from a neighbour already the
//! size it is growing to, the stripe from a little to the left.
//!
//! Motion is in whole luma pixels, both parts even, so chroma (half the
//! size) moves by whole samples too: no prediction is interpolated, and
//! the encoder knows exactly what a decoder rebuilds. A moved block always
//! lies inside the picture, so no decoder fills in an edge either.

use super::residual::{self, Block4, ChromaLevels, LevelTooLarge, TokenTable};
use super::{
    add_clipped, difference, distance, is_within, macroblock_of, read_square, BitWriter,
    MbPosition, PictureCoder, CHROMA_MB, CHROMA_SAMPLES, LUMA_BLOCK_AT, LUMA_SAMPLES, MB, PCM_BITS,
    P_SLICE_INTRA_OFFSET, QP_LADDER, ZIGZAG,
};

/// A macroblock's motion: where in the previous picture its prediction
/// comes from, in luma pixels from its own place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Motion {
    pub x: i32,
    pub y: i32,
}

impl Motion {
    const ZERO: Motion = Motion { x: 0, y: 0 };

    fn moved(self, x: i32, y: i32) -> Motion {
        Motion {
            x: self.x + x,
            y: self.y + y,
        }
    }
}

/// A macroblock's samples: luma, then Cb and Cr.
type Samples = ([u8; LUMA_SAMPLES], [[u8; CHROMA_SAMPLES]; 2]);

/// A neighbouring macroblock as motion prediction sees it (8.4.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Neighbour {
    /// Outside the picture, or not coded yet.
    Missing,
    Intra,
    Inter(Motion),
}

/// Whole blocks a search looks across, each way: a halftone dot's
/// neighbours, near enough that one is often the size it is growing to.
const GRID_REACH: i32 = 2;
/// Steps of the search's last part, which walks to the best spot nearby.
const REFINE_STEPS: [(i32, i32); 8] = [
    (-2, 0),
    (2, 0),
    (0, -2),
    (0, 2),
    (-2, -2),
    (2, -2),
    (-2, 2),
    (2, 2),
];
/// How far the walk may go from where it starts.
const REFINE_REACH: i32 = 16;
/// What a bit of motion costs against a level of difference, in picking
/// where to take a block from (the bits are counted exactly afterwards).
const BITS_WEIGHT: u32 = 4;
/// Motion is coded in quarter pixels.
const QUARTERS: i32 = 4;

/// Bits of `motion` coded against `predicted`: two se(v) codes.
fn motion_bits(motion: Motion, predicted: Motion) -> u32 {
    let se_bits = |value: i32| {
        let code_num = if value > 0 { 2 * value - 1 } else { -2 * value } as u32;
        2 * (code_num + 1).ilog2() + 1
    };
    se_bits(QUARTERS * (motion.x - predicted.x)) + se_bits(QUARTERS * (motion.y - predicted.y))
}

const MB_TYPE_P_L0_16X16: u32 = 0;
/// coded_block_pattern by codeNum for inter macroblocks (Table 9-4).
#[rustfmt::skip]
pub(super) const INTER_CBP_BY_CODE: [u32; 48] = [
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
    motion: Option<Motion>,
    qp: i32,
    ladder_step: usize,
}

impl PictureCoder<'_> {
    /// Code a macroblock of a P picture that can't be skipped: the cheapest
    /// of intra and, when the screen allows a tolerance, inter from where
    /// the search found the closest block and from the same place.
    pub(super) fn code_p_macroblock(&mut self, w: &mut BitWriter, index: usize) {
        let position = self.position(index);
        let before = self.save(position);
        let mut best = BitWriter::continuing(w);
        self.code_intra(&mut best, index, P_SLICE_INTRA_OFFSET);
        if self.tolerance == 0 {
            w.append(&best);
            return;
        }
        let mut best_state = self.save(position);
        let found = self.search_motion(position);
        let tried = if found == Motion::ZERO {
            vec![Motion::ZERO]
        } else {
            vec![found, Motion::ZERO]
        };
        for motion in tried {
            self.restore(position, &before);
            match self.code_inter(position, motion, w) {
                Some(inter) if inter.bit_len() < best.bit_len() => {
                    best = inter;
                    best_state = self.save(position);
                }
                _ => {}
            }
        }
        self.restore(position, &best_state);
        w.append(&best);
    }

    /// The block `motion` points at in the previous picture plus a
    /// residual, at the coarsest QP on the ladder that lands within the
    /// tolerance, as bits continuing `slice`. `None` when none does more
    /// cheaply than raw samples.
    fn code_inter(
        &mut self,
        position: MbPosition,
        motion: Motion,
        slice: &BitWriter,
    ) -> Option<BitWriter> {
        let source = macroblock_of(self.source, position);
        let moved = self.moved_block(position, motion)?;
        let ladder = QP_LADDER.iter().enumerate();
        for (step, qp) in ladder.skip(self.ladder_step.saturating_sub(1)) {
            let Some(attempt) = self.inter_attempt(*qp, &source, &moved) else {
                continue;
            };
            let mut scratch = BitWriter::continuing(slice);
            let codes_qp = match self.write_inter(&mut scratch, position, motion, &attempt) {
                Ok(codes_qp) if scratch.bit_len() < PCM_BITS => codes_qp,
                _ => continue,
            };
            if codes_qp {
                self.qp = attempt.qp;
            }
            self.ladder_step = step;
            let index = self.index_of(position);
            self.motions[index] = Some(motion);
            self.store(position, &attempt.shown_luma, &attempt.shown_chroma);
            return Some(scratch);
        }
        None
    }

    /// Where in the previous picture the block closest to this one's
    /// source is, as a decoder can be told it cheaply: the motions around
    /// and its predicted one, whole blocks nearby, then a walk to the
    /// best spot near the best of those.
    fn search_motion(&self, position: MbPosition) -> Motion {
        let source = macroblock_of(self.source, position);
        let predicted = self.predicted_motion(position);
        let cost = |motion: Motion| {
            let moved = self.moved_block(position, motion)?;
            let bits = motion_bits(motion, predicted);
            Some(distance(&source.0, &moved.0) + BITS_WEIGHT * bits)
        };
        let around = [(-1, 0), (0, -1), (1, -1), (-1, -1)].map(|(dx, dy)| {
            match self.neighbour(position, dx, dy) {
                Neighbour::Inter(motion) => motion,
                _ => Motion::ZERO,
            }
        });
        let block = MB as i32;
        let grid = (-GRID_REACH..=GRID_REACH).flat_map(|gy| {
            (-GRID_REACH..=GRID_REACH).map(move |gx| Motion {
                x: gx * block,
                y: gy * block,
            })
        });
        let starts = [Motion::ZERO, predicted, self.skip_motion(position)]
            .into_iter()
            .chain(around)
            .chain(grid);
        let Some((mut best, mut best_cost)) = starts
            .filter_map(|motion| Some((motion, cost(motion)?)))
            .min_by_key(|(_, cost)| *cost)
        else {
            return Motion::ZERO;
        };
        let start = best;
        loop {
            let near = REFINE_STEPS
                .iter()
                .map(|(x, y)| best.moved(*x, *y))
                .filter(|m| {
                    (m.x - start.x).abs() <= REFINE_REACH && (m.y - start.y).abs() <= REFINE_REACH
                })
                .filter_map(|motion| Some((motion, cost(motion)?)))
                .min_by_key(|(_, cost)| *cost);
            match near {
                Some((motion, cost)) if cost < best_cost => (best, best_cost) = (motion, cost),
                _ => return best,
            }
        }
    }

    /// The previous picture's block `motion` away from `position`, as a
    /// decoder takes it. `None` in an IDR, or when it would reach outside
    /// the picture (a decoder would fill that part in from the edge).
    pub(super) fn moved_block(&self, position: MbPosition, motion: Motion) -> Option<Samples> {
        let reference = self.reference?;
        debug_assert!(
            motion.x % 2 == 0 && motion.y % 2 == 0,
            "whole chroma samples"
        );
        let x = (position.x * MB) as i32 + motion.x;
        let y = (position.y * MB) as i32 + motion.y;
        let fits = |at: i32, side: usize| at >= 0 && at as usize + MB <= side;
        if !fits(x, reference.width) || !fits(y, reference.height) {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        let luma = read_square(&reference.y, reference.width, (x, y));
        let chroma_at = (x / 2, y / 2);
        let chroma = [&reference.u, &reference.v]
            .map(|plane| read_square(plane, reference.width / 2, chroma_at));
        debug_assert_eq!(CHROMA_MB * 2, MB);
        Some((luma, chroma))
    }

    /// The macroblock `dx`, `dy` blocks away, as coded so far.
    fn neighbour(&self, position: MbPosition, dx: isize, dy: isize) -> Neighbour {
        let x = position.x as isize + dx;
        let y = position.y as isize + dy;
        // Blocks are coded row by row: only rows above and blocks to the
        // left on this one are there yet.
        let is_coded = dy < 0 || (dy == 0 && dx < 0);
        if !is_coded || x < 0 || y < 0 || x >= self.width_mbs as isize {
            return Neighbour::Missing;
        }
        match self.motions[y as usize * self.width_mbs + x as usize] {
            None => Neighbour::Intra,
            Some(motion) => Neighbour::Inter(motion),
        }
    }

    /// The motion a decoder predicts for this block from the blocks left,
    /// above and above right of it (above left when that one is missing):
    /// the one of them that has motion when only one does, else the median
    /// of the three (8.4.1.3, for one 16x16 partition).
    pub(super) fn predicted_motion(&self, position: MbPosition) -> Motion {
        let left = self.neighbour(position, -1, 0);
        let mut above = self.neighbour(position, 0, -1);
        let mut above_right = self.neighbour(position, 1, -1);
        if above_right == Neighbour::Missing {
            above_right = self.neighbour(position, -1, -1);
        }
        if above == Neighbour::Missing
            && above_right == Neighbour::Missing
            && left != Neighbour::Missing
        {
            (above, above_right) = (left, left);
        }
        let motions = [left, above, above_right].map(|neighbour| match neighbour {
            Neighbour::Inter(motion) => Some(motion),
            _ => None,
        });
        if let [only] = motions.iter().flatten().collect::<Vec<_>>()[..] {
            return *only;
        }
        let [a, b, c] = motions.map(Option::unwrap_or_default);
        let median = |a: i32, b: i32, c: i32| a.max(b).min(a.min(b).max(c));
        Motion {
            x: median(a.x, b.x, c.x),
            y: median(a.y, b.y, c.y),
        }
    }

    /// The motion a decoder gives a skipped block (8.4.1.1): none at the
    /// picture's top or left edge or beside a block that stayed put, else
    /// the predicted one.
    pub(super) fn skip_motion(&self, position: MbPosition) -> Motion {
        let left = self.neighbour(position, -1, 0);
        let above = self.neighbour(position, 0, -1);
        let still = Neighbour::Inter(Motion::ZERO);
        if left == Neighbour::Missing
            || above == Neighbour::Missing
            || left == still
            || above == still
        {
            return Motion::ZERO;
        }
        self.predicted_motion(position)
    }

    fn index_of(&self, position: MbPosition) -> usize {
        position.y * self.width_mbs + position.x
    }

    fn inter_attempt(
        &self,
        qp: i32,
        (source_luma, source_chroma): &Samples,
        (previous_luma, previous_chroma): &Samples,
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

    /// A P_L0_16x16 macroblock (7.3.5): its type, its motion as the
    /// difference from the predicted one, the coded block pattern, then
    /// the coded 4x4 blocks. Answers whether it codes a QP (only
    /// macroblocks with levels do).
    fn write_inter(
        &mut self,
        w: &mut BitWriter,
        position: MbPosition,
        motion: Motion,
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
        let predicted = self.predicted_motion(position);
        // mvd_l0, horizontal then vertical, in quarter pixels.
        w.se(QUARTERS * (motion.x - predicted.x));
        w.se(QUARTERS * (motion.y - predicted.y));
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
            motion: self.motions[self.index_of(position)],
            qp: self.qp,
            ladder_step: self.ladder_step,
        }
    }

    fn restore(&mut self, position: MbPosition, state: &MacroblockState) {
        let index = self.index_of(position);
        self.motions[index] = state.motion;
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
