//! How an H.264 stream numbers its pictures, read from its parameter sets
//! and slice headers, so a wipe (`wipe`) can carry on from its last
//! picture as if the stream's own encoder had sent it.
//!
//! A decoder takes a P picture only in the order the stream has set up:
//! its frame_num one past the last reference picture's, and (when slices
//! count picture order) an order count after every picture already sent,
//! told in the low bits the decoder extends from the last reference
//! picture's (8.2.1.1). Reading that takes the SPS and the first slice
//! header of each picture since the last keyframe: a few dozen bits each.

use super::wipe::{PictureOrder, StreamNumbering};
use crate::h264::{strip_emulation_prevention, BitReader};

/// The SPS of a stream a wipe can continue: 8-bit 4:2:0 frames with no
/// scaling matrices, a profile with weighted prediction (Main or above),
/// and pictures ordered by count (type 0) or by frame_num (type 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSps {
    pub sps_id: u32,
    log2_max_frame_num: u32,
    /// None for order type 2; the count's bits for type 0.
    log2_max_order_lsb: Option<u32>,
    /// How many reference pictures a decoder keeps: the picture the wipe
    /// starts from has to be one of them.
    max_num_ref_frames: u32,
    pub width_mbs: usize,
    pub height_mbs: usize,
}

const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
const NAL_SLICE: u8 = 1;
const NAL_IDR_SLICE: u8 = 5;
/// The screen's own PPS id (`encoder::PPS_ID`), which a wipe's must
/// differ from too.
const SCREEN_PPS_ID: u32 = 1;
/// Highest PPS id there is.
const MAX_PPS_ID: u32 = 255;

/// The SPS `nal` (header byte included) if a wipe can continue a stream
/// under it.
pub fn read_sps(nal: &[u8]) -> Option<StreamSps> {
    if nal.first()? & 0x1F != NAL_SPS {
        return None;
    }
    let rbsp = strip_emulation_prevention(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let profile_idc = r.read_bits(8)?;
    r.read_bits(16)?; // constraint flags, level_idc
    let sps_id = r.read_ue()?;
    // Baseline has no weighted prediction.
    if profile_idc == 66 {
        return None;
    }
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        let chroma_format_idc = r.read_ue()?;
        let bit_depth_luma = r.read_ue()?;
        let bit_depth_chroma = r.read_ue()?;
        let transform_bypass = r.read_bit()?;
        let scaling_matrices = r.read_bit()?;
        if chroma_format_idc != 1
            || bit_depth_luma != 0
            || bit_depth_chroma != 0
            || transform_bypass != 0
            || scaling_matrices != 0
        {
            return None;
        }
    }
    let log2_max_frame_num = r.read_ue()? + 4;
    let log2_max_order_lsb = match r.read_ue()? {
        0 => Some(r.read_ue()? + 4),
        2 => None,
        _ => return None,
    };
    let max_num_ref_frames = r.read_ue()?;
    let _gaps_allowed = r.read_bit()?;
    let width_mbs = r.read_ue()? as usize + 1;
    let height_mbs = r.read_ue()? as usize + 1;
    let frame_mbs_only = r.read_bit()?;
    if frame_mbs_only != 1 || max_num_ref_frames == 0 || log2_max_frame_num > 16 {
        return None;
    }
    Some(StreamSps {
        sps_id,
        log2_max_frame_num,
        log2_max_order_lsb,
        max_num_ref_frames,
        width_mbs,
        height_mbs,
    })
}

/// The id of the PPS `nal` (header byte included).
pub fn read_pps_id(nal: &[u8]) -> Option<u32> {
    if nal.first()? & 0x1F != NAL_PPS {
        return None;
    }
    let rbsp = strip_emulation_prevention(nal.get(1..)?);
    BitReader::new(&rbsp).read_ue()
}

/// A stream followed picture by picture from a keyframe on.
#[derive(Debug, Clone)]
pub struct Follower {
    sps: StreamSps,
    /// frame_num of the last reference picture; None before a keyframe.
    last_reference: Option<u32>,
    /// The last reference picture's order count, split as the decoder
    /// keeps it: its high part and its low bits.
    reference_high: i64,
    reference_low: i64,
    /// The highest order count of any picture since the keyframe.
    highest: i64,
    /// The latest reference pictures since the keyframe, oldest first:
    /// frame_num and order count.
    references: Vec<(u32, i64)>,
}

/// Reference pictures remembered: as many as a decoder can hold.
const MAX_REFERENCES: usize = 16;

impl Follower {
    pub fn new(sps: StreamSps) -> Self {
        Self {
            sps,
            last_reference: None,
            reference_high: 0,
            reference_low: 0,
            highest: 0,
            references: Vec::new(),
        }
    }

    /// Take in one NAL unit of the stream, header included, in the order
    /// sent. Only a picture's first slice tells anything; a picture before
    /// the first keyframe is passed over. `None` when a slice header can't
    /// be read, after which nothing should be trusted.
    pub fn nal(&mut self, nal: &[u8]) -> Option<()> {
        let nal_type = nal.first()? & 0x1F;
        if nal_type != NAL_SLICE && nal_type != NAL_IDR_SLICE {
            return Some(());
        }
        let is_idr = nal_type == NAL_IDR_SLICE;
        let is_reference = nal[0] >> 5 & 3 != 0;
        if !is_idr && self.last_reference.is_none() {
            return Some(());
        }
        let rbsp = strip_emulation_prevention(&nal[1..]);
        let mut r = BitReader::new(&rbsp);
        if r.read_ue()? != 0 {
            // first_mb_in_slice: a later slice of a picture already read.
            return Some(());
        }
        r.read_ue()?; // slice_type
        r.read_ue()?; // pic_parameter_set_id
        let frame_num = r.read_bits(self.sps.log2_max_frame_num)?;
        if is_idr {
            r.read_ue()?; // idr_pic_id
            (self.reference_high, self.reference_low, self.highest) = (0, 0, 0);
            self.references.clear();
        }
        // Output order: the count, or (type 2) the order sent.
        let mut order = self.references.last().map_or(0, |(_, order)| order + 2);
        if let Some(bits) = self.sps.log2_max_order_lsb {
            let low = i64::from(r.read_bits(bits)?);
            let high = if is_idr {
                0
            } else {
                self.order_high(low, bits)
            };
            order = high + low;
            self.highest = if is_idr {
                order
            } else {
                self.highest.max(order)
            };
            if is_reference {
                (self.reference_high, self.reference_low) = (high, low);
            }
        }
        if is_reference {
            self.last_reference = Some(frame_num);
            if self.references.len() == MAX_REFERENCES {
                self.references.remove(0);
            }
            self.references.push((frame_num, order));
        }
        Some(())
    }

    /// The high part of a picture's order count from its low bits, as a
    /// decoder works it out against the last reference picture (8.2.1.1).
    fn order_high(&self, low: i64, bits: u32) -> i64 {
        let max = 1i64 << bits;
        let before = self.reference_low;
        if low < before && before - low >= max / 2 {
            self.reference_high + max
        } else if low > before && low - before > max / 2 {
            self.reference_high - max
        } else {
            self.reference_high
        }
    }

    /// How a wipe carries on from the pictures taken in, with a PPS id
    /// none of `stream_pps_ids` (nor the screen's own) uses. `None` before
    /// a keyframe, when the stream's order counts leave no room, or when
    /// the picture viewers see last has already left the decoder's
    /// reference pictures.
    pub fn numbering(&self, stream_pps_ids: &[u32]) -> Option<StreamNumbering> {
        let last_reference = self.last_reference?;
        let pps_id =
            (2..=MAX_PPS_ID).find(|id| !stream_pps_ids.contains(id) && *id != SCREEN_PPS_ID)?;
        let max_frame_num = 1u32 << self.sps.log2_max_frame_num;
        let order = match self.sps.log2_max_order_lsb {
            None => PictureOrder::FrameNum,
            Some(bits) => {
                // After every picture sent, by a step of two as encoders
                // count frames.
                let first = self.highest + 2;
                let reference = self.reference_high + self.reference_low;
                if first - reference >= (1i64 << bits) / 2 {
                    return None;
                }
                PictureOrder::Counted {
                    log2_max_lsb: bits,
                    first,
                }
            }
        };
        // The picture viewers see last: the reference latest in output
        // order (with B pictures that need not be the last one sent).
        let (shown_last, _) = *self.references.iter().max_by_key(|(_, order)| *order)?;
        let first_frame_num = (last_reference + 1) % max_frame_num;
        let frozen_back = (first_frame_num + max_frame_num - shown_last) % max_frame_num;
        // Each reference picture takes the next frame_num, and the decoder
        // keeps the latest max_num_ref_frames of them (sliding window).
        if frozen_back > self.sps.max_num_ref_frames {
            return None;
        }
        Some(StreamNumbering {
            sps_id: self.sps.sps_id,
            pps_id,
            log2_max_frame_num: self.sps.log2_max_frame_num,
            first_frame_num,
            frozen_back,
            order,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slate::bitstream::{nal_unit, BitWriter, NAL_IDR_SLICE as IDR, NAL_SLICE as SLICE};

    /// A Main profile SPS keeping `max_num_ref_frames` references, with
    /// 4-bit frame_nums and 6-bit order counts.
    fn sps(max_num_ref_frames: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.bits(77, 8); // profile_idc: Main
        w.bits(0, 16); // constraint flags, level_idc
        w.ue(0); // seq_parameter_set_id
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(0); // pic_order_cnt_type
        w.ue(2); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(max_num_ref_frames);
        w.bit(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(19); // pic_width_in_mbs_minus1
        w.ue(11); // pic_height_in_map_units_minus1
        w.bit(true); // frame_mbs_only_flag
        nal_unit(3, NAL_SPS, &w.finish())
    }

    /// A reference picture's slice header, as far as `Follower` reads it.
    fn slice(nal_type: u8, frame_num: u32, order_lsb: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.ue(0); // first_mb_in_slice
        w.ue(if nal_type == IDR { 7 } else { 5 }); // slice_type
        w.ue(0); // pic_parameter_set_id
        w.bits(frame_num, 4);
        if nal_type == IDR {
            w.ue(0); // idr_pic_id
        }
        w.bits(order_lsb, 6);
        nal_unit(2, nal_type, &w.finish())
    }

    /// Keyframe, then a P picture shown last, then a B picture kept as a
    /// reference after it: the wipe starts two references back.
    fn followed(max_num_ref_frames: u32) -> Option<StreamNumbering> {
        let mut follower = Follower::new(read_sps(&sps(max_num_ref_frames))?);
        for nal in [slice(IDR, 0, 0), slice(SLICE, 1, 8), slice(SLICE, 2, 4)] {
            follower.nal(&nal)?;
        }
        follower.numbering(&[0])
    }

    #[test]
    fn the_wipe_starts_from_the_picture_shown_last() {
        let numbering = followed(2).expect("both references kept");
        assert_eq!(numbering.first_frame_num, 3);
        assert_eq!(numbering.frozen_back, 2);
    }

    #[test]
    fn no_wipe_once_the_picture_shown_last_has_left_the_decoder() {
        assert_eq!(followed(1), None, "one reference kept: only the B picture");
    }
}
