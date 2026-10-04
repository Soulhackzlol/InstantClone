//! Lossless H.264 encoder for flat-colour screens.
//!
//! Only needs three macroblock types, which is what keeps it small:
//! - I_16x16 with no residual: the block is an exact copy of a vertical,
//!   horizontal or DC prediction from its neighbours (flat areas).
//! - I_PCM: the 384 raw samples, for anything with detail.
//! - P_Skip: unchanged since the previous frame.
//!
//! Every macroblock reconstructs to exactly its source samples, so later
//! predictions can read the source frame as if it were the decoder's
//! output. Deblocking is switched off in every slice so nothing touches
//! the reconstruction afterwards.
//!
//! Stream shape: Constrained Baseline, CAVLC, one slice per picture, a
//! single reference frame, POC type 2 (output order = decode order).

use super::bitstream::{self, BitWriter};
use super::raster::YuvFrame;

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

/// IDR picture. Consecutive IDRs must differ in `idr_pic_id`, so a loop
/// that replays its keyframe alternates between two ids.
pub fn encode_idr(frame: &YuvFrame, idr_pic_id: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(0); // first_mb_in_slice
    w.ue(SLICE_TYPE_I);
    w.ue(PPS_ID);
    w.bits(0, LOG2_MAX_FRAME_NUM); // frame_num
    w.ue(idr_pic_id);
    w.bit(false); // no_output_of_prior_pics_flag
    w.bit(false); // long_term_reference_flag
    write_slice_tail(&mut w);
    let mut coder = MacroblockCoder::new(frame);
    for index in 0..coder.mb_count() {
        let coding = choose_intra(frame, coder.position(index));
        coder.write(&mut w, index, coding, 0);
    }
    bitstream::nal_unit(3, bitstream::NAL_IDR_SLICE, &w.finish())
}

/// P picture: blocks identical to `previous` are skipped, the rest are
/// intra coded. Every P picture is a reference so the next one can skip
/// against it; `frame_num` counts up from 1 after the IDR.
pub fn encode_p(frame: &YuvFrame, previous: &YuvFrame, frame_num: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(0); // first_mb_in_slice
    w.ue(SLICE_TYPE_P);
    w.ue(PPS_ID);
    w.bits(frame_num % (1 << LOG2_MAX_FRAME_NUM), LOG2_MAX_FRAME_NUM);
    w.bit(false); // num_ref_idx_active_override_flag
    w.bit(false); // ref_pic_list_modification_flag_l0
    w.bit(false); // adaptive_ref_pic_marking_mode_flag
    write_slice_tail(&mut w);

    let mut coder = MacroblockCoder::new(frame);
    let mut skip_run = 0;
    for index in 0..coder.mb_count() {
        let position = coder.position(index);
        if is_unchanged(frame, previous, position) {
            coder.mark_skipped(index);
            skip_run += 1;
            continue;
        }
        w.ue(skip_run);
        skip_run = 0;
        coder.write(
            &mut w,
            index,
            choose_intra(frame, position),
            P_SLICE_INTRA_OFFSET,
        );
    }
    if skip_run > 0 {
        w.ue(skip_run);
    }
    bitstream::nal_unit(2, bitstream::NAL_SLICE, &w.finish())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LumaMode {
    Vertical = 0,
    Horizontal = 1,
    Dc = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChromaMode {
    Dc = 0,
    Horizontal = 1,
    Vertical = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MbCoding {
    Predicted { luma: LumaMode, chroma: ChromaMode },
    Pcm,
}

/// Writes macroblocks and remembers which were I_PCM, because the
/// CAVLC coeff_token table for the next block depends on it.
struct MacroblockCoder<'a> {
    frame: &'a YuvFrame,
    width_mbs: usize,
    is_pcm: Vec<bool>,
}

impl<'a> MacroblockCoder<'a> {
    fn new(frame: &'a YuvFrame) -> Self {
        let width_mbs = frame.width / MB;
        let height_mbs = frame.height / MB;
        Self {
            frame,
            width_mbs,
            is_pcm: vec![false; width_mbs * height_mbs],
        }
    }

    fn mb_count(&self) -> usize {
        self.is_pcm.len()
    }

    fn position(&self, index: usize) -> MbPosition {
        MbPosition {
            x: index % self.width_mbs,
            y: index / self.width_mbs,
        }
    }

    fn mark_skipped(&mut self, index: usize) {
        self.is_pcm[index] = false;
    }

    fn write(&mut self, w: &mut BitWriter, index: usize, coding: MbCoding, type_offset: u32) {
        match coding {
            MbCoding::Pcm => {
                w.ue(type_offset + MB_TYPE_I_PCM);
                w.align_with_zeros();
                write_pcm_samples(w, self.frame, self.position(index));
                self.is_pcm[index] = true;
            }
            MbCoding::Predicted { luma, chroma } => {
                // I_16x16_<mode>_0_0: both coded block patterns zero.
                w.ue(type_offset + 1 + luma as u32);
                w.ue(chroma as u32);
                w.se(0); // mb_qp_delta
                self.write_empty_dc_block(w, index);
                self.is_pcm[index] = false;
            }
        }
    }

    /// Intra16x16DCLevel with no coefficients. Its coeff_token codeword
    /// depends on nC, the coefficient counts of the left and top blocks.
    fn write_empty_dc_block(&self, w: &mut BitWriter, index: usize) {
        let position = self.position(index);
        let left = (position.x > 0).then(|| self.neighbour_total_coeff(index - 1));
        let top = (position.y > 0).then(|| self.neighbour_total_coeff(index - self.width_mbs));
        let n_c = match (left, top) {
            (Some(a), Some(b)) => (a + b + 1) >> 1,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => 0,
        };
        // TotalCoeff = 0, TrailingOnes = 0 in Table 9-5.
        match n_c {
            0..=1 => w.bits(0b1, 1),
            2..=3 => w.bits(0b11, 2),
            4..=7 => w.bits(0b1111, 4),
            _ => w.bits(0b000011, 6),
        }
    }

    /// Skipped and predicted blocks carry no coefficients; I_PCM counts
    /// as 16.
    fn neighbour_total_coeff(&self, index: usize) -> u32 {
        if self.is_pcm[index] {
            PCM_TOTAL_COEFF
        } else {
            0
        }
    }
}

fn write_pcm_samples(w: &mut BitWriter, frame: &YuvFrame, position: MbPosition) {
    for row in 0..MB {
        let start = (position.y * MB + row) * frame.width + position.x * MB;
        w.bytes(&frame.y[start..start + MB]);
    }
    let chroma_width = frame.width / 2;
    for plane in [&frame.u, &frame.v] {
        for row in 0..CHROMA_MB {
            let start = (position.y * CHROMA_MB + row) * chroma_width + position.x * CHROMA_MB;
            w.bytes(&plane[start..start + CHROMA_MB]);
        }
    }
}

fn is_unchanged(frame: &YuvFrame, previous: &YuvFrame, position: MbPosition) -> bool {
    let luma_same = (0..MB).all(|row| {
        let start = (position.y * MB + row) * frame.width + position.x * MB;
        frame.y[start..start + MB] == previous.y[start..start + MB]
    });
    let chroma_width = frame.width / 2;
    luma_same
        && (0..CHROMA_MB).all(|row| {
            let start = (position.y * CHROMA_MB + row) * chroma_width + position.x * CHROMA_MB;
            frame.u[start..start + CHROMA_MB] == previous.u[start..start + CHROMA_MB]
                && frame.v[start..start + CHROMA_MB] == previous.v[start..start + CHROMA_MB]
        })
}

/// First luma + chroma prediction pair that reproduces the block
/// exactly, or I_PCM when none does.
fn choose_intra(frame: &YuvFrame, position: MbPosition) -> MbCoding {
    let luma = [LumaMode::Vertical, LumaMode::Horizontal, LumaMode::Dc]
        .into_iter()
        .find(|mode| predicts_luma(frame, position, *mode));
    let chroma = [ChromaMode::Dc, ChromaMode::Horizontal, ChromaMode::Vertical]
        .into_iter()
        .find(|mode| predicts_chroma(frame, position, *mode));
    match (luma, chroma) {
        (Some(luma), Some(chroma)) => MbCoding::Predicted { luma, chroma },
        _ => MbCoding::Pcm,
    }
}

/// One plane of a frame plus the block geometry, so the luma and both
/// chroma predictors share the same neighbour reads.
struct PlaneBlock<'a> {
    samples: &'a [u8],
    stride: usize,
    x0: usize,
    y0: usize,
    size: usize,
}

impl PlaneBlock<'_> {
    fn at(&self, x: usize, y: usize) -> u8 {
        self.samples[(self.y0 + y) * self.stride + self.x0 + x]
    }

    fn has_top(&self) -> bool {
        self.y0 > 0
    }

    fn has_left(&self) -> bool {
        self.x0 > 0
    }

    fn top(&self, x: usize) -> u32 {
        self.samples[(self.y0 - 1) * self.stride + self.x0 + x] as u32
    }

    fn left(&self, y: usize) -> u32 {
        self.samples[(self.y0 + y) * self.stride + self.x0 - 1] as u32
    }

    /// Whether `predict(x, y)` equals every sample of the block.
    fn matches(&self, predict: impl Fn(usize, usize) -> u32) -> bool {
        (0..self.size).all(|y| (0..self.size).all(|x| self.at(x, y) as u32 == predict(x, y)))
    }
}

fn luma_block(frame: &YuvFrame, position: MbPosition) -> PlaneBlock<'_> {
    PlaneBlock {
        samples: &frame.y,
        stride: frame.width,
        x0: position.x * MB,
        y0: position.y * MB,
        size: MB,
    }
}

fn chroma_blocks(frame: &YuvFrame, position: MbPosition) -> [PlaneBlock<'_>; 2] {
    let block = |samples| PlaneBlock {
        samples,
        stride: frame.width / 2,
        x0: position.x * CHROMA_MB,
        y0: position.y * CHROMA_MB,
        size: CHROMA_MB,
    };
    [block(&frame.u), block(&frame.v)]
}

/// Intra_16x16 prediction (spec 8.3.3).
fn predicts_luma(frame: &YuvFrame, position: MbPosition, mode: LumaMode) -> bool {
    let block = luma_block(frame, position);
    match mode {
        LumaMode::Vertical => block.has_top() && block.matches(|x, _| block.top(x)),
        LumaMode::Horizontal => block.has_left() && block.matches(|_, y| block.left(y)),
        LumaMode::Dc => {
            let top: u32 = (0..MB)
                .map(|x| if block.has_top() { block.top(x) } else { 0 })
                .sum();
            let left: u32 = (0..MB)
                .map(|y| if block.has_left() { block.left(y) } else { 0 })
                .sum();
            let dc = match (block.has_top(), block.has_left()) {
                (true, true) => (top + left + 16) >> 5,
                (true, false) => (top + 8) >> 4,
                (false, true) => (left + 8) >> 4,
                (false, false) => 128,
            };
            block.matches(|_, _| dc)
        }
    }
}

/// Intra chroma prediction (spec 8.3.4) for both chroma planes.
fn predicts_chroma(frame: &YuvFrame, position: MbPosition, mode: ChromaMode) -> bool {
    chroma_blocks(frame, position)
        .iter()
        .all(|block| match mode {
            ChromaMode::Vertical => block.has_top() && block.matches(|x, _| block.top(x)),
            ChromaMode::Horizontal => block.has_left() && block.matches(|_, y| block.left(y)),
            ChromaMode::Dc => block.matches(|x, y| chroma_dc(block, x / 4 * 4, y / 4 * 4)),
        })
}

/// DC value of the 4x4 chroma sub-block at (x_offset, y_offset), spec
/// 8.3.4.1-3. The corner and diagonal sub-blocks average both edges;
/// the top-right one prefers the top edge, the bottom-left one the left.
fn chroma_dc(block: &PlaneBlock, x_offset: usize, y_offset: usize) -> u32 {
    let top_sum = || (0..4).map(|i| block.top(x_offset + i)).sum::<u32>();
    let left_sum = || (0..4).map(|i| block.left(y_offset + i)).sum::<u32>();
    let (has_top, has_left) = (block.has_top(), block.has_left());
    let is_corner_or_diagonal = (x_offset == 0) == (y_offset == 0);
    if is_corner_or_diagonal && has_top && has_left {
        return (top_sum() + left_sum() + 4) >> 3;
    }
    let prefers_top = x_offset > 0 && y_offset == 0;
    match (prefers_top, has_top, has_left) {
        (true, true, _) => (top_sum() + 2) >> 2,
        (false, _, true) => (left_sum() + 2) >> 2,
        (_, true, _) => (top_sum() + 2) >> 2,
        (_, _, true) => (left_sum() + 2) >> 2,
        _ => 128,
    }
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
