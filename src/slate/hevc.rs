//! Lossless HEVC encoder for flat-colour screens: the H.265 counterpart
//! of `encoder`, for tracks the stream sends as HEVC (Twitch 2K channels).
//!
//! Each 32x32 coding tree unit is one coding unit when it can be, else
//! four 16x16 ones, coded as one of:
//! - intra DC, vertical or horizontal with no residual, when that
//!   prediction reproduces the block exactly (flat areas);
//! - PCM, 16x16 only: the raw samples, for anything with detail;
//! - skip: unchanged since the previous picture (zero motion).
//!
//! Every block reconstructs to exactly its source samples, so predictions
//! read the source frame as if it were the decoder's output. Deblocking
//! and SAO are off so nothing touches the reconstruction afterwards.
//!
//! Stream shape: Main profile, 8-bit 4:2:0, CABAC (HEVC has no other
//! entropy coder), one slice per picture, each P picture referencing the
//! one before it, no reordering.

use super::bitstream::{self, BitWriter};
use super::cabac::{CabacWriter, Context};
use super::encoder::StreamShape;
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
/// One merge candidate: skipped blocks need no merge_idx, and with every
/// inter block skipped at zero motion the candidate is always zero motion.
const FIVE_MINUS_MAX_MERGE_CANDIDATES: u32 = 4;

/// Intra prediction mode numbers (8.4.2) the encoder chooses from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntraMode {
    Dc = 1,
    Horizontal = 10,
    Vertical = 26,
}

const MODE_PLANAR: u8 = 0;
const MODE_DC: u8 = IntraMode::Dc as u8;
const MODE_VERTICAL: u8 = IntraMode::Vertical as u8;
const INTRA_MODES: [IntraMode; 3] = [IntraMode::Dc, IntraMode::Vertical, IntraMode::Horizontal];

impl IntraMode {
    /// intra_chroma_pred_mode 0..3 naming this mode (Table 8-2). Planar
    /// (0) is never chosen.
    fn chroma_index(self) -> u32 {
        match self {
            IntraMode::Vertical => 1,
            IntraMode::Horizontal => 2,
            IntraMode::Dc => 3,
        }
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
    w.ue(0); // max_transform_hierarchy_depth_inter
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
    w.bit(false); // cu_qp_delta_enabled_flag
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

/// IDR picture: every block intra or PCM. Consecutive HEVC IDRs need no
/// distinguishing id, so a replayed loop can reuse it as is.
pub fn encode_idr(frame: &YuvFrame) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.bit(true); // first_slice_segment_in_pic_flag
    w.bit(false); // no_output_of_prior_pics_flag
    w.ue(PPS_ID);
    w.ue(SLICE_TYPE_I);
    write_slice_tail(&mut w);
    let data = PictureCoder::new(frame, None, w).code();
    bitstream::hevc_nal_unit(bitstream::HEVC_NAL_IDR_N_LP, &data)
}

/// P picture `poc` pictures after the IDR: blocks identical to
/// `previous` are skipped, the rest are intra coded. Every P picture is a
/// reference so the next one can skip against it.
pub fn encode_p(frame: &YuvFrame, previous: &YuvFrame, poc: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.bit(true); // first_slice_segment_in_pic_flag
    w.ue(PPS_ID);
    w.ue(SLICE_TYPE_P);
    w.bits(poc % (1 << LOG2_MAX_POC_LSB), LOG2_MAX_POC_LSB);
    w.bit(true); // short_term_ref_pic_set_sps_flag: the SPS's only set
    w.bit(false); // num_ref_idx_active_override_flag
    w.ue(FIVE_MINUS_MAX_MERGE_CANDIDATES);
    write_slice_tail(&mut w);
    let data = PictureCoder::new(frame, Some(previous), w).code();
    bitstream::hevc_nal_unit(bitstream::HEVC_NAL_TRAIL_R, &data)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CuCoding {
    Skip,
    Predicted { luma: IntraMode, chroma: IntraMode },
    Pcm,
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
    prev_intra_luma_pred: Context,
    intra_chroma_pred_mode: Context,
    /// ctxInc 1: the transform tree is never split (trafoDepth 0).
    cbf_luma: Context,
    /// ctxInc 0 (trafoDepth 0), shared by cbf_cb and cbf_cr.
    cbf_chroma: Context,
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
            prev_intra_luma_pred: pick(184, 154),
            intra_chroma_pred_mode: pick(63, 152),
            cbf_luma: pick(141, 111),
            cbf_chroma: pick(94, 149),
        }
    }
}

/// Codes one picture's slice data, CTU by CTU in raster order.
struct PictureCoder<'a> {
    frame: &'a YuvFrame,
    /// The reference picture: P slices only.
    previous: Option<&'a YuvFrame>,
    cabac: CabacWriter,
    contexts: Contexts,
    areas_wide: usize,
    areas: Vec<AreaState>,
}

impl<'a> PictureCoder<'a> {
    fn new(frame: &'a YuvFrame, previous: Option<&'a YuvFrame>, header: BitWriter) -> Self {
        let areas_wide = frame.width / SPLIT_CU;
        Self {
            frame,
            previous,
            cabac: CabacWriter::new(header),
            contexts: Contexts::new(previous.is_some()),
            areas_wide,
            areas: vec![AreaState::default(); areas_wide * (frame.height / SPLIT_CU)],
        }
    }

    fn code(mut self) -> Vec<u8> {
        let ctbs_wide = self.frame.width / CTB;
        let ctb_count = ctbs_wide * (self.frame.height / CTB);
        for index in 0..ctb_count {
            self.code_ctu(index % ctbs_wide * CTB, index / ctbs_wide * CTB);
            if index + 1 < ctb_count {
                self.cabac.terminate(false); // end_of_slice_segment_flag
            }
        }
        self.cabac.finish()
    }

    fn code_ctu(&mut self, x: usize, y: usize) {
        let whole = Block { x, y, size: CTB };
        let coding = choose_coding(self.frame, self.previous, whole);
        // split_cu_flag: ctxInc counts neighbours coded deeper (16x16).
        let ctx_inc = usize::from(x > 0 && self.area(x - 1, y).is_split)
            + usize::from(y > 0 && self.area(x, y - 1).is_split);
        self.cabac
            .decision(&mut self.contexts.split_cu[ctx_inc], coding.is_none());
        if let Some(coding) = coding {
            self.code_cu(whole, coding);
            return;
        }
        for (dx, dy) in [(0, 0), (SPLIT_CU, 0), (0, SPLIT_CU), (SPLIT_CU, SPLIT_CU)] {
            let block = Block {
                x: x + dx,
                y: y + dy,
                size: SPLIT_CU,
            };
            // Nothing is deeper than 16x16, so its split_cu_flag is
            // always 0 with ctxInc 0.
            self.cabac.decision(&mut self.contexts.split_cu[0], false);
            let coding = choose_coding(self.frame, self.previous, block)
                .expect("16x16 blocks can always fall back to PCM");
            self.code_cu(block, coding);
        }
    }

    fn code_cu(&mut self, block: Block, coding: CuCoding) {
        if self.previous.is_some() {
            let ctx_inc = usize::from(block.x > 0 && self.area(block.x - 1, block.y).is_skipped)
                + usize::from(block.y > 0 && self.area(block.x, block.y - 1).is_skipped);
            let is_skip = coding == CuCoding::Skip;
            self.cabac
                .decision(&mut self.contexts.cu_skip[ctx_inc], is_skip);
            if !is_skip {
                // pred_mode_flag: MODE_INTRA.
                self.cabac.decision(&mut self.contexts.pred_mode, true);
            }
        }
        let is_split = block.size == SPLIT_CU;
        let intra_mode = match coding {
            CuCoding::Skip => None,
            CuCoding::Pcm => {
                self.cabac.pcm(&pcm_samples(self.frame, block));
                None
            }
            CuCoding::Predicted { luma, chroma } => {
                // Only 16x16 coding units carry a pcm_flag.
                if is_split {
                    self.cabac.terminate(false);
                }
                self.code_intra_modes(block, luma, chroma);
                // cbf_cb, cbf_cr, cbf_luma: no residual anywhere.
                self.cabac.decision(&mut self.contexts.cbf_chroma, false);
                self.cabac.decision(&mut self.contexts.cbf_chroma, false);
                self.cabac.decision(&mut self.contexts.cbf_luma, false);
                Some(luma)
            }
        };
        let state = AreaState {
            is_split,
            is_skipped: coding == CuCoding::Skip,
            intra_mode,
        };
        for y in (block.y..block.y + block.size).step_by(SPLIT_CU) {
            for x in (block.x..block.x + block.size).step_by(SPLIT_CU) {
                let index = self.area_index(x, y);
                self.areas[index] = state;
            }
        }
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
}

/// Skip when unchanged, else an exact intra prediction, else PCM where
/// the size allows it; `None` means the block must split.
fn choose_coding(frame: &YuvFrame, previous: Option<&YuvFrame>, block: Block) -> Option<CuCoding> {
    if previous.is_some_and(|previous| is_unchanged(frame, previous, block)) {
        return Some(CuCoding::Skip);
    }
    if let Some((luma, chroma)) = exact_prediction(frame, block) {
        return Some(CuCoding::Predicted { luma, chroma });
    }
    (block.size == SPLIT_CU).then_some(CuCoding::Pcm)
}

/// Luma and chroma modes that reproduce the block exactly. Chroma
/// prefers the luma mode, which is the cheapest to signal.
fn exact_prediction(frame: &YuvFrame, block: Block) -> Option<(IntraMode, IntraMode)> {
    let luma = PlaneBlock::luma(frame, block);
    let luma_mode = INTRA_MODES
        .into_iter()
        .find(|mode| luma.is_reproduced_by(*mode))?;
    let chroma = PlaneBlock::chroma(frame, block);
    let chroma_reproduced =
        |mode: IntraMode| chroma.iter().all(|plane| plane.is_reproduced_by(mode));
    let chroma_mode = std::iter::once(luma_mode)
        .chain(INTRA_MODES)
        .find(|mode| chroma_reproduced(*mode))?;
    Some((luma_mode, chroma_mode))
}

/// One plane's square block, and whether intra prediction filters its
/// edges (luma below 32x32, 8.4.4.2.6).
struct PlaneBlock<'a> {
    samples: &'a [u8],
    stride: usize,
    x0: usize,
    y0: usize,
    size: usize,
    filters_edges: bool,
}

impl<'a> PlaneBlock<'a> {
    fn luma(frame: &'a YuvFrame, block: Block) -> Self {
        Self {
            samples: &frame.y,
            stride: frame.width,
            x0: block.x,
            y0: block.y,
            size: block.size,
            filters_edges: block.size < 32,
        }
    }

    fn chroma(frame: &'a YuvFrame, block: Block) -> [Self; 2] {
        [&frame.u, &frame.v].map(|samples| Self {
            samples,
            stride: frame.width / 2,
            x0: block.x / 2,
            y0: block.y / 2,
            size: block.size / 2,
            filters_edges: false,
        })
    }

    fn at(&self, x: usize, y: usize) -> i32 {
        i32::from(self.samples[y * self.stride + x])
    }

    /// The reference samples DC, vertical and horizontal prediction read,
    /// after the substitution of 8.4.4.2.2. Left and above are always
    /// decoded when inside the picture; a missing side copies the nearest
    /// sample of the other, and with neither every sample is 128.
    fn references(&self) -> References {
        let (x0, y0, size) = (self.x0, self.y0, self.size);
        let read_top = || {
            (0..size)
                .map(|i| self.at(x0 + i, y0 - 1))
                .collect::<Vec<_>>()
        };
        let read_left = || {
            (0..size)
                .map(|i| self.at(x0 - 1, y0 + i))
                .collect::<Vec<_>>()
        };
        match (x0 > 0, y0 > 0) {
            (true, true) => References {
                top: read_top(),
                left: read_left(),
                corner: self.at(x0 - 1, y0 - 1),
            },
            (true, false) => {
                let left = read_left();
                References::filled(left[0], size, |references| references.left = left)
            }
            (false, true) => {
                let top = read_top();
                References::filled(top[0], size, |references| references.top = top)
            }
            (false, false) => References::filled(128, size, |_| {}),
        }
    }

    fn is_reproduced_by(&self, mode: IntraMode) -> bool {
        let references = self.references();
        let dc = references.dc();
        (0..self.size).all(|y| {
            (0..self.size).all(|x| {
                let predicted = references.predict(mode, dc, x, y, self.filters_edges);
                self.at(self.x0 + x, self.y0 + y) == predicted
            })
        })
    }
}

struct References {
    top: Vec<i32>,
    left: Vec<i32>,
    /// The sample above-left, p[-1][-1].
    corner: i32,
}

impl References {
    /// Every sample `value`, then `real` puts back the side that exists.
    fn filled(value: i32, size: usize, real: impl FnOnce(&mut Self)) -> Self {
        let mut references = Self {
            top: vec![value; size],
            left: vec![value; size],
            corner: value,
        };
        real(&mut references);
        references
    }

    /// dcVal (8.4.4.2.5): the mean of both edges, rounded.
    fn dc(&self) -> i32 {
        let size = self.top.len();
        let sum: i32 = self.top.iter().chain(&self.left).sum();
        (sum + size as i32) >> (size.trailing_zeros() + 1)
    }

    /// predSamples[x][y] (8.4.4.2.5 and 8.4.4.2.6), with the edge filters
    /// luma blocks below 32x32 get.
    fn predict(&self, mode: IntraMode, dc: i32, x: usize, y: usize, filters_edges: bool) -> i32 {
        let clip = |value: i32| value.clamp(0, 255);
        match (mode, filters_edges) {
            (IntraMode::Dc, true) if x == 0 && y == 0 => {
                (self.left[0] + 2 * dc + self.top[0] + 2) >> 2
            }
            (IntraMode::Dc, true) if y == 0 => (self.top[x] + 3 * dc + 2) >> 2,
            (IntraMode::Dc, true) if x == 0 => (self.left[y] + 3 * dc + 2) >> 2,
            (IntraMode::Dc, _) => dc,
            (IntraMode::Vertical, true) if x == 0 => {
                clip(self.top[0] + ((self.left[y] - self.corner) >> 1))
            }
            (IntraMode::Vertical, _) => self.top[x],
            (IntraMode::Horizontal, true) if y == 0 => {
                clip(self.left[0] + ((self.top[x] - self.corner) >> 1))
            }
            (IntraMode::Horizontal, _) => self.left[y],
        }
    }
}

/// pcm_sample(): the luma block, then Cb, then Cr, row by row.
fn pcm_samples(frame: &YuvFrame, block: Block) -> Vec<u8> {
    let mut samples = Vec::with_capacity(block.size * block.size * 3 / 2);
    for row in 0..block.size {
        let start = (block.y + row) * frame.width + block.x;
        samples.extend_from_slice(&frame.y[start..start + block.size]);
    }
    let (chroma_width, chroma_size) = (frame.width / 2, block.size / 2);
    for plane in [&frame.u, &frame.v] {
        for row in 0..chroma_size {
            let start = (block.y / 2 + row) * chroma_width + block.x / 2;
            samples.extend_from_slice(&plane[start..start + chroma_size]);
        }
    }
    samples
}

fn is_unchanged(frame: &YuvFrame, previous: &YuvFrame, block: Block) -> bool {
    let rows_match =
        |plane: &[u8], before: &[u8], stride: usize, x: usize, y: usize, size: usize| {
            (0..size).all(|row| {
                let start = (y + row) * stride + x;
                plane[start..start + size] == before[start..start + size]
            })
        };
    let (chroma_width, half) = (frame.width / 2, block.size / 2);
    rows_match(
        &frame.y,
        &previous.y,
        frame.width,
        block.x,
        block.y,
        block.size,
    ) && rows_match(
        &frame.u,
        &previous.u,
        chroma_width,
        block.x / 2,
        block.y / 2,
        half,
    ) && rows_match(
        &frame.v,
        &previous.v,
        chroma_width,
        block.x / 2,
        block.y / 2,
        half,
    )
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
        let mut coder = PictureCoder::new(&frame, None, BitWriter::new());
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
