//! A luma wipe from whatever a stream last showed into the screen.
//!
//! When a delay transition starts, viewers' decoders still hold the
//! stream's last picture. The wipe is a few P pictures in the stream's
//! own numbering that tell a decoder to take its previous picture, keep a
//! share of each 8x8 block of it and add a share of the screen: the screen
//! washes in across the frozen picture along a soft luma map (a diagonal
//! sweep bent by slow waves), and the screen's loop starts on the picture
//! the wipe ends on. InstantClone never decodes the stream, and doesn't
//! need to: what the decoder keeps of a block is its own business, what
//! it adds is known exactly.
//!
//! How: explicit weighted prediction (8.4.2.3) with one reference picture
//! listed 16 times, each entry carrying another weight; each 8x8 block
//! picks its weight by reference index, and its residual adds the screen.
//! Every wipe picture refers only to the one before, so it works whatever
//! the stream's reference settings. A block is a share `c` of the frozen
//! picture plus the screen part `e` the encoder tracks; carried with
//! weight `w`, it becomes `w * c` of the frozen picture plus `w * e`, and
//! the residual makes the screen part what it should be. Weighting rounds
//! by half a level each picture, which nothing can see.
//!
//! The pictures' macroblock layers depend on the screen alone, so they are
//! built with its loop (`Wipe`); only the slice headers, which carry the
//! stream's own picture numbering, are written when a wipe starts
//! (`wipe_pictures`).

use super::bitstream::{self, BitWriter};
use super::inter::INTER_CBP_BY_CODE;
use super::residual::{self, ChromaLevels, LevelTooLarge, TokenTable};
use super::{chroma_pattern, MbPosition, PictureCoder, LUMA_BLOCK_AT, MB, SLICE_QP, ZIGZAG};
use crate::slate::raster::YuvFrame;

/// Pictures in a wipe: 0.6 s at the screen's 30 fps.
pub const WIPE_PICTURES: usize = 18;
/// How much of the previous picture a block keeps, in 64ths, by
/// reference index. Index 0 keeps it whole, so skipped macroblocks (which
/// take index 0) leave the picture as it was.
const WEIGHTS: [i32; 16] = [64, 0, 6, 12, 18, 24, 30, 36, 40, 44, 48, 52, 55, 58, 60, 62];
/// Weights are in 64ths: luma_log2_weight_denom and its chroma twin.
const LOG2_WEIGHT_DENOM: u32 = 6;
/// The QP a block's blend of the two is coded at while it moves: moving,
/// it hides coarser quantising, and each picture's error is gone in the
/// next (the screen part is tracked exactly as a decoder rebuilds it).
const BLEND_QP: i32 = 28;
/// The QP a block that shows the screen alone is coded at: fine enough
/// that the picture the wipe ends on is within a few levels of the
/// screen's own first frame, which follows it.
const FINISH_QP: i32 = 20;
/// How wide the soft edge of the wipe is, as a share of the map.
const EDGE: f32 = 0.35;
/// A screen part this close to the screen needs no more residual.
const SETTLED: f32 = 1.5;
/// mb_type P_L0_16x16 and P_8x8 in a P slice, and sub_mb_type P_L0_8x8.
const MB_TYPE_P_L0_16X16: u32 = 0;
const MB_TYPE_P_8X8: u32 = 3;
const SUB_MB_TYPE_P_L0_8X8: u32 = 0;
/// Blocks of 8x8 luma samples, the wipe's grain.
const BLOCK: usize = 8;

/// A wipe's pictures, all but their slice headers.
pub struct Wipe {
    /// Each picture's macroblock layer, after its slice header.
    pub(in crate::slate) bodies: Vec<BitWriter>,
    /// The weight entries' offsets, which the slice headers carry.
    offsets: Offsets,
    /// The picture size it was built for, in macroblocks: a stream it
    /// continues must be the same.
    pub width_mbs: usize,
    pub height_mbs: usize,
    /// For tests: the share of the frozen picture each 8x8 block keeps
    /// after each picture, raster by block.
    #[cfg(test)]
    pub kept: Vec<Vec<f32>>,
}

/// What a wipe's slice headers need from the stream they continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamNumbering {
    /// The stream's SPS, which the wipe's pictures are decoded under.
    pub sps_id: u32,
    /// A PPS id the stream doesn't use, for the wipe's own PPS.
    pub pps_id: u32,
    pub log2_max_frame_num: u32,
    /// frame_num of the wipe's first picture: one past the stream's last
    /// reference picture.
    pub first_frame_num: u32,
    /// How many frame_nums back from the first picture the one viewers
    /// see last is, which the wipe starts from: 1 unless a B picture was
    /// a reference after it.
    pub frozen_back: u32,
    pub order: PictureOrder,
}

/// How the stream numbers its pictures for output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureOrder {
    /// pic_order_cnt_type 0: each slice carries the low bits of its
    /// picture's order count. `first` is the wipe's first picture's whole
    /// count, after every picture the stream sent.
    Counted { log2_max_lsb: u32, first: i64 },
    /// pic_order_cnt_type 2: output order follows frame_num.
    FrameNum,
}

/// Build the wipe into `screen` (padded to whole macroblocks).
pub fn build_wipe(screen: &YuvFrame) -> Wipe {
    let offsets = background_offsets(screen);
    let mut state = WipeState::new(screen, offsets);
    let mut bodies = Vec::with_capacity(WIPE_PICTURES);
    #[cfg(test)]
    let mut kept = Vec::with_capacity(WIPE_PICTURES);
    for picture in 1..=WIPE_PICTURES {
        let progress = ease(picture as f32 / WIPE_PICTURES as f32);
        bodies.push(state.code_picture(progress));
        #[cfg(test)]
        kept.push(state.kept.clone());
    }
    Wipe {
        bodies,
        offsets,
        width_mbs: screen.width / MB,
        height_mbs: screen.height / MB,
        #[cfg(test)]
        kept,
    }
}

/// The wipe's pictures as AVCC samples for a stream numbered `numbering`:
/// the first carries the wipe's PPS, every one a reference P picture.
pub fn wipe_pictures(wipe: &Wipe, numbering: &StreamNumbering) -> Vec<Vec<u8>> {
    let pps = bitstream::nal_unit(3, bitstream::NAL_PPS, &pps_rbsp(numbering));
    wipe.bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            let mut w = BitWriter::new();
            write_slice_header(&mut w, numbering, index as u32, &wipe.offsets);
            w.append_unaligned(body);
            let slice = bitstream::nal_unit(2, bitstream::NAL_SLICE, &w.finish());
            if index == 0 {
                bitstream::length_prefixed(&[&pps, &slice])
            } else {
                bitstream::length_prefixed(&[&slice])
            }
        })
        .collect()
}

/// The wipe's PPS: under the stream's SPS, CAVLC, the 16 weighted entries
/// by default, deblocking left to the slices (which switch it off).
fn pps_rbsp(numbering: &StreamNumbering) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(numbering.pps_id);
    w.ue(numbering.sps_id);
    w.bit(false); // entropy_coding_mode_flag: CAVLC
    w.bit(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(WEIGHTS.len() as u32 - 1); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.bit(true); // weighted_pred_flag
    w.bits(0, 2); // weighted_bipred_idc
    w.se(0); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.bit(true); // deblocking_filter_control_present_flag
    w.bit(false); // constrained_intra_pred_flag
    w.bit(false); // redundant_pic_cnt_present_flag
    w.finish()
}

/// The slice header of wipe picture `index` (7.3.3), from 0.
fn write_slice_header(
    w: &mut BitWriter,
    numbering: &StreamNumbering,
    index: u32,
    offsets: &Offsets,
) {
    let max_frame_num = 1u32 << numbering.log2_max_frame_num;
    w.ue(0); // first_mb_in_slice
    w.ue(5); // slice_type: P, as every slice of the picture
    w.ue(numbering.pps_id);
    let frame_num = (numbering.first_frame_num + index) % max_frame_num;
    w.bits(frame_num, numbering.log2_max_frame_num as u8);
    if let PictureOrder::Counted {
        log2_max_lsb,
        first,
    } = numbering.order
    {
        let count = first + 2 * i64::from(index);
        let lsb = count.rem_euclid(1i64 << log2_max_lsb) as u32;
        w.bits(lsb, log2_max_lsb as u8);
    }
    w.bit(false); // num_ref_idx_active_override_flag
                  // ref_pic_list_modification: the picture the wipe carries on from (the
                  // frozen one, then each wipe picture the one before), then the same
                  // one again for every other entry (a whole MaxPicNum round brings the
                  // prediction back to it).
    let back = if index == 0 { numbering.frozen_back } else { 1 };
    w.bit(true); // ref_pic_list_modification_flag_l0
    for entry in 0..WEIGHTS.len() {
        w.ue(0); // modification_of_pic_nums_idc: subtract
        let step = if entry == 0 { back } else { max_frame_num };
        w.ue(step - 1); // abs_diff_pic_num_minus1
    }
    w.ue(3); // end of the list's modifications
    write_weights(w, offsets);
    w.bit(false); // adaptive_ref_pic_marking_mode_flag: sliding window
    w.se(0); // slice_qp_delta
    w.ue(1); // disable_deblocking_filter_idc: off
}

/// pred_weight_table (7.3.3.2): each entry's weight and offset for luma
/// and both chroma planes. Entry 0 keeps the default (whole, no offset).
fn write_weights(w: &mut BitWriter, offsets: &Offsets) {
    w.ue(LOG2_WEIGHT_DENOM); // luma_log2_weight_denom
    w.ue(LOG2_WEIGHT_DENOM); // chroma_log2_weight_denom
    for (entry, weight) in WEIGHTS.iter().enumerate() {
        let explicit = entry != 0;
        w.bit(explicit); // luma_weight_l0_flag
        if explicit {
            w.se(*weight);
            w.se(offsets[entry][0]);
        }
        w.bit(explicit); // chroma_weight_l0_flag
        if explicit {
            for offset in &offsets[entry][1..] {
                w.se(*weight);
                w.se(*offset);
            }
        }
    }
}

/// Each weight entry's offset for luma, Cb and Cr.
type Offsets = [[i32; 3]; WEIGHTS.len()];

/// The offsets that, with each entry's weight, fade a block toward the
/// screen's background instead of toward black: what a block loses of the
/// picture before comes back as that much background. Where the screen is
/// plain background, the weighting alone then makes the blend, and the
/// block needs no residual.
fn background_offsets(screen: &YuvFrame) -> Offsets {
    let background = [&screen.y, &screen.u, &screen.v].map(|plane| most_common(plane));
    // An offset must fit in 8 signed bits (7.4.3.2): neutral chroma (128)
    // faded out whole asks one more, and the residual adds that level.
    WEIGHTS.map(|weight| {
        background.map(|level| {
            let offset = ((64 - weight) as f32 / 64.0 * f32::from(level)).round() as i32;
            offset.clamp(-128, 127)
        })
    })
}

/// The commonest sample of a plane: a screen's background.
fn most_common(samples: &[u8]) -> u8 {
    let mut counts = [0usize; 256];
    for sample in samples {
        counts[usize::from(*sample)] += 1;
    }
    (0..=255u8)
        .max_by_key(|level| counts[usize::from(*level)])
        .unwrap_or(0)
}

/// Smooth in and out.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// How far into the wipe each 8x8 block is reached: a diagonal sweep from
/// the top left, bent by two slow waves, spread over 0..1.
fn luma_map(blocks_wide: usize, blocks_high: usize) -> Vec<f32> {
    let aspect = blocks_wide as f32 / blocks_high as f32;
    let raw: Vec<f32> = (0..blocks_wide * blocks_high)
        .map(|block| {
            let u = (block % blocks_wide) as f32 / blocks_wide as f32;
            let v = (block / blocks_wide) as f32 / blocks_high as f32;
            let diagonal = (u * aspect + v) / (aspect + 1.0);
            let waves =
                (6.0 * u + 2.0 * (5.0 * v).sin()).sin() + (7.0 * v + 1.5 * (4.0 * u).sin()).sin();
            0.8 * diagonal + 0.05 * waves
        })
        .collect();
    let (low, high) = raw.iter().fold((f32::MAX, f32::MIN), |(low, high), value| {
        (low.min(*value), high.max(*value))
    });
    let span = (high - low).max(f32::EPSILON);
    raw.iter().map(|value| (value - low) / span).collect()
}

/// How much of the screen a block reached `at` (0..1) on the map shows at
/// `progress`: none before the soft edge gets there, all once it is past.
fn screen_share(at: f32, progress: f32) -> f32 {
    let t = (((1.0 + EDGE) * progress - at) / EDGE).clamp(0.0, 1.0);
    ease(t)
}

/// What the encoder knows of the wipe so far.
struct WipeState<'a> {
    screen: &'a YuvFrame,
    blocks_wide: usize,
    map: Vec<f32>,
    /// Share of the frozen picture each 8x8 block still shows.
    kept: Vec<f32>,
    /// The screen part of what a decoder shows, sample by sample: luma,
    /// then Cb and Cr.
    luma: Vec<f32>,
    chroma: [Vec<f32>; 2],
    /// Each weight entry's offsets (`background_offsets`).
    offsets: Offsets,
}

/// One 8x8 block's step: the reference index (its weight) and the share
/// of the frozen picture it keeps after it.
#[derive(Clone, Copy)]
struct Step {
    entry: usize,
    kept: f32,
}

impl<'a> WipeState<'a> {
    fn new(screen: &'a YuvFrame, offsets: Offsets) -> Self {
        let blocks_wide = screen.width / BLOCK;
        let blocks_high = screen.height / BLOCK;
        let count = blocks_wide * blocks_high;
        Self {
            screen,
            blocks_wide,
            map: luma_map(blocks_wide, blocks_high),
            kept: vec![1.0; count],
            luma: vec![0.0; screen.y.len()],
            chroma: [vec![0.0; screen.u.len()], vec![0.0; screen.v.len()]],
            offsets,
        }
    }

    /// Code the picture that takes the wipe to `progress`.
    fn code_picture(&mut self, progress: f32) -> BitWriter {
        let mut coder = PictureCoder::new(self.screen, None, 0);
        coder.qp = SLICE_QP;
        let mut w = BitWriter::new();
        let mut skip_run = 0;
        for index in 0..coder.mb_count() {
            let position = coder.position(index);
            let steps = self.steps(position, progress);
            let (luma, chroma) = self.residual(position, &steps);
            let shows_screen_alone = steps.iter().any(|step| step.kept <= 0.0);
            let qp = if shows_screen_alone {
                FINISH_QP
            } else {
                BLEND_QP
            };
            let levels = quantize(&luma, &chroma, qp);
            // Skipped when a decoder would carry it over whole and add
            // nothing: what a skipped block is.
            if steps.iter().all(|step| step.entry == 0) && levels.is_empty() {
                skip_run += 1;
                continue;
            }
            w.ue(skip_run);
            skip_run = 0;
            self.code_macroblock(&mut w, &mut coder, position, (&steps, &luma, &chroma), qp);
        }
        if skip_run > 0 {
            w.ue(skip_run);
        }
        w
    }

    /// Each 8x8 block's weight and what it keeps, raster within the
    /// macroblock.
    fn steps(&self, position: MbPosition, progress: f32) -> [Step; 4] {
        std::array::from_fn(|quarter| {
            let block = self.block_index(position, quarter);
            let before = self.kept[block];
            let target = 1.0 - screen_share(self.map[block], progress);
            plan_step(before, target)
        })
    }

    fn block_index(&self, position: MbPosition, quarter: usize) -> usize {
        let x = position.x * 2 + quarter % 2;
        let y = position.y * 2 + quarter / 2;
        y * self.blocks_wide + x
    }

    /// The residual that makes each block's screen part what its step
    /// needs: the screen times the share it shows, less the screen part
    /// carried over at the block's weight and its entry's offset. Zero for a block whose screen
    /// part is close enough already, as for one the wipe hasn't reached.
    fn residual(&self, position: MbPosition, steps: &[Step; 4]) -> ([i32; 256], [[i32; 64]; 2]) {
        let mut luma = [0; 256];
        let mut chroma = [[0; 64]; 2];
        let (x0, y0) = (position.x * MB, position.y * MB);
        for (quarter, step) in steps.iter().enumerate() {
            let weight = WEIGHTS[step.entry] as f32 / 64.0;
            let offset = self.offsets[step.entry].map(|o| o as f32);
            let shows = 1.0 - step.kept;
            let (bx, by) = (quarter % 2 * BLOCK, quarter / 2 * BLOCK);
            let mut block = [0f32; BLOCK * BLOCK];
            for row in 0..BLOCK {
                for column in 0..BLOCK {
                    let at = (y0 + by + row) * self.screen.width + x0 + bx + column;
                    let wanted = shows * f32::from(self.screen.y[at]);
                    block[row * BLOCK + column] = wanted - weight * self.luma[at] - offset[0];
                }
            }
            let chroma_blocks = [0, 1].map(|plane| {
                let (samples, part) = (self.chroma_plane(plane), &self.chroma[plane]);
                let stride = self.screen.width / 2;
                let mut block = [0f32; 16];
                for row in 0..4 {
                    for column in 0..4 {
                        let at = (y0 / 2 + by / 2 + row) * stride + x0 / 2 + bx / 2 + column;
                        let wanted = shows * f32::from(samples[at]);
                        block[row * 4 + column] = wanted - weight * part[at] - offset[1 + plane];
                    }
                }
                block
            });
            let is_settled = step.entry == 0
                && block.iter().all(|r| r.abs() < SETTLED)
                && chroma_blocks.iter().flatten().all(|r| r.abs() < SETTLED);
            if is_settled {
                continue;
            }
            for row in 0..BLOCK {
                for column in 0..BLOCK {
                    luma[(by + row) * MB + bx + column] =
                        block[row * BLOCK + column].round() as i32;
                }
            }
            for (plane, values) in chroma_blocks.iter().enumerate() {
                for row in 0..4 {
                    for column in 0..4 {
                        chroma[plane][(by / 2 + row) * 8 + bx / 2 + column] =
                            values[row * 4 + column].round() as i32;
                    }
                }
            }
        }
        (luma, chroma)
    }

    fn chroma_plane(&self, plane: usize) -> &[u8] {
        if plane == 0 {
            &self.screen.u
        } else {
            &self.screen.v
        }
    }

    /// A P_8x8 macroblock: each block's reference index (its weight), no
    /// motion, and the residual, at `qp` or a coarser one if a level would
    /// be too large to code.
    fn code_macroblock(
        &mut self,
        w: &mut BitWriter,
        coder: &mut PictureCoder,
        position: MbPosition,
        (steps, luma, chroma): (&[Step; 4], &[i32; 256], &[[i32; 64]; 2]),
        qp: i32,
    ) {
        for qp in (qp..=51).step_by(6) {
            let mut scratch = BitWriter::continuing(w);
            let levels = quantize(luma, chroma, qp);
            if write_p8x8(&mut scratch, coder, position, (steps, &levels, qp)).is_ok() {
                w.append(&scratch);
                self.carry(position, steps, &levels, qp);
                return;
            }
        }
        unreachable!("QP 51 codes any 8-bit residual");
    }

    /// Update what a decoder shows: each block carried at its weight, plus
    /// its entry's offset and the residual as rebuilt.
    fn carry(&mut self, position: MbPosition, steps: &[Step; 4], levels: &Levels, qp: i32) {
        let luma = residual::rebuild_luma_4x4(&levels.luma, qp);
        let chroma_qp = residual::chroma_qp(qp);
        let chroma = levels
            .chroma
            .each_ref()
            .map(|plane| residual::rebuild_chroma(plane, chroma_qp));
        let (x0, y0) = (position.x * MB, position.y * MB);
        let stride = self.screen.width / 2;
        for (quarter, step) in steps.iter().enumerate() {
            let block = self.block_index(position, quarter);
            self.kept[block] = step.kept;
            let weight = WEIGHTS[step.entry] as f32 / 64.0;
            let offset = self.offsets[step.entry].map(|o| o as f32);
            let (bx, by) = (quarter % 2 * BLOCK, quarter / 2 * BLOCK);
            for row in 0..BLOCK {
                for column in 0..BLOCK {
                    let at = (y0 + by + row) * self.screen.width + x0 + bx + column;
                    let added = luma[(by + row) * MB + bx + column] as f32 + offset[0];
                    self.luma[at] = weight * self.luma[at] + added;
                }
            }
            for (plane, rebuilt) in chroma.iter().enumerate() {
                for row in 0..4 {
                    for column in 0..4 {
                        let at = (y0 / 2 + by / 2 + row) * stride + x0 / 2 + bx / 2 + column;
                        let added = rebuilt[(by / 2 + row) * 8 + bx / 2 + column] as f32
                            + offset[1 + plane];
                        self.chroma[plane][at] = weight * self.chroma[plane][at] + added;
                    }
                }
            }
        }
    }
}

/// The weight that takes a block from keeping `before` of the frozen
/// picture to keeping about `target`, and what it keeps then. A block the
/// wipe is done with keeps none, and is carried whole from then on.
fn plan_step(before: f32, target: f32) -> Step {
    if before <= 0.0 {
        return Step {
            entry: 0,
            kept: 0.0,
        };
    }
    if target <= 0.0 {
        return Step {
            entry: 1,
            kept: 0.0,
        };
    }
    let wanted = 64.0 * target / before;
    let entry = (0..WEIGHTS.len())
        .min_by(|a, b| {
            let off = |entry: &usize| (WEIGHTS[*entry] as f32 - wanted).abs();
            off(a).total_cmp(&off(b))
        })
        .unwrap_or(0);
    Step {
        entry,
        kept: before * WEIGHTS[entry] as f32 / 64.0,
    }
}

/// A macroblock's quantised residual.
struct Levels {
    luma: [residual::Block4; 16],
    chroma: [ChromaLevels; 2],
}

impl Levels {
    /// Whether nothing is coded: every level zero.
    fn is_empty(&self) -> bool {
        self.luma.iter().flatten().all(|level| *level == 0)
            && self
                .chroma
                .iter()
                .all(|plane| !plane.has_dc() && !plane.has_ac())
    }
}

fn quantize(luma: &[i32; 256], chroma: &[[i32; 64]; 2], qp: i32) -> Levels {
    let chroma_qp = residual::chroma_qp(qp);
    Levels {
        luma: residual::quantize_luma_4x4(luma, qp),
        chroma: chroma
            .each_ref()
            .map(|plane| residual::quantize_chroma(plane, chroma_qp)),
    }
}

/// P_8x8 (7.3.5, 7.3.5.2): four P_L0_8x8 blocks, their reference indexes,
/// or P_L0_16x16 with one when all four share it,
/// zero motion (every motion in a wipe is zero, so is every prediction of
/// it), the coded block pattern, then the residual.
fn write_p8x8(
    w: &mut BitWriter,
    coder: &mut PictureCoder,
    position: MbPosition,
    (steps, levels, qp): (&[Step; 4], &Levels, i32),
) -> Result<(), LevelTooLarge> {
    // te(v) with a range over 1 codes as ue(v).
    if steps.iter().all(|step| step.entry == steps[0].entry) {
        // One weight for the whole macroblock: one partition says it.
        w.ue(MB_TYPE_P_L0_16X16);
        w.ue(steps[0].entry as u32);
        w.se(0); // mvd_l0, horizontal
        w.se(0); // mvd_l0, vertical
    } else {
        w.ue(MB_TYPE_P_8X8);
        for _ in steps {
            w.ue(SUB_MB_TYPE_P_L0_8X8);
        }
        for step in steps {
            w.ue(step.entry as u32);
        }
        for _ in steps {
            w.se(0); // mvd_l0, horizontal
            w.se(0); // mvd_l0, vertical
        }
    }
    let quad_has_levels = |quad: usize| {
        LUMA_BLOCK_AT[4 * quad..4 * quad + 4]
            .iter()
            .any(|(column, row)| levels.luma[4 * row + column].iter().any(|l| *l != 0))
    };
    let luma_pattern = (0..4)
        .filter(|quad| quad_has_levels(*quad))
        .fold(0, |pattern, quad| pattern | 1 << quad);
    let chroma = levels.chroma.each_ref().map(|plane| Some(plane.clone()));
    let pattern = luma_pattern | chroma_pattern(&chroma) << 4;
    let code = INTER_CBP_BY_CODE
        .iter()
        .position(|cbp| *cbp == pattern)
        .expect("every pattern has a code");
    w.ue(code as u32);
    coder.set_counts(position, 0);
    if pattern == 0 {
        return Ok(());
    }
    w.se(qp - coder.qp); // mb_qp_delta
    coder.qp = qp;
    for (blk, (column, row)) in LUMA_BLOCK_AT.iter().enumerate() {
        if luma_pattern & 1 << (blk / 4) == 0 {
            continue;
        }
        let block = ZIGZAG.map(|at| levels.luma[4 * row + column][at]);
        let n_c = coder.luma_n_c(position, *column, *row);
        let total = residual::write_block(w, &block, TokenTable::Neighbours(n_c))?;
        let at = coder.luma_count_index(position, *column, *row);
        coder.luma_counts[at] = total as u8;
    }
    coder.write_chroma_residual(w, position, &chroma)
}
