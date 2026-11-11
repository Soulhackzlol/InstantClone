//! H.264 encoder for the screens InstantClone draws itself.
//!
//! It needs four macroblock types, which is what keeps it small:
//! - I_16x16 with no residual: the block is a vertical, horizontal, DC or
//!   plane prediction from its neighbours (flat areas, even ramps).
//! - I_16x16 with residual: a prediction plus transform coefficients, for
//!   soft detail (gradients, glows). Only screens that allow a tolerance
//!   use it.
//! - I_PCM: the 384 raw samples, for anything else.
//! - P_Skip: close enough to the previous frame already.
//!
//! A screen coded with tolerance 0 is lossless: every macroblock
//! reconstructs to exactly its source samples. With a tolerance, every
//! sample lands within that many levels of its source, and the encoder
//! keeps the picture a decoder really shows, so predictions and skips are
//! judged against that, never against the source. Deblocking is switched
//! off in every slice so nothing touches the reconstruction afterwards.
//!
//! Stream shape: Constrained Baseline, CAVLC, one slice per picture, a
//! single reference frame, POC type 2 (output order = decode order).

use super::bitstream::{self, BitWriter};
use super::raster::YuvFrame;
use super::residual::{
    self, ChromaLevels, LevelTooLarge, LumaLevels, TokenTable, LUMA_BLOCK_AT, ZIGZAG,
};

const MB: usize = 16;
const CHROMA_MB: usize = 8;
const LOG2_MAX_FRAME_NUM: u8 = 8;
/// Parameter-set ids for the reconnect screen. Streams from OBS use id 0,
/// so the screen's own SPS/PPS sit beside the stream's in a decoder instead
/// of replacing them: switching to the screen and back needs only a
/// keyframe, never a new sequence header (which decoders don't reliably
/// apply mid-stream).
const SPS_ID: u32 = 1;
const PPS_ID: u32 = 1;
const PROFILE_BASELINE: u32 = 66;
/// constraint_set0 + constraint_set1: Constrained Baseline.
const CONSTRAINT_FLAGS: u32 = 0b1100_0000;
const MB_TYPE_I_PCM: u32 = 25;
/// Intra mb_type values are offset by 5 inside P slices.
const P_SLICE_INTRA_OFFSET: u32 = 5;
const SLICE_TYPE_P: u32 = 5;
const SLICE_TYPE_I: u32 = 7;
/// nC reported for an I_PCM neighbour (spec 9.2.1).
const PCM_TOTAL_COEFF: u32 = 16;

/// Visible size and frame rate of the stream being encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamShape {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
}

/// SPS and PPS NAL units (header included, no length prefix).
pub struct ParameterSets {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
}

pub fn parameter_sets(shape: StreamShape) -> ParameterSets {
    ParameterSets {
        sps: bitstream::nal_unit(3, bitstream::NAL_SPS, &sps_rbsp(shape)),
        pps: bitstream::nal_unit(3, bitstream::NAL_PPS, &pps_rbsp()),
    }
}

fn sps_rbsp(shape: StreamShape) -> Vec<u8> {
    let width_mbs = shape.width.div_ceil(MB);
    let height_mbs = shape.height.div_ceil(MB);
    let mut w = BitWriter::new();
    w.bits(PROFILE_BASELINE, 8);
    w.bits(CONSTRAINT_FLAGS, 8);
    w.bits(level_idc(width_mbs * height_mbs, shape.fps), 8);
    w.ue(SPS_ID);
    w.ue(LOG2_MAX_FRAME_NUM as u32 - 4);
    w.ue(2); // pic_order_cnt_type: POC follows frame_num
    w.ue(1); // max_num_ref_frames
    w.bit(false); // gaps_in_frame_num_value_allowed_flag
    w.ue(width_mbs as u32 - 1);
    w.ue(height_mbs as u32 - 1);
    w.bit(true); // frame_mbs_only_flag
    w.bit(true); // direct_8x8_inference_flag
    let crop_right = (width_mbs * MB - shape.width) / 2;
    let crop_bottom = (height_mbs * MB - shape.height) / 2;
    let needs_crop = crop_right > 0 || crop_bottom > 0;
    w.bit(needs_crop);
    if needs_crop {
        // Crop units are 2 luma samples in 4:2:0.
        w.ue(0);
        w.ue(crop_right as u32);
        w.ue(0);
        w.ue(crop_bottom as u32);
    }
    w.bit(true); // vui_parameters_present_flag
    write_vui(&mut w, shape.fps);
    w.finish()
}

/// BT.709 limited range (matches `raster`), fixed frame rate, and no
/// frame reordering so players can show each frame as it arrives.
fn write_vui(w: &mut BitWriter, fps: u32) {
    w.bit(false); // aspect_ratio_info_present_flag
    w.bit(false); // overscan_info_present_flag
    w.bit(true); // video_signal_type_present_flag
    w.bits(5, 3); // video_format: unspecified
    w.bit(false); // video_full_range_flag
    w.bit(true); // colour_description_present_flag
    w.bits(1, 8); // colour_primaries: BT.709
    w.bits(1, 8); // transfer_characteristics: BT.709
    w.bits(1, 8); // matrix_coefficients: BT.709
    w.bit(false); // chroma_loc_info_present_flag
    w.bit(true); // timing_info_present_flag
    w.bits(1, 32); // num_units_in_tick
    w.bits(fps * 2, 32); // time_scale: two ticks per frame
    w.bit(true); // fixed_frame_rate_flag
    w.bit(false); // nal_hrd_parameters_present_flag
    w.bit(false); // vcl_hrd_parameters_present_flag
    w.bit(false); // pic_struct_present_flag
    w.bit(true); // bitstream_restriction_flag
    w.bit(true); // motion_vectors_over_pic_boundaries_flag
    w.ue(0); // max_bytes_per_pic_denom: no limit
    w.ue(0); // max_bits_per_mb_denom: no limit
    w.ue(16); // log2_max_mv_length_horizontal
    w.ue(16); // log2_max_mv_length_vertical
    w.ue(0); // max_num_reorder_frames
    w.ue(1); // max_dec_frame_buffering
}

/// Smallest level (Table A-1) whose frame size and macroblock rate fit.
fn level_idc(frame_mbs: usize, fps: u32) -> u32 {
    const LEVELS: [(u32, usize, usize); 10] = [
        (30, 1_620, 40_500),
        (31, 3_600, 108_000),
        (32, 5_120, 216_000),
        (40, 8_192, 245_760),
        (42, 8_704, 522_240),
        (50, 22_080, 589_824),
        (51, 36_864, 983_040),
        (52, 36_864, 2_073_600),
        (60, 139_264, 4_177_920),
        (61, 139_264, 8_355_840),
    ];
    let mb_rate = frame_mbs * fps as usize;
    LEVELS
        .iter()
        .find(|(_, max_fs, max_mbps)| frame_mbs <= *max_fs && mb_rate <= *max_mbps)
        .map_or(62, |(level, _, _)| *level)
}

fn pps_rbsp() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(PPS_ID);
    w.ue(SPS_ID);
    w.bit(false); // entropy_coding_mode_flag: CAVLC
    w.bit(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.bit(false); // weighted_pred_flag
    w.bits(0, 2); // weighted_bipred_idc
    w.se(0); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.bit(true); // deblocking_filter_control_present_flag
    w.bit(false); // constrained_intra_pred_flag
    w.bit(false); // redundant_pic_cnt_present_flag
    w.finish()
}

/// A coded picture, and the picture a decoder shows for it, which is what
/// the next picture is coded against.
pub struct Coded {
    pub nal: Vec<u8>,
    pub shown: YuvFrame,
}

/// IDR picture, exact. Consecutive IDRs must differ in `idr_pic_id`, so a
/// loop that replays its keyframe alternates between two ids.
pub fn encode_idr(frame: &YuvFrame, idr_pic_id: u32) -> Vec<u8> {
    encode_idr_within(frame, idr_pic_id, 0).nal
}

/// IDR picture whose every sample lands within `tolerance` of `frame`.
pub fn encode_idr_within(frame: &YuvFrame, idr_pic_id: u32, tolerance: u8) -> Coded {
    let mut w = BitWriter::new();
    w.ue(0); // first_mb_in_slice
    w.ue(SLICE_TYPE_I);
    w.ue(PPS_ID);
    w.bits(0, LOG2_MAX_FRAME_NUM); // frame_num
    w.ue(idr_pic_id);
    w.bit(false); // no_output_of_prior_pics_flag
    w.bit(false); // long_term_reference_flag
    write_slice_tail(&mut w);
    let mut coder = PictureCoder::new(frame, None, tolerance);
    for index in 0..coder.mb_count() {
        coder.code_intra(&mut w, index, 0);
    }
    Coded {
        nal: bitstream::nal_unit(3, bitstream::NAL_IDR_SLICE, &w.finish()),
        shown: coder.shown,
    }
}

/// P picture, exact: blocks identical to `previous` are skipped, the rest
/// are intra coded. Every P picture is a reference so the next one can
/// skip against it; `frame_num` counts up from 1 after the IDR.
pub fn encode_p(frame: &YuvFrame, previous: &YuvFrame, frame_num: u32) -> Vec<u8> {
    encode_p_within(frame, previous, frame_num, 0).nal
}

/// P picture whose every sample lands within `tolerance` of `frame`.
/// `shown_before` is what the decoder shows for the picture before it: a
/// block already that close to `frame` is skipped.
pub fn encode_p_within(
    frame: &YuvFrame,
    shown_before: &YuvFrame,
    frame_num: u32,
    tolerance: u8,
) -> Coded {
    let mut w = BitWriter::new();
    w.ue(0); // first_mb_in_slice
    w.ue(SLICE_TYPE_P);
    w.ue(PPS_ID);
    w.bits(frame_num % (1 << LOG2_MAX_FRAME_NUM), LOG2_MAX_FRAME_NUM);
    w.bit(false); // num_ref_idx_active_override_flag
    w.bit(false); // ref_pic_list_modification_flag_l0
    w.bit(false); // adaptive_ref_pic_marking_mode_flag
    write_slice_tail(&mut w);

    let mut coder = PictureCoder::new(frame, Some(shown_before), tolerance);
    let mut skip_run = 0;
    for index in 0..coder.mb_count() {
        if coder.is_already_shown(index) {
            skip_run += 1;
            continue;
        }
        w.ue(skip_run);
        skip_run = 0;
        coder.code_intra(&mut w, index, P_SLICE_INTRA_OFFSET);
    }
    if skip_run > 0 {
        w.ue(skip_run);
    }
    Coded {
        nal: bitstream::nal_unit(2, bitstream::NAL_SLICE, &w.finish()),
        shown: coder.shown,
    }
}

/// slice_qp_delta and the deblocking switch, shared by both slice types.
fn write_slice_tail(w: &mut BitWriter) {
    w.se(0); // slice_qp_delta
    w.ue(1); // disable_deblocking_filter_idc: off
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MbPosition {
    x: usize,
    y: usize,
}

const LUMA_VERTICAL: u32 = 0;
const LUMA_HORIZONTAL: u32 = 1;
const LUMA_DC: u32 = 2;
const LUMA_PLANE: u32 = 3;
/// Intra16x16PredMode numbers, in the order they are tried.
const LUMA_MODES: [u32; 4] = [LUMA_VERTICAL, LUMA_HORIZONTAL, LUMA_DC, LUMA_PLANE];
const CHROMA_DC: u32 = 0;
const CHROMA_HORIZONTAL: u32 = 1;
const CHROMA_VERTICAL: u32 = 2;
const CHROMA_PLANE: u32 = 3;
/// intra_chroma_pred_mode numbers, in the order they are tried.
const CHROMA_MODES: [u32; 4] = [CHROMA_DC, CHROMA_HORIZONTAL, CHROMA_VERTICAL, CHROMA_PLANE];

const LUMA_SAMPLES: usize = MB * MB;
const CHROMA_SAMPLES: usize = CHROMA_MB * CHROMA_MB;
/// The QP every slice starts at: pic_init_qp with no slice delta.
const SLICE_QP: i32 = 26;
/// The quantisers a block with residual is tried at, coarsest first; it
/// takes the first that lands within the tolerance.
const QP_LADDER: [i32; 6] = [34, 28, 22, 16, 10, 4];
/// What an I_PCM macroblock costs: past this, raw samples are cheaper.
const PCM_BITS: usize = 8 * (LUMA_SAMPLES + 2 * CHROMA_SAMPLES);

/// A luma prediction, and whether it is close enough to use as it is.
struct LumaPick {
    mode: u32,
    prediction: [u8; LUMA_SAMPLES],
    needs_residual: bool,
}

/// The one mode Cb and Cr share, with each plane's prediction.
struct ChromaPick {
    mode: u32,
    prediction: [[u8; CHROMA_SAMPLES]; 2],
    needs_residual: bool,
}

/// One macroblock coded with residual at one QP: the levels, and the
/// samples a decoder rebuilds from them.
struct Attempt {
    qp: i32,
    luma: Option<LumaLevels>,
    chroma: [Option<ChromaLevels>; 2],
    shown_luma: [u8; LUMA_SAMPLES],
    shown_chroma: [[u8; CHROMA_SAMPLES]; 2],
}

/// Codes one picture's macroblocks, keeping the picture a decoder shows
/// (predictions read it) and what later blocks' syntax depends on.
struct PictureCoder<'a> {
    source: &'a YuvFrame,
    /// The decoder's picture: the one before, until a block is coded.
    shown: YuvFrame,
    width_mbs: usize,
    tolerance: i32,
    /// QP of the last macroblock that set one; mb_qp_delta is against it.
    qp: i32,
    /// Where on `QP_LADDER` the last block with residual landed.
    ladder_step: usize,
    /// TotalCoeff of every 4x4 luma block, for the coeff_token tables of
    /// the blocks right of and below it.
    luma_counts: Vec<u8>,
    /// The same for the 4x4 blocks of Cb and Cr.
    chroma_counts: [Vec<u8>; 2],
}

impl<'a> PictureCoder<'a> {
    fn new(source: &'a YuvFrame, shown_before: Option<&YuvFrame>, tolerance: u8) -> Self {
        let width_mbs = source.width / MB;
        let mb_count = width_mbs * (source.height / MB);
        Self {
            source,
            shown: shown_before.unwrap_or(source).clone(),
            width_mbs,
            tolerance: i32::from(tolerance),
            qp: SLICE_QP,
            ladder_step: 0,
            luma_counts: vec![0; mb_count * 16],
            chroma_counts: [vec![0; mb_count * 4], vec![0; mb_count * 4]],
        }
    }

    fn mb_count(&self) -> usize {
        self.luma_counts.len() / 16
    }

    fn position(&self, index: usize) -> MbPosition {
        MbPosition {
            x: index % self.width_mbs,
            y: index / self.width_mbs,
        }
    }

    /// Whether the decoder's picture already has this macroblock within
    /// the tolerance, so a P picture can skip it.
    fn is_already_shown(&self, index: usize) -> bool {
        let position = self.position(index);
        let shown = macroblock_of(&self.shown, position);
        let source = macroblock_of(self.source, position);
        is_within(&source.0, &shown.0, self.tolerance)
            && is_within(&source.1[0], &shown.1[0], self.tolerance)
            && is_within(&source.1[1], &shown.1[1], self.tolerance)
    }

    /// Code macroblock `index` as intra: a plain prediction when one is
    /// close enough, else a prediction plus residual, else raw samples.
    fn code_intra(&mut self, w: &mut BitWriter, index: usize, type_offset: u32) {
        let position = self.position(index);
        let (luma_source, chroma_source) = macroblock_of(self.source, position);
        let luma = pick_luma(
            &luma_source,
            &luma_predictions(&self.shown, position),
            self.tolerance,
        );
        let chroma = pick_chroma(
            &chroma_source,
            &chroma_predictions(&self.shown, position),
            self.tolerance,
        );
        let (Some(luma), Some(chroma)) = (luma, chroma) else {
            self.write_pcm(w, position, type_offset);
            return;
        };
        if !luma.needs_residual && !chroma.needs_residual {
            // I_16x16_<mode>_0_0: both coded block patterns zero.
            w.ue(type_offset + 1 + luma.mode);
            w.ue(chroma.mode);
            w.se(0); // mb_qp_delta
            self.set_counts(position, 0);
            let n_c = self.luma_n_c(position, 0, 0);
            residual::write_block(w, &[0; 16], TokenTable::Neighbours(n_c))
                .expect("an empty block has no level to refuse");
            self.store(position, &luma.prediction, &chroma.prediction);
            return;
        }
        let sources = (&luma_source, &chroma_source);
        let ladder = QP_LADDER.iter().enumerate();
        for (step, qp) in ladder.skip(self.ladder_step.saturating_sub(1)) {
            let Some(attempt) = self.attempt(*qp, sources, &luma, &chroma) else {
                continue;
            };
            let mut scratch = BitWriter::new();
            let modes = (luma.mode, chroma.mode);
            let written =
                self.write_with_residual(&mut scratch, position, type_offset, modes, &attempt);
            if written.is_err() || scratch.bit_len() >= PCM_BITS {
                continue;
            }
            w.append(&scratch);
            self.qp = attempt.qp;
            self.ladder_step = step;
            self.store(position, &attempt.shown_luma, &attempt.shown_chroma);
            return;
        }
        self.write_pcm(w, position, type_offset);
    }

    /// The decoder's samples for a macroblock.
    fn store(
        &mut self,
        position: MbPosition,
        luma: &[u8; LUMA_SAMPLES],
        chroma: &[[u8; CHROMA_SAMPLES]; 2],
    ) {
        let width = self.shown.width;
        write_square(
            &mut self.shown.y,
            width,
            (position.x * MB, position.y * MB),
            luma,
        );
        let at = (position.x * CHROMA_MB, position.y * CHROMA_MB);
        write_square(&mut self.shown.u, width / 2, at, &chroma[0]);
        write_square(&mut self.shown.v, width / 2, at, &chroma[1]);
    }

    /// Quantise the block's residual at `qp` and rebuild it as a decoder
    /// will. `None` when a sample lands outside the tolerance.
    fn attempt(
        &self,
        qp: i32,
        (luma_source, chroma_source): (&[u8; LUMA_SAMPLES], &[[u8; CHROMA_SAMPLES]; 2]),
        luma: &LumaPick,
        chroma: &ChromaPick,
    ) -> Option<Attempt> {
        let mut attempt = Attempt {
            qp,
            luma: None,
            chroma: [None, None],
            shown_luma: luma.prediction,
            shown_chroma: chroma.prediction,
        };
        if luma.needs_residual {
            let residual = difference(luma_source, &luma.prediction);
            let levels = residual::quantize_luma(&residual, qp);
            let rebuilt = residual::rebuild_luma(&levels, qp);
            attempt.shown_luma = add_clipped(&luma.prediction, &rebuilt);
            attempt.luma = Some(levels);
        }
        if chroma.needs_residual {
            let chroma_qp = residual::chroma_qp(qp);
            for (plane, source) in chroma_source.iter().enumerate() {
                let residual = difference(source, &chroma.prediction[plane]);
                let levels = residual::quantize_chroma(&residual, chroma_qp);
                let rebuilt = residual::rebuild_chroma(&levels, chroma_qp);
                attempt.shown_chroma[plane] = add_clipped(&chroma.prediction[plane], &rebuilt);
                attempt.chroma[plane] = Some(levels);
            }
        }
        let is_close = is_within(luma_source, &attempt.shown_luma, self.tolerance)
            && is_within(&chroma_source[0], &attempt.shown_chroma[0], self.tolerance)
            && is_within(&chroma_source[1], &attempt.shown_chroma[1], self.tolerance);
        is_close.then_some(attempt)
    }

    /// An Intra16x16 macroblock with its residual (7.3.5): the type names
    /// the luma mode and which coefficients follow. `modes` are the luma
    /// and chroma prediction modes.
    fn write_with_residual(
        &mut self,
        w: &mut BitWriter,
        position: MbPosition,
        type_offset: u32,
        (luma_mode, chroma_mode): (u32, u32),
        attempt: &Attempt,
    ) -> Result<(), LevelTooLarge> {
        let has_luma_ac = attempt.luma.as_ref().is_some_and(LumaLevels::has_ac);
        let chroma_levels = || attempt.chroma.iter().flatten();
        let chroma_pattern = if chroma_levels().any(ChromaLevels::has_ac) {
            2
        } else {
            u32::from(chroma_levels().any(ChromaLevels::has_dc))
        };
        let luma_pattern = if has_luma_ac { 12 } else { 0 };
        w.ue(type_offset + 1 + luma_mode + 4 * chroma_pattern + luma_pattern);
        w.ue(chroma_mode);
        w.se(attempt.qp - self.qp); // mb_qp_delta
        self.set_counts(position, 0);

        // Intra16x16DCLevel takes its nC as block 0 would.
        let dc = attempt
            .luma
            .as_ref()
            .map_or([0; 16], |levels| ZIGZAG.map(|at| levels.dc[at]));
        let n_c = self.luma_n_c(position, 0, 0);
        residual::write_block(w, &dc, TokenTable::Neighbours(n_c))?;
        if let (true, Some(levels)) = (has_luma_ac, &attempt.luma) {
            for (column, row) in LUMA_BLOCK_AT {
                let block = &levels.ac[4 * row + column];
                let ac: [i32; 15] = std::array::from_fn(|at| block[ZIGZAG[at + 1]]);
                let n_c = self.luma_n_c(position, column, row);
                let total = residual::write_block(w, &ac, TokenTable::Neighbours(n_c))?;
                let at = self.luma_count_index(position, column, row);
                self.luma_counts[at] = total as u8;
            }
        }
        if chroma_pattern == 0 {
            return Ok(());
        }
        for levels in &attempt.chroma {
            let dc = levels.as_ref().map_or([0; 4], |levels| levels.dc);
            residual::write_block(w, &dc, TokenTable::ChromaDc)?;
        }
        if chroma_pattern == 1 {
            return Ok(());
        }
        for (plane, levels) in attempt.chroma.iter().enumerate() {
            for block in 0..4 {
                let (column, row) = (block % 2, block / 2);
                let ac: [i32; 15] = match levels {
                    Some(levels) => std::array::from_fn(|at| levels.ac[block][ZIGZAG[at + 1]]),
                    None => [0; 15],
                };
                let n_c = self.chroma_n_c(plane, position, column, row);
                let total = residual::write_block(w, &ac, TokenTable::Neighbours(n_c))?;
                let at = self.chroma_count_index(position, column, row);
                self.chroma_counts[plane][at] = total as u8;
            }
        }
        Ok(())
    }

    /// I_PCM: the raw samples, which the decoder shows as they are.
    fn write_pcm(&mut self, w: &mut BitWriter, position: MbPosition, type_offset: u32) {
        w.ue(type_offset + MB_TYPE_I_PCM);
        w.align_with_zeros();
        let (luma, chroma) = macroblock_of(self.source, position);
        w.bytes(&luma);
        w.bytes(&chroma[0]);
        w.bytes(&chroma[1]);
        self.set_counts(position, PCM_TOTAL_COEFF as u8);
        self.store(position, &luma, &chroma);
    }

    /// Every 4x4 block of the macroblock reports `count` coefficients.
    fn set_counts(&mut self, position: MbPosition, count: u8) {
        for row in 0..4 {
            for column in 0..4 {
                let at = self.luma_count_index(position, column, row);
                self.luma_counts[at] = count;
            }
        }
        for row in 0..2 {
            for column in 0..2 {
                let at = self.chroma_count_index(position, column, row);
                self.chroma_counts[0][at] = count;
                self.chroma_counts[1][at] = count;
            }
        }
    }

    fn luma_count_index(&self, position: MbPosition, column: usize, row: usize) -> usize {
        (4 * position.y + row) * (4 * self.width_mbs) + 4 * position.x + column
    }

    fn chroma_count_index(&self, position: MbPosition, column: usize, row: usize) -> usize {
        (2 * position.y + row) * (2 * self.width_mbs) + 2 * position.x + column
    }

    /// nC (9.2.1) for the luma block at (`column`, `row`) of a macroblock.
    fn luma_n_c(&self, position: MbPosition, column: usize, row: usize) -> u32 {
        let at = self.luma_count_index(position, column, row);
        let has_left = 4 * position.x + column > 0;
        let has_top = 4 * position.y + row > 0;
        n_c(&self.luma_counts, at, 4 * self.width_mbs, has_left, has_top)
    }

    fn chroma_n_c(&self, plane: usize, position: MbPosition, column: usize, row: usize) -> u32 {
        let at = self.chroma_count_index(position, column, row);
        let has_left = 2 * position.x + column > 0;
        let has_top = 2 * position.y + row > 0;
        let stride = 2 * self.width_mbs;
        n_c(&self.chroma_counts[plane], at, stride, has_left, has_top)
    }
}

/// nC from the coefficient counts of the blocks left of and above `at`:
/// their rounded mean, or the one there is, or 0.
fn n_c(counts: &[u8], at: usize, stride: usize, has_left: bool, has_top: bool) -> u32 {
    let left = has_left.then(|| u32::from(counts[at - 1]));
    let top = has_top.then(|| u32::from(counts[at - stride]));
    match (left, top) {
        (Some(a), Some(b)) => (a + b + 1) >> 1,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 0,
    }
}

/// One macroblock of `frame`: its luma, then Cb and Cr.
fn macroblock_of(
    frame: &YuvFrame,
    position: MbPosition,
) -> ([u8; LUMA_SAMPLES], [[u8; CHROMA_SAMPLES]; 2]) {
    let luma = read_square(&frame.y, frame.width, (position.x * MB, position.y * MB));
    let at = (position.x * CHROMA_MB, position.y * CHROMA_MB);
    let chroma = [&frame.u, &frame.v].map(|plane| read_square(plane, frame.width / 2, at));
    (luma, chroma)
}

/// The `N`-sample square (16x16 or 8x8) with its corner at `(x0, y0)`.
fn read_square<const N: usize>(samples: &[u8], stride: usize, (x0, y0): (usize, usize)) -> [u8; N] {
    let size = N.isqrt();
    let mut block = [0; N];
    for row in 0..size {
        let start = (y0 + row) * stride + x0;
        block[row * size..(row + 1) * size].copy_from_slice(&samples[start..start + size]);
    }
    block
}

fn write_square<const N: usize>(
    samples: &mut [u8],
    stride: usize,
    (x0, y0): (usize, usize),
    block: &[u8; N],
) {
    let size = N.isqrt();
    for row in 0..size {
        let start = (y0 + row) * stride + x0;
        samples[start..start + size].copy_from_slice(&block[row * size..(row + 1) * size]);
    }
}

fn is_within<const N: usize>(a: &[u8; N], b: &[u8; N], tolerance: i32) -> bool {
    if tolerance == 0 {
        return a == b;
    }
    a.iter()
        .zip(b)
        .all(|(a, b)| i32::from(a.abs_diff(*b)) <= tolerance)
}

fn difference<const N: usize>(source: &[u8; N], prediction: &[u8; N]) -> [i32; N] {
    std::array::from_fn(|at| i32::from(source[at]) - i32::from(prediction[at]))
}

fn add_clipped<const N: usize>(prediction: &[u8; N], residual: &[i32; N]) -> [u8; N] {
    std::array::from_fn(|at| (i32::from(prediction[at]) + residual[at]).clamp(0, 255) as u8)
}

/// Sum of absolute differences.
fn distance<const N: usize>(a: &[u8; N], b: &[u8; N]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .sum()
}

/// The first luma mode whose prediction is within the tolerance; failing
/// that, when residual is allowed, the closest one. `None` means raw
/// samples.
fn pick_luma(
    source: &[u8; LUMA_SAMPLES],
    predictions: &[Option<[u8; LUMA_SAMPLES]>; 4],
    tolerance: i32,
) -> Option<LumaPick> {
    let available = || {
        LUMA_MODES
            .iter()
            .zip(predictions)
            .filter_map(|(mode, prediction)| Some((*mode, prediction.as_ref()?)))
    };
    let pick = |(mode, prediction): (u32, &[u8; LUMA_SAMPLES]), needs_residual| LumaPick {
        mode,
        prediction: *prediction,
        needs_residual,
    };
    if let Some(close) = available().find(|(_, p)| is_within(source, p, tolerance)) {
        return Some(pick(close, false));
    }
    if tolerance == 0 {
        return None;
    }
    available()
        .min_by_key(|(_, p)| distance(source, p))
        .map(|closest| pick(closest, true))
}

/// Like `pick_luma`, for the mode Cb and Cr share.
fn pick_chroma(
    source: &[[u8; CHROMA_SAMPLES]; 2],
    predictions: &[Option<[[u8; CHROMA_SAMPLES]; 2]>; 4],
    tolerance: i32,
) -> Option<ChromaPick> {
    let available = || {
        CHROMA_MODES
            .iter()
            .zip(predictions)
            .filter_map(|(mode, prediction)| Some((*mode, prediction.as_ref()?)))
    };
    let pick = |(mode, prediction): (u32, &[[u8; CHROMA_SAMPLES]; 2]), needs_residual| ChromaPick {
        mode,
        prediction: *prediction,
        needs_residual,
    };
    let is_close = |p: &[[u8; CHROMA_SAMPLES]; 2]| {
        is_within(&source[0], &p[0], tolerance) && is_within(&source[1], &p[1], tolerance)
    };
    if let Some(close) = available().find(|(_, p)| is_close(p)) {
        return Some(pick(close, false));
    }
    if tolerance == 0 {
        return None;
    }
    available()
        .min_by_key(|(_, p)| distance(&source[0], &p[0]) + distance(&source[1], &p[1]))
        .map(|closest| pick(closest, true))
}

/// The decoded samples around a block that intra prediction reads.
struct Edges {
    size: usize,
    /// The row above, when the block isn't on the top edge.
    top: Option<[i32; MB]>,
    /// The column to the left, when the block isn't on the left edge.
    left: Option<[i32; MB]>,
    /// The sample above-left, when both exist.
    corner: i32,
}

impl Edges {
    fn of(samples: &[u8], stride: usize, (x0, y0): (usize, usize), size: usize) -> Self {
        let sample = |x: usize, y: usize| i32::from(samples[y * stride + x]);
        let within = |at: usize, read: &dyn Fn(usize) -> i32| if at < size { read(at) } else { 0 };
        let top = (y0 > 0).then(|| std::array::from_fn(|x| within(x, &|x| sample(x0 + x, y0 - 1))));
        let left =
            (x0 > 0).then(|| std::array::from_fn(|y| within(y, &|y| sample(x0 - 1, y0 + y))));
        let corner = if x0 > 0 && y0 > 0 {
            sample(x0 - 1, y0 - 1)
        } else {
            0
        };
        Self {
            size,
            top,
            left,
            corner,
        }
    }

    /// A block predicted sample by sample, from (column, row).
    fn block<const N: usize>(&self, predict: impl Fn(usize, usize) -> i32) -> [u8; N] {
        std::array::from_fn(|at| predict(at % self.size, at / self.size) as u8)
    }

    fn vertical<const N: usize>(&self) -> Option<[u8; N]> {
        let top = self.top?;
        Some(self.block(|x, _| top[x]))
    }

    fn horizontal<const N: usize>(&self) -> Option<[u8; N]> {
        let left = self.left?;
        Some(self.block(|_, y| left[y]))
    }

    /// Plane prediction (8.3.3.4 for luma, 8.3.4.4 for chroma): a ramp
    /// fitted to both edges. `slope_factor` is 5 for 16x16, 34 for 8x8.
    fn plane<const N: usize>(&self, slope_factor: i32) -> Option<[u8; N]> {
        let (top, left) = (self.top?, self.left?);
        let half = self.size / 2;
        // p[-1] of either edge is the corner sample.
        let before = |edge: &[i32; MB], at: usize| {
            if at == 0 {
                self.corner
            } else {
                edge[at - 1]
            }
        };
        let gradient = |edge: &[i32; MB]| {
            (0..half)
                .map(|i| (i as i32 + 1) * (edge[half + i] - before(edge, half - 1 - i)))
                .sum::<i32>()
        };
        let a = 16 * (left[self.size - 1] + top[self.size - 1]);
        let b = (slope_factor * gradient(&top) + 32) >> 6;
        let c = (slope_factor * gradient(&left) + 32) >> 6;
        let centre = half as i32 - 1;
        Some(self.block(|x, y| {
            ((a + b * (x as i32 - centre) + c * (y as i32 - centre) + 16) >> 5).clamp(0, 255)
        }))
    }
}

/// Intra_16x16 predictions (8.3.3) in `LUMA_MODES` order; `None` where
/// the edge a mode reads is outside the picture.
fn luma_predictions(shown: &YuvFrame, position: MbPosition) -> [Option<[u8; LUMA_SAMPLES]>; 4] {
    let at = (position.x * MB, position.y * MB);
    let edges = Edges::of(&shown.y, shown.width, at, MB);
    let sum = |edge: &[i32; MB]| edge.iter().sum::<i32>();
    let dc = match (&edges.top, &edges.left) {
        (Some(top), Some(left)) => (sum(top) + sum(left) + 16) >> 5,
        (Some(top), None) => (sum(top) + 8) >> 4,
        (None, Some(left)) => (sum(left) + 8) >> 4,
        (None, None) => 128,
    };
    [
        edges.vertical(),
        edges.horizontal(),
        Some(edges.block(|_, _| dc)),
        edges.plane(5),
    ]
}

/// Intra chroma predictions (8.3.4) of Cb and Cr in `CHROMA_MODES` order.
fn chroma_predictions(
    shown: &YuvFrame,
    position: MbPosition,
) -> [Option<[[u8; CHROMA_SAMPLES]; 2]>; 4] {
    let at = (position.x * CHROMA_MB, position.y * CHROMA_MB);
    let edges = [&shown.u, &shown.v].map(|plane| Edges::of(plane, shown.width / 2, at, CHROMA_MB));
    let both = |predict: &dyn Fn(&Edges) -> Option<[u8; CHROMA_SAMPLES]>| {
        Some([predict(&edges[0])?, predict(&edges[1])?])
    };
    [
        both(&|edges| Some(edges.block(|x, y| chroma_dc(edges, x / 4 * 4, y / 4 * 4)))),
        both(&|edges| edges.horizontal()),
        both(&|edges| edges.vertical()),
        both(&|edges| edges.plane(34)),
    ]
}

/// DC value of the 4x4 chroma sub-block at (x_offset, y_offset), spec
/// 8.3.4.1-3. The corner and diagonal sub-blocks average both edges;
/// the top-right one prefers the top edge, the bottom-left one the left.
fn chroma_dc(edges: &Edges, x_offset: usize, y_offset: usize) -> i32 {
    let sum = |edge: &[i32; MB], from: usize| edge[from..from + 4].iter().sum::<i32>();
    let top = edges.top.as_ref().map(|top| sum(top, x_offset));
    let left = edges.left.as_ref().map(|left| sum(left, y_offset));
    let is_corner_or_diagonal = (x_offset == 0) == (y_offset == 0);
    if let (true, Some(top), Some(left)) = (is_corner_or_diagonal, top, left) {
        return (top + left + 4) >> 3;
    }
    let prefers_top = x_offset > 0 && y_offset == 0;
    let one_edge = if prefers_top {
        top.or(left)
    } else {
        left.or(top)
    };
    one_edge.map_or(128, |sum| (sum + 2) >> 2)
}

#[cfg(test)]
mod tests {
    use super::super::raster::{Canvas, Rgb};
    use super::*;

    const BACKGROUND: Rgb = Rgb::new(0x0e, 0x0f, 0x12);
    const SHAPE_1080P: StreamShape = StreamShape {
        width: 1920,
        height: 1080,
        fps: 30,
    };

    /// Reads fields back out of one NAL unit: past its header byte, with
    /// the emulation-prevention bytes dropped.
    struct FieldReader {
        bits: Vec<bool>,
        at: usize,
    }

    impl FieldReader {
        fn new(nal: &[u8]) -> Self {
            let mut rbsp = Vec::new();
            let mut zeros = 0;
            for &byte in &nal[1..] {
                if zeros >= 2 && byte == 3 {
                    zeros = 0;
                    continue;
                }
                rbsp.push(byte);
                zeros = if byte == 0 { zeros + 1 } else { 0 };
            }
            let bits = rbsp
                .iter()
                .flat_map(|byte| (0..8).rev().map(move |shift| (byte >> shift) & 1 == 1))
                .collect();
            Self { bits, at: 0 }
        }

        /// u(n)
        fn bits(&mut self, count: usize) -> u32 {
            let field = &self.bits[self.at..self.at + count];
            self.at += count;
            field
                .iter()
                .fold(0, |value, bit| (value << 1) | u32::from(*bit))
        }

        /// ue(v)
        fn ue(&mut self) -> u32 {
            let zeros = self.bits[self.at..].iter().take_while(|bit| !**bit).count();
            self.at += zeros;
            self.bits(zeros + 1) - 1
        }
    }

    /// The SPS's id and how many bits its frame_num takes in a slice.
    fn sps_id_and_frame_num_bits(sps: &[u8]) -> (u32, usize) {
        let mut reader = FieldReader::new(sps);
        reader.bits(24); // profile_idc, constraint flags, level_idc
        let id = reader.ue();
        (id, reader.ue() as usize + 4)
    }

    /// A slice header read up to frame_num: (reader, slice_type % 5,
    /// pic_parameter_set_id, frame_num).
    fn read_slice_header(slice: &[u8], frame_num_bits: usize) -> (FieldReader, u32, u32, u32) {
        let mut header = FieldReader::new(slice);
        assert_eq!(header.ue(), 0, "one slice per picture");
        let slice_type = header.ue() % 5;
        let pps_id = header.ue();
        let frame_num = header.bits(frame_num_bits);
        (header, slice_type, pps_id, frame_num)
    }

    fn flat_frame(width: usize, height: usize) -> YuvFrame {
        Canvas::new(width, height, BACKGROUND).to_yuv420()
    }

    /// OBS's streams use parameter-set id 0. The screen's SPS and PPS must
    /// sit under another id, and its slices must name that id: otherwise
    /// switching to the screen overwrites the stream's own SPS in every
    /// decoder, and switching back needs a sequence header they may ignore.
    #[test]
    fn the_screen_never_reuses_the_parameter_set_ids_of_the_stream() {
        let sets = parameter_sets(SHAPE_1080P);
        let (sps_id, frame_num_bits) = sps_id_and_frame_num_bits(&sets.sps);
        let mut pps = FieldReader::new(&sets.pps);
        let pps_id = pps.ue();
        assert_ne!(sps_id, 0, "SPS id 0 belongs to the stream");
        assert_ne!(pps_id, 0, "PPS id 0 belongs to the stream");
        assert_eq!(pps.ue(), sps_id, "the PPS points at the screen's SPS");

        let frame = flat_frame(64, 64);
        for slice in [encode_idr(&frame, 0), encode_p(&frame, &frame, 1)] {
            let (_, _, slice_pps_id, _) = read_slice_header(&slice, frame_num_bits);
            assert_eq!(slice_pps_id, pps_id, "slices point at the screen's PPS");
        }
    }

    /// What replaying the loop relies on: each IDR carries the idr_pic_id
    /// it was asked for (back-to-back IDRs must differ), and P pictures are
    /// kept as references, since the next one skips against them, with
    /// frame_num counting up and wrapping at the size the SPS declares.
    /// A 240 fps loop has 480 pictures, so the wrap is a real path.
    #[test]
    fn slice_headers_number_the_loop_the_way_the_sps_says() {
        let (_, frame_num_bits) = sps_id_and_frame_num_bits(&parameter_sets(SHAPE_1080P).sps);
        let frame = flat_frame(64, 64);
        for idr_pic_id in [0, 1] {
            let idr = encode_idr(&frame, idr_pic_id);
            assert_eq!(idr[0] & 0x1F, bitstream::NAL_IDR_SLICE);
            assert_ne!(idr[0] >> 5, 0, "an IDR must be a reference");
            let (mut header, slice_type, _, frame_num) = read_slice_header(&idr, frame_num_bits);
            assert_eq!((slice_type, frame_num), (2, 0), "I slice, frame_num 0");
            assert_eq!(header.ue(), idr_pic_id);
        }
        let max_frame_num = 1 << frame_num_bits;
        for index in [1, max_frame_num - 1, max_frame_num, max_frame_num + 223] {
            let p = encode_p(&frame, &frame, index);
            assert_eq!(p[0] & 0x1F, bitstream::NAL_SLICE);
            assert_ne!(p[0] >> 5, 0, "picture {index} must be a reference");
            let (_, slice_type, _, frame_num) = read_slice_header(&p, frame_num_bits);
            assert_eq!(slice_type, 0, "picture {index} is a P slice");
            assert_eq!(frame_num, index % max_frame_num, "picture {index}");
        }
    }

    /// level_idc must be the smallest level of Table A-1 whose frame size
    /// and macroblock rate both fit: too low and strict decoders refuse
    /// the screen. The table starts at 3.0, so anything smaller reports
    /// 3.0, which every decoder of a real stream handles.
    #[test]
    fn level_is_the_smallest_in_table_a1_that_fits() {
        for (width, height, fps, level) in [
            (320, 180, 30, 30),
            (854, 480, 30, 31),
            (1280, 720, 30, 31),
            (1280, 720, 60, 32),
            (1920, 1080, 30, 40),
            (1080, 1920, 30, 40),
            (1920, 1080, 60, 42),
            (2560, 1440, 30, 50),
            (2560, 1440, 60, 51),
            (3840, 2160, 30, 51),
            (3840, 2160, 60, 52),
            (7680, 4320, 30, 60),
            (7680, 4320, 60, 61),
            (7680, 4320, 120, 62),
        ] {
            let sps = parameter_sets(StreamShape { width, height, fps }).sps;
            assert_eq!(sps[3], level, "{width}x{height} at {fps} fps");
        }
    }

    /// A picture identical to the previous one is a single skip run, a few
    /// bytes at any resolution: that is what keeps the held screen cheap.
    /// A block that did change must still be coded, and only that block,
    /// or viewers keep a stale picture.
    #[test]
    fn a_p_picture_codes_only_the_blocks_that_changed() {
        let still = flat_frame(1920, 1080);
        let unchanged = encode_p(&still, &still, 1);
        assert!(
            unchanged.len() < 16,
            "{} bytes for an unchanged 1080p picture",
            unchanged.len()
        );

        // Noise in one macroblock: no prediction reproduces it, so it goes
        // out as I_PCM, 384 raw samples.
        let mut noisy = Canvas::new(1920, 1080, BACKGROUND);
        for i in 0..256 {
            let color = Rgb::new((i * 37) as u8, (i * 91) as u8, (i * 53) as u8);
            noisy.fill_rect(800 + i % 16, 400 + i / 16, 1, 1, color);
        }
        let changed = encode_p(&noisy.to_yuv420(), &still, 1);
        assert!(
            (384..384 + 32).contains(&changed.len()),
            "{} bytes for one changed block",
            changed.len()
        );
    }
}
