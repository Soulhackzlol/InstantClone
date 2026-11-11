//! HEVC encoder for the screens InstantClone draws itself: the H.265
//! counterpart of `encoder`, for tracks the stream sends as HEVC (Twitch
//! 2K channels).
//!
//! Each 32x32 coding tree unit is one coding unit when it can be, else
//! four 16x16 ones, coded as one of:
//! - intra planar, DC, vertical or horizontal with no residual, when that
//!   prediction is close enough (flat areas, even ramps);
//! - the same with transform residual, for soft detail (gradients,
//!   glows). Only screens that allow a tolerance use it;
//! - PCM, 16x16 only: the raw samples, for anything else;
//! - skip: close enough to the previous picture already (zero motion);
//! - carried over with residual: the previous picture's block (zero
//!   motion, merged) plus a correction, for detail that changes a little,
//!   like a dot getting brighter. Only with a tolerance, like residual.
//!
//! When more than one fits, the one estimated to take fewest bits wins.
//!
//! A screen coded with tolerance 0 is lossless. With a tolerance, every
//! sample lands within that many levels of its source, and the encoder
//! keeps the picture a decoder really shows, so predictions and skips are
//! judged against that. Deblocking and SAO are off so nothing touches the
//! reconstruction afterwards.
//!
//! Stream shape: Main profile, 8-bit 4:2:0, CABAC (HEVC has no other
//! entropy coder), one slice per picture, each P picture referencing the
//! one before it, no reordering, one QP per CTU.

use super::bitstream::{self, BitWriter};
use super::cabac::{CabacWriter, Context};
use super::encoder::{Coded, StreamShape};
use super::hevc_residual::{self, ResidualContexts};
use transform_tree::{Coefficients, Residual};

mod transform_tree;
use super::raster::YuvFrame;

/// Coding tree block side. The coded frame is whole CTBs (the SPS crops
/// the padding), so no coding unit ever crosses the picture edge.
pub const CTB: usize = 32;
/// Side of the coding units a CTB splits into, and of PCM blocks.
const SPLIT_CU: usize = 16;
const LOG2_MAX_POC_LSB: u8 = 8;
/// Parameter-set ids for the reconnect screen. OBS's HEVC uses 0 for its
/// VPS, SPS and PPS; see `encoder::SPS_ID` for why the screen's own must
/// sit beside them instead of replacing them.
const VPS_ID: u32 = 1;
const SPS_ID: u32 = 1;
const PPS_ID: u32 = 1;
const PROFILE_MAIN: u32 = 1;
/// general_profile_compatibility_flag[1] and [2]: Main, and so Main 10.
const PROFILE_COMPATIBILITY: u32 = 0x6000_0000;
const SLICE_QP: i32 = 26;
const SLICE_TYPE_P: u32 = 1;
const SLICE_TYPE_I: u32 = 2;
/// One merge candidate, so no merge_idx: every inter block (skipped or
/// carried over) is merged at zero motion, so the candidate always is
/// zero motion too.
const FIVE_MINUS_MAX_MERGE_CANDIDATES: u32 = 4;

/// Intra prediction mode numbers (8.4.2) the encoder chooses from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntraMode {
    Planar = 0,
    Dc = 1,
    Horizontal = 10,
    Vertical = 26,
}

const MODE_PLANAR: u8 = IntraMode::Planar as u8;
const MODE_DC: u8 = IntraMode::Dc as u8;
const MODE_VERTICAL: u8 = IntraMode::Vertical as u8;
/// The order modes are tried in: the cheap exact ones of the lossless
/// screens first.
const INTRA_MODES: [IntraMode; 4] = [
    IntraMode::Dc,
    IntraMode::Vertical,
    IntraMode::Horizontal,
    IntraMode::Planar,
];
/// The modes by `IntraMode::index`.
const INTRA_MODES_BY_INDEX: [IntraMode; 4] = [
    IntraMode::Planar,
    IntraMode::Vertical,
    IntraMode::Horizontal,
    IntraMode::Dc,
];

impl IntraMode {
    /// intra_chroma_pred_mode 0..3 naming this mode (Table 8-2).
    fn chroma_index(self) -> u32 {
        match self {
            IntraMode::Planar => 0,
            IntraMode::Vertical => 1,
            IntraMode::Horizontal => 2,
            IntraMode::Dc => 3,
        }
    }

    /// Where this mode sits in `INTRA_MODES_BY_INDEX`.
    fn index(self) -> usize {
        self.chroma_index() as usize
    }
}

/// VPS, SPS and PPS NAL units (headers included, no length prefix).
pub fn parameter_sets(shape: StreamShape) -> [Vec<u8>; 3] {
    let level = level_idc(shape);
    [
        bitstream::hevc_nal_unit(bitstream::HEVC_NAL_VPS, &vps_rbsp(level)),
        bitstream::hevc_nal_unit(bitstream::HEVC_NAL_SPS, &sps_rbsp(shape, level)),
        bitstream::hevc_nal_unit(bitstream::HEVC_NAL_PPS, &pps_rbsp()),
    ]
}

fn write_profile_tier_level(w: &mut BitWriter, level: u32) {
    w.bits(0, 2); // general_profile_space
    w.bit(false); // general_tier_flag: Main tier
    w.bits(PROFILE_MAIN, 5);
    w.bits(PROFILE_COMPATIBILITY, 32);
    w.bit(true); // general_progressive_source_flag
    w.bit(false); // general_interlaced_source_flag
    w.bit(false); // general_non_packed_constraint_flag
    w.bit(true); // general_frame_only_constraint_flag
    w.bits(0, 32); // general_reserved_zero_43bits, first 32
    w.bits(0, 11); // and the other 11
    w.bit(false); // general_inbld_flag
    w.bits(level, 8);
}

/// max_dec_pic_buffering_minus1 (the picture being decoded plus its one
/// reference), max_num_reorder_pics, max_latency_increase_plus1.
fn write_sub_layer_ordering(w: &mut BitWriter) {
    w.bit(true); // sub_layer_ordering_info_present_flag
    w.ue(1);
    w.ue(0);
    w.ue(0);
}

fn vps_rbsp(level: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.bits(VPS_ID, 4);
    w.bit(true); // vps_base_layer_internal_flag
    w.bit(true); // vps_base_layer_available_flag
    w.bits(0, 6); // vps_max_layers_minus1
    w.bits(0, 3); // vps_max_sub_layers_minus1
    w.bit(true); // vps_temporal_id_nesting_flag
    w.bits(0xFFFF, 16); // vps_reserved_0xffff_16bits
    write_profile_tier_level(&mut w, level);
    write_sub_layer_ordering(&mut w);
    w.bits(0, 6); // vps_max_layer_id
    w.ue(0); // vps_num_layer_sets_minus1
    w.bit(false); // vps_timing_info_present_flag: the SPS VUI has it
    w.bit(false); // vps_extension_flag
    w.finish()
}

fn sps_rbsp(shape: StreamShape, level: u32) -> Vec<u8> {
    let (coded_width, coded_height) = coded_size(shape);
    let mut w = BitWriter::new();
    w.bits(VPS_ID, 4);
    w.bits(0, 3); // sps_max_sub_layers_minus1
    w.bit(true); // sps_temporal_id_nesting_flag
    write_profile_tier_level(&mut w, level);
    w.ue(SPS_ID);
    w.ue(1); // chroma_format_idc: 4:2:0
    w.ue(coded_width as u32);
    w.ue(coded_height as u32);
    let crop_right = (coded_width - shape.width) / 2;
    let crop_bottom = (coded_height - shape.height) / 2;
    let needs_crop = crop_right > 0 || crop_bottom > 0;
    w.bit(needs_crop); // conformance_window_flag
    if needs_crop {
        // Offsets are in chroma samples: 2 luma samples in 4:2:0.
        w.ue(0);
        w.ue(crop_right as u32);
        w.ue(0);
        w.ue(crop_bottom as u32);
    }
    w.ue(0); // bit_depth_luma_minus8
    w.ue(0); // bit_depth_chroma_minus8
    w.ue(LOG2_MAX_POC_LSB as u32 - 4);
    write_sub_layer_ordering(&mut w);
    w.ue(0); // log2_min_luma_coding_block_size_minus3: 8
    w.ue(2); // log2_diff_max_min_luma_coding_block_size: CTB 32
    w.ue(0); // log2_min_luma_transform_block_size_minus2: 4
    w.ue(3); // log2_diff_max_min_luma_transform_block_size: 32
    w.ue(transform_tree::MAX_INTER_DEPTH as u32); // max_transform_hierarchy_depth_inter
    w.ue(0); // max_transform_hierarchy_depth_intra
    w.bit(false); // scaling_list_enabled_flag
    w.bit(false); // amp_enabled_flag
    w.bit(false); // sample_adaptive_offset_enabled_flag
    w.bit(true); // pcm_enabled_flag
    w.bits(7, 4); // pcm_sample_bit_depth_luma_minus1
    w.bits(7, 4); // pcm_sample_bit_depth_chroma_minus1
    w.ue(1); // log2_min_pcm_luma_coding_block_size_minus3: 16
    w.ue(0); // log2_diff_max_min_pcm_luma_coding_block_size: 16 only
    w.bit(true); // pcm_loop_filter_disabled_flag
                 // One reference picture set: the picture just before.
    w.ue(1); // num_short_term_ref_pic_sets
    w.ue(1); // num_negative_pics
    w.ue(0); // num_positive_pics
    w.ue(0); // delta_poc_s0_minus1
    w.bit(true); // used_by_curr_pic_s0_flag
    w.bit(false); // long_term_ref_pics_present_flag
    w.bit(false); // sps_temporal_mvp_enabled_flag
    w.bit(false); // strong_intra_smoothing_enabled_flag
    w.bit(true); // vui_parameters_present_flag
    write_vui(&mut w, shape.fps);
    w.bit(false); // sps_extension_present_flag
    w.finish()
}

/// BT.709 limited range (matches `raster`) and a fixed frame rate, like
/// the H.264 screen's VUI.
fn write_vui(w: &mut BitWriter, fps: u32) {
    w.bit(false); // aspect_ratio_info_present_flag
    w.bit(false); // overscan_info_present_flag
    w.bit(true); // video_signal_type_present_flag
    w.bits(5, 3); // video_format: unspecified
    w.bit(false); // video_full_range_flag
    w.bit(true); // colour_description_present_flag
    w.bits(1, 8); // colour_primaries: BT.709
    w.bits(1, 8); // transfer_characteristics: BT.709
    w.bits(1, 8); // matrix_coeffs: BT.709
    w.bit(false); // chroma_loc_info_present_flag
    w.bit(false); // neutral_chroma_indication_flag
    w.bit(false); // field_seq_flag
    w.bit(false); // frame_field_info_present_flag
    w.bit(false); // default_display_window_flag
    w.bit(true); // vui_timing_info_present_flag
    w.bits(1, 32); // vui_num_units_in_tick
    w.bits(fps, 32); // vui_time_scale: one tick per frame
    w.bit(false); // vui_poc_proportional_to_timing_flag
    w.bit(false); // vui_hrd_parameters_present_flag
    w.bit(true); // bitstream_restriction_flag
    w.bit(false); // tiles_fixed_structure_flag
    w.bit(true); // motion_vectors_over_pic_boundaries_flag
    w.bit(true); // restricted_ref_pic_lists_flag
    w.ue(0); // min_spatial_segmentation_idc
    w.ue(0); // max_bytes_per_pic_denom: no limit
    w.ue(0); // max_bits_per_min_cu_denom: no limit
    w.ue(15); // log2_max_mv_length_horizontal
    w.ue(15); // log2_max_mv_length_vertical
}

fn pps_rbsp() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(PPS_ID);
    w.ue(SPS_ID);
    w.bit(false); // dependent_slice_segments_enabled_flag
    w.bit(false); // output_flag_present_flag
    w.bits(0, 3); // num_extra_slice_header_bits
    w.bit(false); // sign_data_hiding_enabled_flag
    w.bit(false); // cabac_init_present_flag
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.se(0); // init_qp_minus26
    w.bit(false); // constrained_intra_pred_flag
    w.bit(false); // transform_skip_enabled_flag
    w.bit(true); // cu_qp_delta_enabled_flag
    w.ue(0); // diff_cu_qp_delta_depth: one QP per CTU
    w.se(0); // pps_cb_qp_offset
    w.se(0); // pps_cr_qp_offset
    w.bit(false); // pps_slice_chroma_qp_offsets_present_flag
    w.bit(false); // weighted_pred_flag
    w.bit(false); // weighted_bipred_flag
    w.bit(false); // transquant_bypass_enabled_flag
    w.bit(false); // tiles_enabled_flag
    w.bit(false); // entropy_coding_sync_enabled_flag
    w.bit(false); // pps_loop_filter_across_slices_enabled_flag
    w.bit(true); // deblocking_filter_control_present_flag
    w.bit(false); // deblocking_filter_override_enabled_flag
    w.bit(true); // pps_deblocking_filter_disabled_flag
    w.bit(false); // pps_scaling_list_data_present_flag
    w.bit(false); // lists_modification_present_flag
    w.ue(0); // log2_parallel_merge_level_minus2
    w.bit(false); // slice_segment_header_extension_present_flag
    w.bit(false); // pps_extension_present_flag
    w.finish()
}

/// The visible size rounded up to whole CTBs.
pub fn coded_size(shape: StreamShape) -> (usize, usize) {
    (
        shape.width.div_ceil(CTB) * CTB,
        shape.height.div_ceil(CTB) * CTB,
    )
}

/// Smallest level (Table A.8) whose picture size, sample rate and
/// longest side fit; 6.2 when none does.
fn level_idc(shape: StreamShape) -> u32 {
    const LEVELS: [(u32, u64, u64); 13] = [
        (30, 36_864, 552_960),
        (60, 122_880, 3_686_400),
        (63, 245_760, 7_372_800),
        (90, 552_960, 16_588_800),
        (93, 983_040, 33_177_600),
        (120, 2_228_224, 66_846_720),
        (123, 2_228_224, 133_693_440),
        (150, 8_912_896, 267_386_880),
        (153, 8_912_896, 534_773_760),
        (156, 8_912_896, 1_069_547_520),
        (180, 35_651_584, 1_069_547_520),
        (183, 35_651_584, 2_139_095_040),
        (186, 35_651_584, 4_278_190_080),
    ];
    let (width, height) = coded_size(shape);
    let picture = (width * height) as u64;
    let longest = width.max(height) as u64;
    let rate = picture * u64::from(shape.fps);
    LEVELS
        .iter()
        .find(|(_, max_picture, max_rate)| {
            picture <= *max_picture && rate <= *max_rate && longest * longest <= max_picture * 8
        })
        .map_or(186, |(level, _, _)| *level)
}

/// IDR picture, exact: every block intra or PCM. Consecutive HEVC IDRs
/// need no distinguishing id, so a replayed loop can reuse it as is.
#[cfg(test)]
pub fn encode_idr(frame: &YuvFrame) -> Vec<u8> {
    encode_idr_within(frame, 0).nal
}

/// IDR picture whose every sample lands within `tolerance` of `frame`.
pub fn encode_idr_within(frame: &YuvFrame, tolerance: u8) -> Coded {
    let mut w = BitWriter::new();
    w.bit(true); // first_slice_segment_in_pic_flag
    w.bit(false); // no_output_of_prior_pics_flag
    w.ue(PPS_ID);
    w.ue(SLICE_TYPE_I);
    write_slice_tail(&mut w);
    let (data, shown) = PictureCoder::new(frame, None, tolerance, w).code();
    Coded {
        nal: bitstream::hevc_nal_unit(bitstream::HEVC_NAL_IDR_N_LP, &data),
        shown,
    }
}

/// P picture `poc` pictures after the IDR, exact: blocks identical to
/// `previous` are skipped, the rest are intra coded. Every P picture is a
/// reference so the next one can skip against it.
#[cfg(test)]
pub fn encode_p(frame: &YuvFrame, previous: &YuvFrame, poc: u32) -> Vec<u8> {
    encode_p_within(frame, previous, poc, 0).nal
}

/// P picture whose every sample lands within `tolerance` of `frame`.
/// `shown_before` is what the decoder shows for the picture before it: a
/// block already that close to `frame` is skipped.
pub fn encode_p_within(
    frame: &YuvFrame,
    shown_before: &YuvFrame,
    poc: u32,
    tolerance: u8,
) -> Coded {
    let mut w = BitWriter::new();
    w.bit(true); // first_slice_segment_in_pic_flag
    w.ue(PPS_ID);
    w.ue(SLICE_TYPE_P);
    w.bits(poc % (1 << LOG2_MAX_POC_LSB), LOG2_MAX_POC_LSB);
    w.bit(true); // short_term_ref_pic_set_sps_flag: the SPS's only set
    w.bit(false); // num_ref_idx_active_override_flag
    w.ue(FIVE_MINUS_MAX_MERGE_CANDIDATES);
    write_slice_tail(&mut w);
    let (data, shown) = PictureCoder::new(frame, Some(shown_before), tolerance, w).code();
    Coded {
        nal: bitstream::hevc_nal_unit(bitstream::HEVC_NAL_TRAIL_R, &data),
        shown,
    }
}

/// slice_qp_delta, then byte_alignment() so slice data starts on a byte.
fn write_slice_tail(w: &mut BitWriter) {
    w.se(0); // slice_qp_delta
    w.bit(true); // alignment_bit_equal_to_one
    w.align_with_zeros();
}

/// A square block of luma samples; its chroma is half the side.
#[derive(Debug, Clone, Copy)]
struct Block {
    x: usize,
    y: usize,
    size: usize,
}

enum CuCoding {
    Skip,
    Predicted {
        luma: IntraMode,
        chroma: IntraMode,
        residual: Option<Residual>,
    },
    Pcm,
    /// The previous picture's block, merged at zero motion, plus residual.
    CarriedOver {
        residual: Residual,
    },
}

/// How one coding unit is coded, the samples a decoder shows for it, and
/// about how many bits it takes.
struct Decision {
    coding: CuCoding,
    shown: Samples,
    bits: usize,
}

/// Rough cost of a coding unit's syntax besides its residual: the flags
/// and modes. Only used to compare one coding with another.
const HEADER_BITS: usize = 6;
/// A carried-over block this cheap is taken without trying intra: intra
/// could save a few bits at most, for the cost of four predictions.
const GOOD_ENOUGH_BITS: usize = HEADER_BITS + 32;
/// Residual this cheap whole is not tried split as well.
const CHEAP_RESIDUAL_BITS: usize = 48;

/// A block's samples: luma, then Cb and Cr.
#[derive(Clone)]
struct Samples {
    luma: Vec<u8>,
    chroma: [Vec<u8>; 2],
}

/// What later blocks' syntax reads about each 16x16 area.
#[derive(Debug, Clone, Copy, Default)]
struct AreaState {
    /// Covered by a 16x16 coding unit (depth 1) rather than a 32x32 one.
    is_split: bool,
    is_skipped: bool,
    /// Luma mode of an intra, non-PCM coding unit.
    intra_mode: Option<IntraMode>,
}

/// Context variables for the syntax elements the encoder writes, by
/// ctxInc. initValues are from H.265 Tables 9-5 to 9-37: initType 0 for
/// I slices, 1 for P slices.
struct Contexts {
    split_cu: [Context; 3],
    cu_skip: [Context; 3],
    pred_mode: Context,
    /// part_mode's first bin, and merge_flag: only inter units code them.
    part_mode: Context,
    merge_flag: Context,
    prev_intra_luma_pred: Context,
    intra_chroma_pred_mode: Context,
    /// split_transform_flag, by 5 - log2 of the block side.
    split_transform: [Context; 3],
    /// cbf_luma: ctxInc 1 at trafoDepth 0, else 0.
    cbf_luma: [Context; 2],
    /// cbf_cb and cbf_cr share theirs, by trafoDepth.
    cbf_chroma: [Context; 3],
    /// cu_qp_delta_abs: its first bin, then the rest.
    cu_qp_delta: [Context; 2],
    residual: ResidualContexts,
}

impl Contexts {
    fn new(is_p_slice: bool) -> Self {
        let pick =
            |for_i: u8, for_p: u8| Context::new(if is_p_slice { for_p } else { for_i }, SLICE_QP);
        Self {
            split_cu: [pick(139, 107), pick(141, 139), pick(157, 126)],
            // Only coded in P slices.
            cu_skip: [197, 185, 201].map(|value| Context::new(value, SLICE_QP)),
            pred_mode: Context::new(149, SLICE_QP),
            part_mode: Context::new(154, SLICE_QP),
            merge_flag: Context::new(110, SLICE_QP),
            prev_intra_luma_pred: pick(184, 154),
            intra_chroma_pred_mode: pick(63, 152),
            split_transform: [pick(153, 124), pick(138, 138), pick(138, 94)],
            cbf_luma: [pick(111, 153), pick(141, 111)],
            cbf_chroma: [pick(94, 149), pick(138, 107), pick(182, 167)],
            cu_qp_delta: [154, 154].map(|value| Context::new(value, SLICE_QP)),
            residual: ResidualContexts::new(is_p_slice, SLICE_QP),
        }
    }
}

/// The quantisers a block with residual is tried at, coarsest first; it
/// takes the first that lands within the tolerance.
const QP_LADDER: [i32; 6] = [34, 28, 22, 16, 10, 4];
/// What a 16x16 PCM block costs: past this, raw samples are cheaper.
const PCM_BITS: usize = 8 * (SPLIT_CU * SPLIT_CU * 3 / 2);
/// The QP range cu_qp_delta wraps around in (8-bit video).
const QP_RANGE: i32 = 52;

/// Codes one picture's slice data, CTU by CTU in raster order, keeping
/// the picture a decoder shows (predictions and skips are judged by it).
struct PictureCoder<'a> {
    source: &'a YuvFrame,
    /// The decoder's picture: the one before, until a block is coded.
    shown: YuvFrame,
    is_p: bool,
    tolerance: i32,
    cabac: CabacWriter,
    contexts: Contexts,
    areas_wide: usize,
    areas: Vec<AreaState>,
    /// The QP the next coded cu_qp_delta is relative to.
    qp: i32,
    /// Whether this CTU (the quantisation group) has coded its delta.
    is_qp_coded: bool,
    /// Where on `QP_LADDER` the last block with residual landed.
    ladder_step: usize,
}

impl<'a> PictureCoder<'a> {
    fn new(
        source: &'a YuvFrame,
        shown_before: Option<&YuvFrame>,
        tolerance: u8,
        header: BitWriter,
    ) -> Self {
        let areas_wide = source.width / SPLIT_CU;
        Self {
            source,
            shown: shown_before.unwrap_or(source).clone(),
            is_p: shown_before.is_some(),
            tolerance: i32::from(tolerance),
            cabac: CabacWriter::new(header),
            contexts: Contexts::new(shown_before.is_some()),
            areas_wide,
            areas: vec![AreaState::default(); areas_wide * (source.height / SPLIT_CU)],
            qp: SLICE_QP,
            is_qp_coded: false,
            ladder_step: 0,
        }
    }

    fn code(mut self) -> (Vec<u8>, YuvFrame) {
        let ctbs_wide = self.source.width / CTB;
        let ctb_count = ctbs_wide * (self.source.height / CTB);
        for index in 0..ctb_count {
            self.code_ctu(index % ctbs_wide * CTB, index / ctbs_wide * CTB);
            if index + 1 < ctb_count {
                self.cabac.terminate(false); // end_of_slice_segment_flag
            }
        }
        (self.cabac.finish(), self.shown)
    }

    /// One CTU: a single 32x32 coding unit when one is good enough, else
    /// four 16x16 ones sharing one QP (the CTU is the quantisation group).
    fn code_ctu(&mut self, x: usize, y: usize) {
        self.is_qp_coded = false;
        let whole = Block { x, y, size: CTB };
        // split_cu_flag: ctxInc counts neighbours coded deeper (16x16).
        let ctx_inc = usize::from(x > 0 && self.area(x - 1, y).is_split)
            + usize::from(y > 0 && self.area(x, y - 1).is_split);
        if let Some(decision) = self.decide(whole, None) {
            self.cabac
                .decision(&mut self.contexts.split_cu[ctx_inc], false);
            self.code_cu(whole, decision);
            return;
        }
        self.cabac
            .decision(&mut self.contexts.split_cu[ctx_inc], true);
        let quarters =
            [(0, 0), (SPLIT_CU, 0), (0, SPLIT_CU), (SPLIT_CU, SPLIT_CU)].map(|(dx, dy)| Block {
                x: x + dx,
                y: y + dy,
                size: SPLIT_CU,
            });
        // A first pass finds the finest QP any quarter needs; the quarters
        // are then coded at that one QP, from the picture as it was.
        let before = self.read_samples(&self.shown, whole);
        let mut finest: Option<i32> = None;
        let mut first_pass = Vec::with_capacity(quarters.len());
        for block in quarters {
            let decision = self
                .decide(block, None)
                .expect("16x16 blocks can always fall back to PCM");
            if let CuCoding::Predicted {
                residual: Some(residual),
                ..
            }
            | CuCoding::CarriedOver { residual } = &decision.coding
            {
                finest = Some(finest.map_or(residual.qp, |qp| qp.min(residual.qp)));
            }
            self.write_shown(block, &decision.shown);
            first_pass.push(decision);
        }
        // Without residual the QP doesn't matter: the first pass stands.
        let mut decisions = first_pass.into_iter();
        if finest.is_some() {
            self.write_shown(whole, &before);
        }
        for block in quarters {
            // Nothing is deeper than 16x16, so its split_cu_flag is
            // always 0 with ctxInc 0.
            self.cabac.decision(&mut self.contexts.split_cu[0], false);
            let decided = decisions.next().expect("one decision per quarter");
            let decision = match finest {
                None => decided,
                Some(qp) => self
                    .decide(block, Some(qp))
                    .expect("16x16 blocks can always fall back to PCM"),
            };
            self.code_cu(block, decision);
        }
    }

    /// How to code `block`: skipped, a plain prediction, a prediction with
    /// residual (at `fixed_qp` when given), carried over with residual, or
    /// raw samples. `None` when a 32x32 block can't be coded whole and
    /// must split.
    fn decide(&mut self, block: Block, fixed_qp: Option<i32>) -> Option<Decision> {
        let source = self.read_samples(self.source, block);
        let current = self.read_samples(&self.shown, block);
        if self.is_p && self.is_close(&source, &current) {
            return Some(Decision {
                coding: CuCoding::Skip,
                shown: current,
                bits: 1,
            });
        }
        if !self.is_p || self.tolerance == 0 {
            return self.decide_intra(block, fixed_qp, &source);
        }
        // Each coding takes transforms to try, so a clear winner stops the
        // search: a small correction to the block already there, or else
        // an intra prediction that needs none.
        let carried = self.decide_carried_over(block, fixed_qp, &source, &current);
        if carried
            .as_ref()
            .is_some_and(|carried| carried.bits <= GOOD_ENOUGH_BITS)
        {
            return carried;
        }
        let intra = self.decide_intra(block, fixed_qp, &source);
        match (intra, carried) {
            (Some(intra), Some(carried)) if carried.bits < intra.bits => Some(carried),
            (intra, carried) => intra.or(carried),
        }
    }

    /// The previous picture's block plus residual, when that lands within
    /// the tolerance for fewer bits than raw samples.
    fn decide_carried_over(
        &mut self,
        block: Block,
        fixed_qp: Option<i32>,
        source: &Samples,
        current: &Samples,
    ) -> Option<Decision> {
        let needs = [
            !source.luma.within(&current.luma, self.tolerance),
            !source.chroma.within(&current.chroma, self.tolerance),
        ];
        let depths = 0..=transform_tree::MAX_INTER_DEPTH;
        let (residual, shown, bits) =
            self.with_residual(block, (source, current), needs, fixed_qp, depths)?;
        // Merged 2Nx2N with no residual would be a skip: never coded so.
        residual.has_levels().then_some(Decision {
            coding: CuCoding::CarriedOver { residual },
            shown,
            bits: bits + HEADER_BITS,
        })
    }

    /// The intra codings: a plain prediction, one with residual, or raw
    /// samples.
    fn decide_intra(
        &mut self,
        block: Block,
        fixed_qp: Option<i32>,
        source: &Samples,
    ) -> Option<Decision> {
        let luma_predictions = self.predictions(block, Plane::Luma);
        let chroma_predictions = [Plane::Cb, Plane::Cr].map(|plane| self.predictions(block, plane));
        let luma_candidates =
            INTRA_MODES.map(|mode| (mode, luma_predictions[mode.index()].clone()));
        let luma = self.pick(&source.luma, &luma_candidates);
        // Chroma tries the luma mode first: repeating it costs one bin.
        let chroma = luma.and_then(|(luma_mode, _)| {
            let order = std::iter::once(luma_mode).chain(INTRA_MODES);
            let candidates: Vec<(IntraMode, [Vec<u8>; 2])> = order
                .map(|mode| {
                    (
                        mode,
                        chroma_predictions.clone().map(|p| p[mode.index()].clone()),
                    )
                })
                .collect();
            self.pick(&source.chroma, &candidates)
        });
        let pcm = || Decision {
            coding: CuCoding::Pcm,
            shown: source.clone(),
            bits: PCM_BITS,
        };
        let (Some((luma_mode, luma_close)), Some((chroma_mode, chroma_close))) = (luma, chroma)
        else {
            return (block.size == SPLIT_CU).then(pcm);
        };
        let prediction = Samples {
            luma: luma_predictions[luma_mode.index()].clone(),
            chroma: chroma_predictions
                .clone()
                .map(|p| p[chroma_mode.index()].clone()),
        };
        if luma_close && chroma_close {
            return Some(Decision {
                coding: CuCoding::Predicted {
                    luma: luma_mode,
                    chroma: chroma_mode,
                    residual: None,
                },
                shown: prediction,
                bits: HEADER_BITS,
            });
        }
        let needs = [!luma_close, !chroma_close];
        match self.with_residual(block, (source, &prediction), needs, fixed_qp, 0..=0) {
            Some((residual, shown, bits)) => Some(Decision {
                coding: CuCoding::Predicted {
                    luma: luma_mode,
                    chroma: chroma_mode,
                    residual: Some(residual),
                },
                shown,
                bits: bits + HEADER_BITS,
            }),
            None => (block.size == SPLIT_CU).then(pcm),
        }
    }

    /// `prediction` corrected by residual in the planes `needs` names
    /// (luma, chroma), at the coarsest QP on the ladder (or `fixed_qp`)
    /// that lands within the tolerance, with the transform tree split as
    /// many times (of `depths`) as costs least: the residual, what a
    /// decoder shows and the residual's estimated bits. `None` when no QP
    /// does it for fewer bits than raw samples.
    fn with_residual(
        &mut self,
        block: Block,
        (source, prediction): (&Samples, &Samples),
        needs: [bool; 2],
        fixed_qp: Option<i32>,
        depths: std::ops::RangeInclusive<usize>,
    ) -> Option<(Residual, Samples, usize)> {
        let ladder: Vec<(usize, i32)> = match fixed_qp {
            Some(qp) => vec![(self.ladder_step, qp)],
            None => QP_LADDER
                .iter()
                .copied()
                .enumerate()
                .skip(self.ladder_step.saturating_sub(1))
                .collect(),
        };
        let pcm_bits = PCM_BITS * (block.size / SPLIT_CU).pow(2);
        // Each split's transforms, made the first time a QP needs them.
        let mut splits: Vec<(usize, Option<Coefficients>)> =
            depths.map(|depth| (depth, None)).collect();
        for (step, qp) in ladder {
            let mut cheapest: Option<(Residual, Samples, usize)> = None;
            for (depth, coefficients) in &mut splits {
                let coefficients = coefficients
                    .get_or_insert_with(|| Coefficients::of(source, prediction, needs, *depth));
                if let Some((residual, shown)) = self.attempt(source, prediction, coefficients, qp)
                {
                    let bits = residual.estimated_bits();
                    if cheapest.as_ref().is_none_or(|(_, _, best)| bits < *best) {
                        cheapest = Some((residual, shown, bits));
                    }
                }
                // Splitting can only save a few bits on a cheap block.
                if cheapest
                    .as_ref()
                    .is_some_and(|(_, _, bits)| *bits <= CHEAP_RESIDUAL_BITS)
                {
                    break;
                }
            }
            let Some((residual, shown, bits)) = cheapest else {
                continue;
            };
            if bits >= pcm_bits {
                break;
            }
            self.ladder_step = step;
            return Some((residual, shown, bits));
        }
        None
    }

    /// The first candidate whose prediction is within the tolerance;
    /// failing that, when residual is allowed, the closest one. The flag
    /// says whether it was close enough as it is.
    fn pick<S: SampleSet>(
        &self,
        source: &S,
        candidates: &[(IntraMode, S)],
    ) -> Option<(IntraMode, bool)> {
        if let Some((mode, _)) = candidates
            .iter()
            .find(|(_, prediction)| source.within(prediction, self.tolerance))
        {
            return Some((*mode, true));
        }
        if self.tolerance == 0 {
            return None;
        }
        candidates
            .iter()
            .min_by_key(|(_, prediction)| source.distance(prediction))
            .map(|(mode, _)| (*mode, false))
    }

    /// Quantise each plane's `coefficients` at `qp` and rebuild them as a
    /// decoder will. `None` when a sample lands outside the tolerance.
    fn attempt(
        &self,
        source: &Samples,
        prediction: &Samples,
        coefficients: &Coefficients,
        qp: i32,
    ) -> Option<(Residual, Samples)> {
        let unit = prediction.luma.len().isqrt();
        let (luma_side, chroma_side) = transform_tree::block_sides(unit, coefficients.depth);
        // Each block's levels, and the plane as a decoder rebuilds it.
        let code = |blocks: &[Vec<i64>], prediction: &[u8], side: usize, qp: i32| {
            let mut levels = Vec::with_capacity(blocks.len());
            let mut rebuilt = Vec::with_capacity(blocks.len());
            for block in blocks {
                let quantized = hevc_residual::quantize(block, side, qp);
                let has_levels = quantized.iter().any(|level| *level != 0);
                rebuilt.push(if has_levels {
                    hevc_residual::rebuild(&quantized, side, qp)
                } else {
                    vec![0; side * side]
                });
                levels.push(has_levels.then_some(quantized));
            }
            let whole = prediction.len().isqrt();
            let rebuilt = transform_tree::join_blocks(&rebuilt, side, whole);
            let shown: Vec<u8> = prediction
                .iter()
                .zip(&rebuilt)
                .map(|(p, r)| (i32::from(*p) + r).clamp(0, 255) as u8)
                .collect();
            (levels, shown)
        };
        let blocks_of = |side: usize, whole: usize| vec![None; (whole / side).pow(2)];
        let mut shown = prediction.clone();
        let mut residual = Residual {
            qp,
            depth: coefficients.depth,
            luma: blocks_of(luma_side, unit),
            chroma: [0, 1].map(|_| blocks_of(chroma_side, unit / 2)),
        };
        if let Some(luma) = &coefficients.luma {
            (residual.luma, shown.luma) = code(luma, &prediction.luma, luma_side, qp);
        }
        let chroma_qp = hevc_residual::chroma_qp(qp);
        for (plane, chroma) in coefficients.chroma.iter().enumerate() {
            if let Some(chroma) = chroma {
                (residual.chroma[plane], shown.chroma[plane]) =
                    code(chroma, &prediction.chroma[plane], chroma_side, chroma_qp);
            }
        }
        self.is_close(source, &shown).then_some((residual, shown))
    }

    fn is_close(&self, a: &Samples, b: &Samples) -> bool {
        a.luma.within(&b.luma, self.tolerance) && a.chroma.within(&b.chroma, self.tolerance)
    }

    fn code_cu(&mut self, block: Block, decision: Decision) {
        let is_skip = matches!(decision.coding, CuCoding::Skip);
        if self.is_p {
            let ctx_inc = usize::from(block.x > 0 && self.area(block.x - 1, block.y).is_skipped)
                + usize::from(block.y > 0 && self.area(block.x, block.y - 1).is_skipped);
            self.cabac
                .decision(&mut self.contexts.cu_skip[ctx_inc], is_skip);
            if !is_skip {
                // pred_mode_flag: 1 is MODE_INTRA.
                let is_intra = !matches!(decision.coding, CuCoding::CarriedOver { .. });
                self.cabac.decision(&mut self.contexts.pred_mode, is_intra);
            }
        }
        let is_split = block.size == SPLIT_CU;
        let intra_mode = match &decision.coding {
            CuCoding::Skip => None,
            CuCoding::CarriedOver { residual } => {
                // part_mode PART_2Nx2N, merge_flag; with one merge
                // candidate there is no merge_idx, and a merged 2Nx2N unit
                // has residual by definition, so no rqt_root_cbf either.
                self.cabac.decision(&mut self.contexts.part_mode, true);
                self.cabac.decision(&mut self.contexts.merge_flag, true);
                self.code_transform_tree(block, Some(residual), true);
                None
            }
            CuCoding::Pcm => {
                self.cabac.pcm(&pcm_samples(&decision.shown));
                None
            }
            CuCoding::Predicted {
                luma,
                chroma,
                residual,
            } => {
                // Only 16x16 coding units carry a pcm_flag.
                if is_split {
                    self.cabac.terminate(false);
                }
                self.code_intra_modes(block, *luma, *chroma);
                self.code_transform_tree(block, residual.as_ref(), false);
                Some(*luma)
            }
        };
        self.write_shown(block, &decision.shown);
        let state = AreaState {
            is_split,
            is_skipped: is_skip,
            intra_mode,
        };
        for y in (block.y..block.y + block.size).step_by(SPLIT_CU) {
            for x in (block.x..block.x + block.size).step_by(SPLIT_CU) {
                let index = self.area_index(x, y);
                self.areas[index] = state;
            }
        }
    }

    /// cu_qp_delta_abs and its sign (9.3.3.10): a unary prefix of up to
    /// five context-coded bins, then an Exp-Golomb suffix in bypass.
    fn code_qp_delta(&mut self, qp: i32) {
        let half = QP_RANGE / 2;
        let delta = (qp - self.qp + half).rem_euclid(QP_RANGE) - half;
        let magnitude = delta.unsigned_abs();
        for bin in 0..magnitude.min(5) {
            let ctx = usize::from(bin > 0);
            self.cabac
                .decision(&mut self.contexts.cu_qp_delta[ctx], true);
        }
        if magnitude < 5 {
            let ctx = usize::from(magnitude > 0);
            self.cabac
                .decision(&mut self.contexts.cu_qp_delta[ctx], false);
        } else {
            write_exp_golomb(&mut self.cabac, magnitude - 5);
        }
        if magnitude > 0 {
            self.cabac.bypass_bits(u32::from(delta < 0), 1);
        }
        self.qp = qp;
        self.is_qp_coded = true;
    }

    /// The luma mode as a most-probable-mode index or a remainder, then
    /// the chroma mode, which costs one bin when it repeats the luma one.
    fn code_intra_modes(&mut self, block: Block, luma: IntraMode, chroma: IntraMode) {
        let candidates = self.most_probable_modes(block);
        let luma_number = luma as u8;
        match candidates.iter().position(|&mode| mode == luma_number) {
            Some(mpm_idx) => {
                self.cabac
                    .decision(&mut self.contexts.prev_intra_luma_pred, true);
                // Truncated rice, cMax 2: 0, 10, 11.
                match mpm_idx {
                    0 => self.cabac.bypass_bits(0, 1),
                    1 => self.cabac.bypass_bits(0b10, 2),
                    _ => self.cabac.bypass_bits(0b11, 2),
                }
            }
            None => {
                self.cabac
                    .decision(&mut self.contexts.prev_intra_luma_pred, false);
                let below = candidates
                    .iter()
                    .filter(|&&mode| mode < luma_number)
                    .count();
                self.cabac
                    .bypass_bits(u32::from(luma_number) - below as u32, 5);
            }
        }
        if chroma == luma {
            self.cabac
                .decision(&mut self.contexts.intra_chroma_pred_mode, false);
        } else {
            self.cabac
                .decision(&mut self.contexts.intra_chroma_pred_mode, true);
            self.cabac.bypass_bits(chroma.chroma_index(), 2);
        }
    }

    /// candModeList (8.4.2) from the blocks left of and above `block`.
    /// The one above only counts inside the same CTB; PCM, skipped and
    /// missing neighbours count as DC.
    fn most_probable_modes(&self, block: Block) -> [u8; 3] {
        let mode_at = |x: usize, y: usize| self.area(x, y).intra_mode.map_or(MODE_DC, |m| m as u8);
        let left = if block.x > 0 {
            mode_at(block.x - 1, block.y)
        } else {
            MODE_DC
        };
        let above = if !block.y.is_multiple_of(CTB) {
            mode_at(block.x, block.y - 1)
        } else {
            MODE_DC
        };
        if left != above {
            let third = if left != MODE_PLANAR && above != MODE_PLANAR {
                MODE_PLANAR
            } else if left != MODE_DC && above != MODE_DC {
                MODE_DC
            } else {
                MODE_VERTICAL
            };
            return [left, above, third];
        }
        if left < 2 {
            return [MODE_PLANAR, MODE_DC, MODE_VERTICAL];
        }
        // The two angular modes beside it.
        [left, 2 + ((left + 29) % 32), 2 + ((left - 2 + 1) % 32)]
    }

    fn area_index(&self, x: usize, y: usize) -> usize {
        (y / SPLIT_CU) * self.areas_wide + x / SPLIT_CU
    }

    fn area(&self, x: usize, y: usize) -> AreaState {
        self.areas[self.area_index(x, y)]
    }

    /// `block`'s samples in `frame`.
    fn read_samples(&self, frame: &YuvFrame, block: Block) -> Samples {
        let read = |plane: Plane| {
            let (samples, stride) = plane.of(frame);
            let (x0, y0, size) = plane.scaled(block);
            let mut out = Vec::with_capacity(size * size);
            for row in 0..size {
                let start = (y0 + row) * stride + x0;
                out.extend_from_slice(&samples[start..start + size]);
            }
            out
        };
        Samples {
            luma: read(Plane::Luma),
            chroma: [read(Plane::Cb), read(Plane::Cr)],
        }
    }

    /// Put `samples` into the decoder's picture at `block`.
    fn write_shown(&mut self, block: Block, samples: &Samples) {
        let planes = [
            (Plane::Luma, &samples.luma),
            (Plane::Cb, &samples.chroma[0]),
            (Plane::Cr, &samples.chroma[1]),
        ];
        for (plane, values) in planes {
            let (x0, y0, size) = plane.scaled(block);
            let width = self.shown.width;
            let (target, stride) = match plane {
                Plane::Luma => (&mut self.shown.y, width),
                Plane::Cb => (&mut self.shown.u, width / 2),
                Plane::Cr => (&mut self.shown.v, width / 2),
            };
            for row in 0..size {
                let start = (y0 + row) * stride + x0;
                target[start..start + size].copy_from_slice(&values[row * size..(row + 1) * size]);
            }
        }
    }

    /// Whether the luma sample at (`x`, `y`) is decoded before `block`:
    /// inside the picture, and earlier in z-scan order (6.4.1). Earlier
    /// CTUs are decoded; inside the same CTU, an earlier 16x16 quarter.
    fn is_decoded(&self, x: isize, y: isize, block: Block) -> bool {
        let (width, height) = (self.source.width as isize, self.source.height as isize);
        if x < 0 || y < 0 || x >= width || y >= height {
            return false;
        }
        let (x, y) = (x as usize, y as usize);
        let ctbs_wide = self.source.width / CTB;
        let ctb_of = |x: usize, y: usize| (y / CTB) * ctbs_wide + x / CTB;
        let (neighbour, current) = (ctb_of(x, y), ctb_of(block.x, block.y));
        if neighbour != current {
            return neighbour < current;
        }
        let quarter = |x: usize, y: usize| (y % CTB) / SPLIT_CU * 2 + (x % CTB) / SPLIT_CU;
        quarter(x, y) < quarter(block.x, block.y)
    }

    /// The reference samples of `block` in `plane` (8.4.4.2.2), with
    /// missing ones substituted from their neighbours along the edge.
    fn references(&self, block: Block, plane: Plane) -> References {
        let (samples, stride) = plane.of(&self.shown);
        let (x0, y0, size) = plane.scaled(block);
        let scale = if plane == Plane::Luma { 1 } else { 2 };
        let sample = |x: isize, y: isize| {
            self.is_decoded(x * scale, y * scale, block)
                .then(|| i32::from(samples[y as usize * stride + x as usize]))
        };
        let (x0, y0, n) = (x0 as isize, y0 as isize, size as isize);
        // Bottom of the left column up to the corner, then the top row
        // left to right: the order substitution runs in.
        let mut edge: Vec<Option<i32>> = (0..2 * n)
            .rev()
            .map(|y| sample(x0 - 1, y0 + y))
            .chain([sample(x0 - 1, y0 - 1)])
            .chain((0..2 * n).map(|x| sample(x0 + x, y0 - 1)))
            .collect();
        match edge.iter().flatten().next().copied() {
            None => edge.fill(Some(128)),
            Some(first) => {
                edge[0].get_or_insert(first);
                for at in 1..edge.len() {
                    if edge[at].is_none() {
                        edge[at] = edge[at - 1];
                    }
                }
            }
        }
        let edge: Vec<i32> = edge.into_iter().flatten().collect();
        let two_n = 2 * size;
        References {
            left: (0..two_n).map(|y| edge[two_n - 1 - y]).collect(),
            corner: edge[two_n],
            top: edge[two_n + 1..].to_vec(),
        }
    }

    /// Every mode's prediction of `block` in `plane`, by `IntraMode::index`.
    fn predictions(&self, block: Block, plane: Plane) -> [Vec<u8>; 4] {
        let references = self.references(block, plane);
        let (_, _, size) = plane.scaled(block);
        let is_luma = plane == Plane::Luma;
        INTRA_MODES_BY_INDEX.map(|mode| references.predict(mode, size, is_luma))
    }
}

/// Exp-Golomb of order 0 in bypass bins (9.3.3.3).
fn write_exp_golomb(cabac: &mut CabacWriter, mut value: u32) {
    let mut k = 0;
    while value >= 1 << k {
        cabac.bypass_bits(1, 1);
        value -= 1 << k;
        k += 1;
    }
    cabac.bypass_bits(0, 1);
    cabac.bypass_bits(value, k);
}

/// A colour plane, and where a block's samples sit in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    Luma,
    Cb,
    Cr,
}

impl Plane {
    fn of(self, frame: &YuvFrame) -> (&[u8], usize) {
        match self {
            Plane::Luma => (&frame.y, frame.width),
            Plane::Cb => (&frame.u, frame.width / 2),
            Plane::Cr => (&frame.v, frame.width / 2),
        }
    }

    /// `block`'s corner and side in this plane's samples.
    fn scaled(self, block: Block) -> (usize, usize, usize) {
        match self {
            Plane::Luma => (block.x, block.y, block.size),
            _ => (block.x / 2, block.y / 2, block.size / 2),
        }
    }
}

/// Sample comparisons for one plane or a pair of chroma planes.
trait SampleSet {
    fn within(&self, other: &Self, tolerance: i32) -> bool;
    fn distance(&self, other: &Self) -> u32;
}

impl SampleSet for Vec<u8> {
    fn within(&self, other: &Self, tolerance: i32) -> bool {
        super::fidelity::is_faithful(self, other, tolerance)
    }

    fn distance(&self, other: &Self) -> u32 {
        self.iter()
            .zip(other)
            .map(|(a, b)| u32::from(a.abs_diff(*b)))
            .sum()
    }
}

impl SampleSet for [Vec<u8>; 2] {
    fn within(&self, other: &Self, tolerance: i32) -> bool {
        self[0].within(&other[0], tolerance) && self[1].within(&other[1], tolerance)
    }

    fn distance(&self, other: &Self) -> u32 {
        self[0].distance(&other[0]) + self[1].distance(&other[1])
    }
}

/// The reference samples around a block: the left column and top row,
/// each twice the block's side, and the corner between them.
struct References {
    left: Vec<i32>,
    top: Vec<i32>,
    corner: i32,
}

impl References {
    /// The [1 2 1] smoothing planar prediction of luma reads (8.4.4.2.3;
    /// strong smoothing is off in the SPS).
    fn smoothed(&self) -> Self {
        let last = self.left.len() - 1;
        let smooth = |edge: &[i32], at: usize| {
            let before = if at == 0 { self.corner } else { edge[at - 1] };
            (edge[at + 1] + 2 * edge[at] + before + 2) >> 2
        };
        let filtered = |edge: &[i32]| {
            (0..=last)
                .map(|at| {
                    if at == last {
                        edge[at]
                    } else {
                        smooth(edge, at)
                    }
                })
                .collect()
        };
        Self {
            left: filtered(&self.left),
            top: filtered(&self.top),
            corner: (self.left[0] + 2 * self.corner + self.top[0] + 2) >> 2,
        }
    }

    /// predSamples (8.4.4.2.4 to 8.4.4.2.6), row by row, with the edge
    /// filters luma blocks below 32x32 get.
    fn predict(&self, mode: IntraMode, size: usize, is_luma: bool) -> Vec<u8> {
        if mode == IntraMode::Planar && is_luma {
            return self.smoothed().predict(mode, size, false);
        }
        let filters_edges = is_luma && size < 32;
        let log2 = size.trailing_zeros();
        let sum: i32 = self.top[..size].iter().chain(&self.left[..size]).sum();
        let dc = (sum + size as i32) >> (log2 + 1);
        let n = size as i32;
        let clip = |value: i32| value.clamp(0, 255);
        let (top, left, corner) = (&self.top, &self.left, self.corner);
        let at = |x: usize, y: usize| match (mode, filters_edges) {
            (IntraMode::Planar, _) => {
                let (xi, yi) = (x as i32, y as i32);
                ((n - 1 - xi) * left[y]
                    + (xi + 1) * top[size]
                    + (n - 1 - yi) * top[x]
                    + (yi + 1) * left[size]
                    + n)
                    >> (log2 + 1)
            }
            (IntraMode::Dc, true) if x == 0 && y == 0 => (left[0] + 2 * dc + top[0] + 2) >> 2,
            (IntraMode::Dc, true) if y == 0 => (top[x] + 3 * dc + 2) >> 2,
            (IntraMode::Dc, true) if x == 0 => (left[y] + 3 * dc + 2) >> 2,
            (IntraMode::Dc, _) => dc,
            (IntraMode::Vertical, true) if x == 0 => clip(top[0] + ((left[y] - corner) >> 1)),
            (IntraMode::Vertical, _) => top[x],
            (IntraMode::Horizontal, true) if y == 0 => clip(left[0] + ((top[x] - corner) >> 1)),
            (IntraMode::Horizontal, _) => left[y],
        };
        (0..size * size)
            .map(|index| at(index % size, index / size) as u8)
            .collect()
    }
}

/// pcm_sample(): the luma block, then Cb, then Cr, row by row.
fn pcm_samples(samples: &Samples) -> Vec<u8> {
    [&samples.luma[..], &samples.chroma[0], &samples.chroma[1]].concat()
}

#[cfg(test)]
mod tests {
    use super::super::raster::{Canvas, Rgb};
    use super::*;

    const BACKGROUND: Rgb = Rgb::new(0x0e, 0x0f, 0x12);

    fn flat_frame(width: usize, height: usize) -> YuvFrame {
        Canvas::new(width, height, BACKGROUND).to_yuv420_padded(CTB)
    }

    /// Strips the two-byte NAL header and emulation prevention, then reads
    /// the first ue(v) fields, as a decoder finds the ids.
    fn leading_fields(nal: &[u8], skip_bits: usize, count: usize) -> Vec<u32> {
        let mut rbsp = Vec::new();
        let mut zeros = 0;
        for &byte in &nal[2..] {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            rbsp.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        let bits: Vec<bool> = rbsp
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |shift| (byte >> shift) & 1 == 1))
            .collect();
        let mut at = skip_bits;
        (0..count)
            .map(|_| {
                let zeros = bits[at..].iter().take_while(|bit| !**bit).count();
                let value = bits[at + zeros..at + 2 * zeros + 1]
                    .iter()
                    .fold(0, |value, bit| (value << 1) | u32::from(*bit));
                at += 2 * zeros + 1;
                value - 1
            })
            .collect()
    }

    /// Like the H.264 screen, the HEVC one must leave id 0 to the stream,
    /// or switching to it overwrites the stream's parameter sets in every
    /// decoder. The SPS id sits after the 4+3+1 header bits and the 96-bit
    /// profile_tier_level; the PPS names the SPS; slices name the PPS.
    #[test]
    fn the_screen_never_reuses_the_parameter_set_ids_of_the_stream() {
        let [vps, sps, pps] = parameter_sets(StreamShape {
            width: 1920,
            height: 1080,
            fps: 30,
        });
        assert_eq!(vps[0] >> 1, bitstream::HEVC_NAL_VPS);
        assert_eq!(vps[2] >> 4, VPS_ID as u8, "the VPS id is its first 4 bits");
        assert_eq!(sps[0] >> 1, bitstream::HEVC_NAL_SPS);
        assert_eq!(
            sps[2] >> 4,
            VPS_ID as u8,
            "the SPS points at the screen's VPS"
        );
        assert_eq!(leading_fields(&sps, 8 + 96, 1), vec![SPS_ID]);
        assert_eq!(leading_fields(&pps, 0, 2), vec![PPS_ID, SPS_ID]);
        for id in [VPS_ID, SPS_ID, PPS_ID] {
            assert_ne!(id, 0, "id 0 belongs to the stream");
        }

        let frame = flat_frame(64, 64);
        let idr = encode_idr(&frame);
        // first_slice_segment_in_pic_flag, no_output_of_prior_pics_flag.
        assert_eq!(leading_fields(&idr, 2, 1), vec![PPS_ID]);
        let p = encode_p(&frame, &frame, 1);
        assert_eq!(leading_fields(&p, 1, 1), vec![PPS_ID]);
    }

    #[test]
    fn level_is_the_smallest_in_table_a8_that_fits() {
        for (width, height, fps, level) in [
            (320, 180, 30, 60),
            (1280, 720, 30, 93),
            (1920, 1080, 30, 120),
            (1080, 1920, 30, 120),
            (1920, 1080, 60, 123),
            (2560, 1440, 30, 150),
            (3840, 2160, 60, 153),
            (8192, 16, 30, 150),
        ] {
            assert_eq!(
                level_idc(StreamShape { width, height, fps }),
                level,
                "{width}x{height} at {fps} fps"
            );
        }
    }

    /// The most probable modes the decoder derives, worked from 8.4.2:
    /// a DC (or missing) left neighbour gives planar, DC, vertical; an
    /// angular one leads its own list; two equal angular modes bring their
    /// neighbours.
    #[test]
    fn most_probable_modes_follow_the_spec() {
        let frame = flat_frame(64, 64);
        let mut coder = PictureCoder::new(&frame, None, 0, BitWriter::new());
        let block = Block {
            x: 16,
            y: 16,
            size: SPLIT_CU,
        };
        assert_eq!(coder.most_probable_modes(block), [0, 1, 26]);
        let index = coder.area_index(0, 16);
        coder.areas[index].intra_mode = Some(IntraMode::Horizontal);
        assert_eq!(coder.most_probable_modes(block), [10, 1, 0]);
        let index = coder.area_index(16, 0);
        coder.areas[index].intra_mode = Some(IntraMode::Horizontal);
        assert_eq!(coder.most_probable_modes(block), [10, 9, 11]);
        // Above in another CTB counts as DC.
        let top_of_ctb = Block {
            x: 32,
            y: 32,
            size: CTB,
        };
        let index = coder.area_index(32, 16);
        coder.areas[index].intra_mode = Some(IntraMode::Vertical);
        assert_eq!(coder.most_probable_modes(top_of_ctb), [0, 1, 26]);
    }

    /// An unchanged picture is one skip per CTU: a few bytes at 1080p,
    /// which is what keeps the held screen cheap.
    #[test]
    fn an_unchanged_p_picture_is_tiny() {
        let still = flat_frame(1920, 1080);
        let unchanged = encode_p(&still, &still, 1);
        assert!(
            unchanged.len() < 64,
            "{} bytes for an unchanged 1080p picture",
            unchanged.len()
        );
    }
}
