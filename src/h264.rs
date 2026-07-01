//! Codec parsing for FLV video/audio tag payloads.
//!
//! Covers both legacy (AVC, AAC, MP3) and Enhanced RTMP (HEVC, AV1, VP9,
//! Opus, AC-3, EAC-3, FLAC, multi-track) tag headers. We only need to
//! identify three things - actual decoding never happens:
//!
//!   * sequence headers (cache + resend on every cut and reconnect)
//!   * keyframes / IDRs (cut points the player can resync to)
//!   * multi-track packets (Twitch Enhanced Broadcasting - kept raw in
//!     the buffer; the pump flattens per-destination via
//!     `select_video_bytes` so Twitch receives the simulcast while
//!     every other destination gets a standard single-track tag)
//!
//! The buffer stores tags bit-faithfully as ingest received them. The
//! only transformation we apply is `select_video_bytes` in the pump
//! right before each `sink.send_video` call, on a per-destination
//! basis - single-track tags are borrowed through with zero copies.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Avc,  // H.264
    Hevc, // H.265
    Av1,
    Vp9,
    Unknown,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::Avc => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VideoTagInfo {
    pub is_seq_header: bool,
    pub is_idr: bool,
    pub is_multitrack: bool,
    /// Enhanced-RTMP `PacketTypeMetadata` (=4) carries mid-stream
    /// updates like HDR `colorInfo`. Bit-faithfully passed through to
    /// the destination - flagged here only so the trace log doesn't
    /// mistake it for an anomalous packet, and so callers that cache
    /// "must-replay-on-cut" packets can choose to include it.
    pub is_metadata: bool,
    pub codec: VideoCodec,
}

const FOURCC_HVC1: [u8; 4] = *b"hvc1";
const FOURCC_HEV1: [u8; 4] = *b"hev1";
const FOURCC_AV01: [u8; 4] = *b"av01";
const FOURCC_VP09: [u8; 4] = *b"vp09";
const FOURCC_AVC1: [u8; 4] = *b"avc1";

pub fn classify_video_tag(payload: &[u8]) -> VideoTagInfo {
    let unknown = VideoTagInfo {
        is_seq_header: false,
        is_idr: false,
        is_multitrack: false,
        is_metadata: false,
        codec: VideoCodec::Unknown,
    };
    if payload.is_empty() {
        return unknown;
    }
    let b0 = payload[0];
    let is_ex_header = b0 & 0x80 != 0;

    if !is_ex_header {
        if payload.len() < 5 {
            return unknown;
        }
        let codec_id = b0 & 0x0F;
        if codec_id != 7 {
            return unknown; // We only understand AVC in legacy mode.
        }
        let avc_packet_type = payload[1];
        let is_seq_header = avc_packet_type == 0;
        // Sequence headers carry SPS/PPS, never an IDR slice - trust the
        // NAL walk rather than the FLV FrameType nibble (some encoders set
        // FrameType=key on the seq header too).
        let is_idr = avc_packet_type == 1 && contains_idr_nalu(&payload[5..]);
        return VideoTagInfo {
            is_seq_header,
            is_idr,
            is_multitrack: false,
            is_metadata: false,
            codec: VideoCodec::Avc,
        };
    }

    // Enhanced RTMP: bit7=IsEx, bits6:4=FrameType, bits3:0=PacketType.
    //   PacketType 0=SequenceStart, 1=CodedFrames, 2=SequenceEnd,
    //              3=CodedFramesX (no composition time),
    //              4=Metadata (mid-stream HDR colorInfo etc.),
    //              5=MPEG2TSSequenceStart, 6=Multitrack.
    let frame_type = (b0 >> 4) & 0x07;
    let packet_type = b0 & 0x0F;
    let is_seq_header = packet_type == 0;
    let is_metadata = packet_type == 4;
    // Enhanced encoders always set FrameType=1 on keyframes. For AVC we
    // walked NALUs because some encoders flag P-frames as key incorrectly;
    // enhanced-rtmp tightens this and FrameType is the spec-blessed signal.
    // SeqStart / Metadata packets carry no slice data, so refuse to flag
    // them as IDR even if the encoder set FrameType=1.
    let is_keyframe = frame_type == 1 && !is_seq_header && !is_metadata;
    let is_multitrack = packet_type == 6;

    // For multi-track, the FourCC sits behind the multitrack header; we
    // don't bother classifying the inner codec here because each
    // destination's pump either passes the multi-track tag through
    // (Twitch) or flattens it to single-track right before send (every
    // other destination, via `select_video_bytes`).
    //
    // Crucially: the *nested* PacketType lives in the low nibble of
    // byte 1, not byte 0. A multi-track tag carrying decoder config
    // has outer packet_type=6 but nested packet_type=0 - and that's
    // what determines whether this is a seq header that needs caching
    // for re-emit on cuts. Same nested check refuses IDR-cut treatment
    // on a multi-track config tag where the encoder also set
    // FrameType=1 (some do).
    if is_multitrack {
        let nested_pt = payload.get(1).map(|b| b & 0x0F).unwrap_or(0xFF);
        let is_mt_seq_header = nested_pt == 0;
        let is_mt_metadata = nested_pt == 4;
        return VideoTagInfo {
            is_seq_header: is_mt_seq_header,
            is_idr: is_keyframe && !is_mt_seq_header && !is_mt_metadata,
            is_multitrack: true,
            is_metadata: is_mt_metadata,
            codec: VideoCodec::Unknown,
        };
    }

    if payload.len() < 5 {
        return VideoTagInfo {
            is_seq_header,
            is_idr: is_keyframe,
            is_multitrack: false,
            is_metadata,
            codec: VideoCodec::Unknown,
        };
    }
    let fourcc = [payload[1], payload[2], payload[3], payload[4]];
    let codec = match fourcc {
        FOURCC_HVC1 | FOURCC_HEV1 => VideoCodec::Hevc,
        FOURCC_AV01 => VideoCodec::Av1,
        FOURCC_VP09 => VideoCodec::Vp9,
        FOURCC_AVC1 => VideoCodec::Avc,
        _ => VideoCodec::Unknown,
    };
    VideoTagInfo {
        is_seq_header,
        is_idr: is_keyframe,
        is_multitrack: false,
        is_metadata,
        codec,
    }
}

fn contains_idr_nalu(mut data: &[u8]) -> bool {
    while data.len() >= 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if 4 + len > data.len() {
            return false;
        }
        let nal_unit_type = data[4] & 0x1F;
        if nal_unit_type == 5 {
            return true;
        }
        data = &data[4 + len..];
    }
    false
}

/// Flatten an Enhanced RTMP multi-track video tag down to a standard
/// single-track tag carrying the first listed track only.
///
/// Twitch's Enhanced Broadcasting feeds simulcast resolutions in a single
/// RTMP stream - picking the first track keeps the user's primary
/// resolution (encoders are required by the spec to list the primary
/// first). Returns `None` if the payload isn't a recognised multi-track
/// layout, in which case the caller should pass the bytes through and
/// let the upstream platform error gracefully.
///
/// Output layout matches a normal enhanced video tag:
///   byte 0 = 0x80 | (FrameType<<4) | nested_PacketType
///   bytes 1..5 = FourCC
///   bytes 5..  = the chosen track's payload
pub fn flatten_multitrack_video(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() < 6 {
        return None;
    }
    let b0 = payload[0];
    if (b0 & 0x80) == 0 || (b0 & 0x0F) != 6 {
        return None;
    }
    let frame_type = (b0 >> 4) & 0x07;
    let mt_header = payload[1];
    let mt_type = (mt_header >> 4) & 0x0F;
    let nested_pt = mt_header & 0x0F;

    // Stitch the rewritten single-track tag header.
    let mut out = Vec::with_capacity(payload.len());
    let header_byte = 0x80 | ((frame_type & 0x07) << 4) | (nested_pt & 0x0F);

    match mt_type {
        0 => {
            // OneTrack: [FourCC(4)][TrackId(1)][payload..]
            if payload.len() < 7 {
                return None;
            }
            let fourcc = &payload[2..6];
            let track_payload = &payload[7..];
            out.push(header_byte);
            out.extend_from_slice(fourcc);
            out.extend_from_slice(track_payload);
            Some(out)
        }
        1 => {
            // ManyTracks: [FourCC(4)] then repeated
            //   [TrackId(1)][SizeOfVideoTrack(3 BE)][payload]
            // Take the FIRST track in the list (primary by spec).
            if payload.len() < 6 + 4 {
                return None;
            }
            let fourcc = &payload[2..6];
            let rest = &payload[6..];
            if rest.len() < 4 {
                return None;
            }
            let track_len = u32::from_be_bytes([0, rest[1], rest[2], rest[3]]) as usize;
            if rest.len() < 4 + track_len {
                return None;
            }
            let track_payload = &rest[4..4 + track_len];
            out.push(header_byte);
            out.extend_from_slice(fourcc);
            out.extend_from_slice(track_payload);
            Some(out)
        }
        2 => {
            // ManyTracksManyCodecs: repeated
            //   [TrackId(1)][FourCC(4)][SizeOfVideoTrack(3 BE)][payload]
            let rest = &payload[2..];
            if rest.len() < 8 {
                return None;
            }
            let fourcc = &rest[1..5];
            let track_len = u32::from_be_bytes([0, rest[5], rest[6], rest[7]]) as usize;
            if rest.len() < 8 + track_len {
                return None;
            }
            let track_payload = &rest[8..8 + track_len];
            out.push(header_byte);
            out.extend_from_slice(fourcc);
            out.extend_from_slice(track_payload);
            Some(out)
        }
        _ => None,
    }
}

/// Convert an Enhanced-RTMP `OneTrack` **AVC** video tag into a *legacy*
/// AVC FLV tag (`0x17`/`0x27` ...). Some ingests - notably YouTube's
/// vertical / secondary ingest - accept legacy AVC rock-solid but are
/// flaky with Enhanced-RTMP `avc1` framing: the stream is viewable yet
/// the socket is aborted on a ~11 s cycle. The horizontal primary reaches
/// those ingests as legacy AVC already (OBS emits it that way), so a
/// vertical destination - which only exists as a multi-track tag - is the
/// one that hits the E-RTMP path. Rewriting it to legacy AVC gives the
/// vertical feed the exact framing YouTube already ingests happily.
///
/// Returns `None` (caller falls back to the generic E-RTMP flatten) when
/// the tag is not a OneTrack AVC coded-frame / seq-header tag we can
/// safely convert - non-AVC codecs (HEVC/AV1 can't be legacy-framed),
/// ManyTracks bundles, metadata/colour packets, or truncated input.
fn onetrack_avc_to_legacy(payload: &[u8]) -> Option<Vec<u8>> {
    // [b0][mt_header][FourCC(4)][TrackId(1)][body..] - at least 8 bytes.
    if payload.len() < 8 {
        return None;
    }
    let b0 = payload[0];
    if (b0 & 0x80) == 0 || (b0 & 0x0F) != 6 {
        return None; // not an Enhanced-RTMP multi-track tag
    }
    let mt_header = payload[1];
    if (mt_header >> 4) & 0x0F != 0 {
        return None; // only the OneTrack layout carries a single track here
    }
    if payload[2..6] != FOURCC_AVC1 {
        return None; // legacy framing only covers AVC
    }
    let frame_type = (b0 >> 4) & 0x07;
    let nested_pt = mt_header & 0x0F;
    let body = &payload[7..];
    // FrameType 1 = keyframe -> 0x17, anything else -> inter -> 0x27.
    let legacy_head = if frame_type == 1 { 0x17u8 } else { 0x27u8 };
    let mut out = Vec::with_capacity(body.len() + 5);
    match nested_pt {
        // SequenceStart: body IS the AVCDecoderConfigurationRecord.
        0 => {
            out.push(0x17); // seq header is always sent on a keyframe head
            out.push(0x00); // AVCPacketType: sequence header
            out.extend_from_slice(&[0, 0, 0]); // composition time = 0
            out.extend_from_slice(body);
        }
        // CodedFrames: body = [CompositionTime(3)][length-prefixed NALs].
        1 => {
            if body.len() < 3 {
                return None;
            }
            out.push(legacy_head);
            out.push(0x01); // AVCPacketType: NALU
            out.extend_from_slice(&body[0..3]); // preserve composition time
            out.extend_from_slice(&body[3..]);
        }
        // CodedFramesX: body = [length-prefixed NALs], composition time 0.
        3 => {
            out.push(legacy_head);
            out.push(0x01);
            out.extend_from_slice(&[0, 0, 0]);
            out.extend_from_slice(body);
        }
        // Metadata / colour info (nested_pt 4) and anything else have no
        // legacy AVC equivalent - drop rather than mix E-RTMP into an
        // otherwise-legacy stream.
        _ => return None,
    }
    Some(out)
}

/// Best-effort extraction of the Enhanced-RTMP TrackId carried by an
/// Enhanced-RTMP `OneTrack`-layout multi-track video tag (the format
/// OBS uses for Enhanced Broadcasting seq-headers, where each track's
/// SPS/PPS arrives as its own tag with TrackId in byte 6). Returns 0
/// for:
///   * legacy single-track payloads
///   * Enhanced-RTMP single-track payloads (IsEx bit set but
///     PacketType != Multitrack)
///   * Multi-track ManyTracks / ManyTracksManyCodecs layouts (the
///     whole tag holds every track's config - one slot is correct)
///   * Truncated payloads we can't parse safely
///
/// Used by the seq-header cache to key per-track entries so a
/// multi-track stream's full config is preserved across cuts /
/// reconnects, instead of being silently overwritten by the
/// last-received track.
pub fn seq_header_track_id(payload: &[u8]) -> u8 {
    if payload.len() < 2 {
        return 0;
    }
    let b0 = payload[0];
    // Not Enhanced-RTMP at all - legacy AVC/HEVC tag, no track id.
    if b0 & 0x80 == 0 {
        return 0;
    }
    // Enhanced-RTMP but not the Multitrack PacketType - single-track,
    // no track id field to read.
    if b0 & 0x0F != 6 {
        return 0;
    }
    // Multi-track layout type lives in the high nibble of byte 1.
    // OneTrack = 0; ManyTracks = 1; ManyTracksManyCodecs = 2. Only
    // the OneTrack layout has a per-tag track id at byte 6; the other
    // layouts pack every track into one tag so the single-slot key 0
    // already captures the whole config.
    let mt_type = (payload[1] >> 4) & 0x0F;
    if mt_type != 0 {
        return 0;
    }
    if payload.len() < 7 {
        return 0;
    }
    payload[6]
}

/// True iff this video tag is a keyframe carrying the PRIMARY track's
/// IDR. Used to gate which tags enter the buffer's IDR-only secondary
/// index (the cut-point selector). For multi-track Enhanced Broadcasting
/// streams every ladder rung has its own IDR cadence; if we index all
/// of them as cut candidates, `compute_delay_cut` lands on whichever
/// rung's IDR happens to be at the closest seq, and the destination
/// decoder for the OTHER rungs receives a P-frame with no reference -
/// the pixel-glitch artefact users see on EB cuts. Restricting the
/// IDR index to primary-track keyframes only guarantees the cut lands
/// at a temporal moment where the legacy decoder has its anchor, and
/// since OBS aligns every ladder track's IDR to the same encoder PTS
/// the other rungs naturally have IDRs at the same input ts too.
///
/// Returns true for:
///   * legacy AVC keyframes (byte 0 == 0x17) - single-track, IS primary
///   * Enhanced-RTMP single-track keyframes (Ex bit + FrameType=1 +
///     PacketType != Multitrack)
///   * Enhanced-RTMP OneTrack multi-track keyframes with TrackId == 0
///
/// Returns false for non-key frames, non-primary multi-track tags, and
/// payloads too short to classify.
pub fn is_primary_video_idr(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    let b0 = payload[0];
    if (b0 & 0x80) == 0 {
        // Legacy: FrameType=1 (key, high nibble) + CodecID=7 (AVC, low
        // nibble) packs into 0x17. AVC has only one track per stream,
        // so any AVC keyframe IS the primary IDR.
        return b0 == 0x17;
    }
    // Enhanced-RTMP. bits6:4 = FrameType, bits3:0 = PacketType.
    let frame_type = (b0 >> 4) & 0x07;
    if frame_type != 1 {
        return false;
    }
    let packet_type = b0 & 0x0F;
    if packet_type != 6 {
        // Single-track Enhanced-RTMP keyframe - one track per stream,
        // so the keyframe IS the primary.
        return true;
    }
    // Multitrack tag. mt_type in byte 1 high nibble; only OneTrack (=0)
    // has a per-tag TrackId at byte 6. ManyTracks layouts pack every
    // track into one tag and we treat them as primary too (the cut is
    // semantically aligned for the whole bundle).
    if payload.len() < 2 {
        return false;
    }
    let mt_type = (payload[1] >> 4) & 0x0F;
    if mt_type != 0 {
        return true;
    }
    if payload.len() < 7 {
        return false;
    }
    payload[6] == 0
}

/// Per-destination video egress policy, computed by the egress pump from
/// the destination's platform + format settings:
///
/// - `Passthrough`: Twitch destinations with an EB session. Multi-track
///   tags go through bit-faithfully (the simulcast ladder + every canvas).
/// - `Track(n)`: every other case. Forward only OneTrack `TrackId == n`,
///   flattened to a standard single-track tag. `Track(0)` is the
///   horizontal primary (the default for all non-Twitch destinations);
///   `Track(n)` with n>0 is the vertical-canvas primary chosen by
///   `detect_vertical_primary_track`. Single-track / legacy tags pass
///   through unchanged in either case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEgress {
    Passthrough,
    Track(u8),
}

/// Locate the AVCDecoderConfigurationRecord inside a *sequence-header*
/// video tag payload, across the framings OBS emits. Returns `None` for
/// anything that isn't an AVC seq header we can read (legacy non-AVC,
/// HEVC/AV1 Enhanced configs, non-seq-header tags, ManyTracks bundles,
/// or truncated input). Best-effort and fully bounds-checked - never
/// panics on hostile / partial bytes.
fn avc_config_record(payload: &[u8]) -> Option<&[u8]> {
    if payload.is_empty() {
        return None;
    }
    let b0 = payload[0];
    // Legacy AVC: byte0 low nibble = CodecID (7 = AVC), byte1 = AVCPacketType
    // (0 = seq header), bytes 2..5 = composition time, then the config record.
    if b0 & 0x80 == 0 {
        if b0 & 0x0F != 7 || payload.get(1) != Some(&0) || payload.len() < 5 {
            return None;
        }
        return payload.get(5..);
    }
    let packet_type = b0 & 0x0F;
    if packet_type == 6 {
        // Multitrack. Only the OneTrack layout (mt_type 0) with a nested
        // SequenceStart (nested_pt 0) carries a per-track config record at
        // a known offset (FourCC[4] + TrackId[1] then the record).
        let mt = *payload.get(1)?;
        if (mt >> 4) & 0x0F != 0 || mt & 0x0F != 0 {
            return None;
        }
        if payload.len() < 7 || payload.get(2..6) != Some(&FOURCC_AVC1) {
            return None;
        }
        return payload.get(7..);
    }
    // Enhanced single-track. Only SequenceStart (packet_type 0) carries the
    // config record; bytes 1..5 are the FourCC, then the record.
    if packet_type != 0 || payload.len() < 5 || payload.get(1..5) != Some(&FOURCC_AVC1) {
        return None;
    }
    payload.get(5..)
}

/// Pull the first SPS NAL (RBSP, NAL header byte included) out of an
/// AVCDecoderConfigurationRecord. Bounds-checked; `None` on short input.
fn first_sps_nal(cfg: &[u8]) -> Option<&[u8]> {
    // cfg: [version, profile, compat, level, lengthSizeMinusOne,
    //       numOfSPS (low 5 bits), spsLength(2 BE), sps...]
    let num_sps = cfg.get(5)? & 0x1F;
    if num_sps == 0 {
        return None;
    }
    let len = u16::from_be_bytes([*cfg.get(6)?, *cfg.get(7)?]) as usize;
    cfg.get(8..8 + len)
}

/// Decode (width, height) in pixels from an AVC sequence header tag.
/// Returns `None` for non-AVC, malformed, or truncated input. Used only
/// to classify orientation (portrait vs landscape) for vertical-canvas
/// selection - it parses just enough of the SPS to reach the picture
/// dimensions and applies frame cropping. Never panics.
pub fn sps_dimensions(seq_header: &[u8]) -> Option<(u32, u32)> {
    let cfg = avc_config_record(seq_header)?;
    let nal = first_sps_nal(cfg)?;
    // Drop the 1-byte NAL header, then strip emulation-prevention bytes
    // (0x00 00 03 -> 0x00 00) before Exp-Golomb parsing.
    let rbsp = strip_emulation_prevention(nal.get(1..)?);
    let mut r = BitReader::new(&rbsp);

    let profile_idc = r.read_bits(8)?;
    let _constraints_and_reserved = r.read_bits(8)?;
    let _level_idc = r.read_bits(8)?;
    let _sps_id = r.read_ue()?;

    let mut chroma_format_idc = 1u32; // 4:2:0 default for older profiles
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = r.read_ue()?;
        if chroma_format_idc == 3 {
            let _separate_colour_plane = r.read_bit()?;
        }
        let _bit_depth_luma = r.read_ue()?;
        let _bit_depth_chroma = r.read_ue()?;
        let _qpprime = r.read_bit()?;
        if r.read_bit()? == 1 {
            // seq_scaling_matrix_present_flag
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..count {
                if r.read_bit()? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    r.skip_scaling_list(size)?;
                }
            }
        }
    }

    let _log2_max_frame_num = r.read_ue()?;
    let pic_order_cnt_type = r.read_ue()?;
    if pic_order_cnt_type == 0 {
        let _log2_max_poc_lsb = r.read_ue()?;
    } else if pic_order_cnt_type == 1 {
        let _delta_pic_order_always_zero = r.read_bit()?;
        let _offset_for_non_ref_pic = r.read_se()?;
        let _offset_for_top_to_bottom = r.read_se()?;
        let cycle = r.read_ue()?;
        for _ in 0..cycle {
            let _offset = r.read_se()?;
        }
    }
    let _max_num_ref_frames = r.read_ue()?;
    let _gaps_allowed = r.read_bit()?;

    let pic_width_in_mbs = r.read_ue()?.checked_add(1)?;
    let pic_height_in_map_units = r.read_ue()?.checked_add(1)?;
    let frame_mbs_only = r.read_bit()?;
    if frame_mbs_only == 0 {
        let _mb_adaptive = r.read_bit()?;
    }
    let _direct_8x8 = r.read_bit()?;

    let mut width = pic_width_in_mbs.checked_mul(16)?;
    let mut height = (2 - frame_mbs_only)
        .checked_mul(pic_height_in_map_units)?
        .checked_mul(16)?;

    if r.read_bit()? == 1 {
        // frame_cropping_flag: subtract the cropped border, scaled by the
        // chroma subsampling so the result is the real coded resolution.
        let crop_left = r.read_ue()?;
        let crop_right = r.read_ue()?;
        let crop_top = r.read_ue()?;
        let crop_bottom = r.read_ue()?;
        let (sub_w, sub_h): (u32, u32) = match chroma_format_idc {
            0 => (1, 1), // monochrome
            1 => (2, 2), // 4:2:0
            2 => (2, 1), // 4:2:2
            _ => (1, 1), // 4:4:4
        };
        let crop_unit_x = sub_w;
        let crop_unit_y = sub_h * (2 - frame_mbs_only);
        // checked_add on the crop pairs too: a malformed SPS can carry
        // near-u32::MAX Exp-Golomb values, and a plain `+` would panic in
        // debug / wrap in release, breaking the "never panics" contract.
        width = width.saturating_sub(crop_unit_x.checked_mul(crop_left.checked_add(crop_right)?)?);
        height = height.saturating_sub(crop_unit_y.checked_mul(crop_top.checked_add(crop_bottom)?)?);
    }
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// Best-effort codec of a sequence-header tag, across the framings OBS
/// emits (legacy AVC, Enhanced single-track, Enhanced OneTrack multi-track).
/// Used for the per-destination "1080x1920 · H.264" readout. Returns
/// `Unknown` for anything unrecognised or truncated; never panics.
pub fn seq_header_codec(payload: &[u8]) -> VideoCodec {
    if payload.is_empty() {
        return VideoCodec::Unknown;
    }
    let b0 = payload[0];
    if b0 & 0x80 == 0 {
        // Legacy: low nibble is the CodecID; 7 = AVC is the only one we map.
        return if b0 & 0x0F == 7 {
            VideoCodec::Avc
        } else {
            VideoCodec::Unknown
        };
    }
    // Enhanced-RTMP: OneTrack multi-track carries the FourCC at bytes 2..6
    // (after the mt header), single-track at bytes 1..5.
    let fourcc = if b0 & 0x0F == 6 {
        payload.get(2..6)
    } else {
        payload.get(1..5)
    };
    match fourcc {
        Some(f) if f == FOURCC_AVC1 => VideoCodec::Avc,
        Some(f) if f == FOURCC_HVC1 || f == FOURCC_HEV1 => VideoCodec::Hevc,
        Some(f) if f == FOURCC_AV01 => VideoCodec::Av1,
        Some(f) if f == FOURCC_VP09 => VideoCodec::Vp9,
        _ => VideoCodec::Unknown,
    }
}

/// Pick the vertical-canvas primary track from the per-TrackId seq-header
/// cache. The vertical primary is the portrait track (`height > width`)
/// with the largest pixel area - if Twitch Dual Format emits a vertical
/// ladder, that is its top rung. Returns `None` when no track is portrait
/// (EB off, no vertical canvas, or only non-AVC/unparseable headers), in
/// which case a vertical destination has nothing to send and waits.
pub fn detect_vertical_primary_track(
    seq_headers: &std::collections::BTreeMap<u8, Vec<u8>>,
) -> Option<u8> {
    let mut best: Option<(u8, u32)> = None;
    for (&track_id, header) in seq_headers {
        let Some((w, h)) = sps_dimensions(header) else {
            continue;
        };
        if h <= w {
            continue; // landscape or square - not the vertical canvas
        }
        let area = w.saturating_mul(h);
        let better = match best {
            None => true,
            Some((_, best_area)) => area > best_area,
        };
        if better {
            best = Some((track_id, area));
        }
    }
    best.map(|(track_id, _)| track_id)
}

/// Remove H.264 emulation-prevention bytes (0x00 00 03 -> 0x00 00) so the
/// SPS RBSP can be Exp-Golomb decoded directly.
fn strip_emulation_prevention(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0u32;
    for &b in nal {
        if zeros >= 2 && b == 0x03 {
            zeros = 0;
            continue; // drop the emulation_prevention_three_byte
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// Minimal MSB-first bit reader for SPS parsing. Every read is bounds
/// checked and returns `None` past the end, so a truncated SPS aborts the
/// parse cleanly instead of panicking.
struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.bit_pos / 8)?;
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(((byte >> shift) & 1) as u32)
    }

    fn read_bits(&mut self, n: u32) -> Option<u32> {
        // Cap at 32 so a corrupt length can't loop unboundedly.
        if n > 32 {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Some(v)
    }

    /// Unsigned Exp-Golomb. The leading-zero count is capped at 31 so
    /// malformed input can't spin forever.
    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0u32;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return None;
            }
        }
        if leading_zeros == 0 {
            return Some(0);
        }
        let rest = self.read_bits(leading_zeros)?;
        Some((1u32 << leading_zeros) - 1 + rest)
    }

    /// Signed Exp-Golomb.
    fn read_se(&mut self) -> Option<i32> {
        let k = self.read_ue()?;
        // Map 0,1,2,3,... -> 0,+1,-1,+2,...
        let val = k.div_ceil(2) as i32;
        Some(if k % 2 == 0 { -val } else { val })
    }

    /// Skip a scaling list per H.264 7.3.2.1.1.1 without storing it.
    fn skip_scaling_list(&mut self, size: u32) -> Option<()> {
        let mut last_scale = 8i32;
        let mut next_scale = 8i32;
        for _ in 0..size {
            if next_scale != 0 {
                let delta = self.read_se()?;
                next_scale = (last_scale + delta + 256) % 256;
            }
            last_scale = if next_scale == 0 {
                last_scale
            } else {
                next_scale
            };
        }
        Some(())
    }
}

/// Decide what video bytes to put on the wire for a destination given its
/// [`VideoEgress`] policy:
///
/// - [`VideoEgress::Passthrough`] (Twitch): every tag goes through
///   bit-faithfully so Twitch's edge gets the full EB bundle it negotiated.
/// - [`VideoEgress::Track(n)`]: forward only OneTrack `TrackId == n`
///   (`0` = horizontal primary, `n>0` = the vertical-canvas primary),
///   dropping every other track. An AVC track is rewritten to *legacy* AVC
///   (see [`onetrack_avc_to_legacy`]); non-AVC codecs and ManyTracks
///   bundles fall back to the generic E-RTMP flatten. Single-track / legacy
///   tags pass through for `Track(0)` and are dropped for a vertical target.
///
/// Returns `None` when the tag must be skipped on this destination, a
/// borrow when no copy is needed, and an owned `Vec` only when a rewrite or
/// flatten actually happened.
pub fn select_video_bytes<'a>(
    payload: &'a [u8],
    egress: VideoEgress,
) -> Option<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    let target = match egress {
        VideoEgress::Passthrough => return Some(Cow::Borrowed(payload)),
        VideoEgress::Track(t) => t,
    };
    let info = classify_video_tag(payload);
    if !info.is_multitrack {
        // Single-track / legacy tag. For the horizontal primary
        // (target 0) this IS the stream, so pass it through. For a
        // vertical destination (target > 0) a single-track tag is the
        // horizontal primary OBS also emits for legacy ingests - the
        // vertical canvas must never carry it, so drop it. The vertical
        // canvas always arrives as OneTrack multi-track tags handled
        // below.
        if target == 0 {
            return Some(Cow::Borrowed(payload));
        }
        return None;
    }
    // For OneTrack layout, OBS sends one tag per track per frame.
    // Forwarding every track to a non-Twitch destination would deliver
    // N frames per PTS - decoders read that as a multi-frame storm and
    // either drop the connection (YouTube did, ~12 s per ladder rung)
    // or render heavy artefacts. We keep only the target track (0 for
    // horizontal primary, the vertical-canvas primary otherwise) and
    // drop the rest, which is lossless from the destination's POV.
    let is_onetrack = payload.len() >= 7 && (payload[1] >> 4) & 0x0F == 0;
    if is_onetrack {
        let track_id = payload[6];
        if track_id != target {
            return None;
        }
    } else if target != 0 {
        // ManyTracks / ManyTracksManyCodecs pack every track into one tag;
        // a non-zero target can't be isolated from the bundle, so drop it
        // rather than risk sending the wrong canvas. Twitch Dual Format
        // uses the OneTrack layout handled above, so this only affects
        // exotic encoders, and only for vertical destinations.
        return None;
    }
    // Prefer legacy AVC framing for a OneTrack AVC track. YouTube's vertical
    // ingest (and other secondary ingests) accept legacy AVC reliably but
    // abort Enhanced-RTMP `avc1` on a ~11 s cycle; the horizontal primary
    // already arrives as legacy, so this gives the vertical feed the same
    // framing. Coded frames / seq headers convert; metadata / colour packets
    // return None and are dropped rather than smuggling E-RTMP into an
    // otherwise-legacy stream. Non-AVC codecs and ManyTracks bundles fall
    // through to the generic E-RTMP flatten below.
    if is_onetrack && payload[2..6] == FOURCC_AVC1 {
        return onetrack_avc_to_legacy(payload).map(Cow::Owned);
    }
    match flatten_multitrack_video(payload) {
        Some(flat) => Some(Cow::Owned(flat)),
        // Pathological multi-track layout we couldn't parse - let it
        // through as-is, same fallback the old ingest-side flatten
        // used. The destination will either accept it or surface an
        // error we can chase.
        None => Some(Cow::Borrowed(payload)),
    }
}

/// Decide what audio bytes to put on the wire given the destination's
/// EB policy. Mirrors `select_video_bytes`:
///
/// - `pass_through_multitrack = true`: Twitch destination. Multi-track
///   audio passes through bit-faithfully so the VOD-audio session
///   that Twitch allocated (track 1 separate from live track 0) keeps
///   being fed. Single-track audio also passes through unchanged.
/// - `pass_through_multitrack = false`: every other RTMP ingest. We
///   only forward audio from TrackId 0; TrackId != 0 OneTrack-format
///   multi-track audio tags are dropped, so a YouTube / Kick
///   destination running alongside a Twitch-EB simulcast doesn't
///   choke on a second audio track it doesn't know how to handle.
///   Legacy / single-track audio always passes through.
///
/// Returns `None` when the tag must be skipped on this destination.
pub fn select_audio_bytes<'a>(
    payload: &'a [u8],
    pass_through_multitrack: bool,
) -> Option<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    if pass_through_multitrack {
        return Some(Cow::Borrowed(payload));
    }
    // Pre-checks mirror audio_seq_header_track_id. Anything that isn't
    // an Enhanced-RTMP OneTrack multi-track audio tag passes through
    // unchanged - single-track audio (Enhanced or legacy), MP3, or
    // ManyTracks / ManyTracksManyCodecs layouts.
    if payload.is_empty() {
        return Some(Cow::Borrowed(payload));
    }
    if (payload[0] >> 4) & 0x0F != 9 {
        return Some(Cow::Borrowed(payload));
    }
    // PacketType in byte 0's low nibble; 5 = Multitrack.
    if payload[0] & 0x0F != 5 {
        return Some(Cow::Borrowed(payload));
    }
    if payload.len() < 2 {
        return Some(Cow::Borrowed(payload));
    }
    // OneTrack only (mt_type=0). ManyTracks layouts pack every track
    // into one tag so there is nothing to drop.
    if (payload[1] >> 4) & 0x0F != 0 {
        return Some(Cow::Borrowed(payload));
    }
    if payload.len() < 7 {
        return Some(Cow::Borrowed(payload));
    }
    let track_id = payload[6];
    if track_id != 0 {
        return None;
    }
    Some(Cow::Borrowed(payload))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Mp3,
    Opus,
    Ac3,
    Eac3,
    Flac,
    Unknown,
}

impl AudioCodec {
    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Aac => "AAC",
            AudioCodec::Mp3 => "MP3",
            AudioCodec::Opus => "Opus",
            AudioCodec::Ac3 => "AC-3",
            AudioCodec::Eac3 => "E-AC-3",
            AudioCodec::Flac => "FLAC",
            AudioCodec::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioTagInfo {
    pub is_seq_header: bool,
    pub is_multitrack: bool,
    pub codec: AudioCodec,
}

const FOURCC_OPUS: [u8; 4] = *b"Opus";
const FOURCC_AC3: [u8; 4] = *b"ac-3";
const FOURCC_EAC3: [u8; 4] = *b"ec-3";
const FOURCC_FLAC: [u8; 4] = *b"fLaC";
const FOURCC_MP4A: [u8; 4] = *b"mp4a";

pub fn classify_audio_tag(payload: &[u8]) -> AudioTagInfo {
    let unknown = AudioTagInfo {
        is_seq_header: false,
        is_multitrack: false,
        codec: AudioCodec::Unknown,
    };
    if payload.is_empty() {
        return unknown;
    }
    let sound_format = (payload[0] >> 4) & 0x0F;

    // Legacy AAC: byte 1 is AACPacketType (0=AudioSpecificConfig).
    if sound_format == 10 {
        let is_seq_header = payload.len() >= 2 && payload[1] == 0;
        return AudioTagInfo {
            is_seq_header,
            is_multitrack: false,
            codec: AudioCodec::Aac,
        };
    }
    if sound_format == 2 {
        return AudioTagInfo {
            codec: AudioCodec::Mp3,
            ..unknown
        };
    }
    // Enhanced RTMP audio (ExAudioTagHeader) uses sound_format=9 as the
    // sentinel. The PacketType lives in the SAME byte's low nibble -
    // NOT byte 1 as our v0.1.0..0.1.3 wrote it. See OBS's
    // flv_packet_audio_ex in plugins/obs-outputs/flv-mux.c:
    //
    //   s_w8(&s, AUDIO_HEADER_EX | (is_multitrack
    //              ? AUDIO_PACKETTYPE_MULTITRACK
    //              : type));
    //   if (is_multitrack) {
    //       s_w8(&s, MULTITRACKTYPE_ONE_TRACK | type);
    //       s_wa4cc(&s, codec_id);
    //       s_w8(&s, (uint8_t)idx);
    //   } else {
    //       s_wa4cc(&s, codec_id);
    //   }
    //
    // Single-track layout: byte0 = 0x9X (X = packet type),
    //                      bytes 1..5 = FourCC, payload from byte 5.
    // Multi-track  layout: byte0 = 0x95 (Multitrack packet type),
    //                      byte1 = MultiTrackType | nested packet type,
    //                      bytes 2..6 = FourCC,
    //                      byte 6 = TrackId, payload from byte 7.
    if sound_format == 9 {
        let packet_type = payload[0] & 0x0F;
        let is_multitrack = packet_type == 5;
        // For multi-track the SeqStart marker is the *nested* packet
        // type in byte 1's low nibble - the outer packet type is 5
        // (Multitrack) regardless of whether this is a config or a
        // coded frame.
        let is_seq_header = if is_multitrack {
            payload.len() >= 2 && (payload[1] & 0x0F) == 0
        } else {
            packet_type == 0
        };
        // FourCC sits at bytes 1..5 for single-track Enhanced-RTMP,
        // and at bytes 2..6 for multi-track OneTrack / ManyTracks
        // layouts (one extra byte for the multitrack header itself).
        // ManyTracksManyCodecs packs FourCC per-track inside the body,
        // so we report Unknown at the outer level - good enough for
        // dashboard chip display.
        let codec = if !is_multitrack && payload.len() >= 5 {
            fourcc_to_codec([payload[1], payload[2], payload[3], payload[4]])
        } else if is_multitrack && payload.len() >= 6 {
            let mt_type = (payload[1] >> 4) & 0x0F;
            if mt_type <= 1 {
                fourcc_to_codec([payload[2], payload[3], payload[4], payload[5]])
            } else {
                AudioCodec::Unknown
            }
        } else {
            AudioCodec::Unknown
        };
        return AudioTagInfo {
            is_seq_header,
            is_multitrack,
            codec,
        };
    }
    unknown
}

fn fourcc_to_codec(fourcc: [u8; 4]) -> AudioCodec {
    match fourcc {
        FOURCC_OPUS => AudioCodec::Opus,
        FOURCC_AC3 => AudioCodec::Ac3,
        FOURCC_EAC3 => AudioCodec::Eac3,
        FOURCC_FLAC => AudioCodec::Flac,
        FOURCC_MP4A => AudioCodec::Aac,
        _ => AudioCodec::Unknown,
    }
}

/// Best-effort extraction of the Enhanced-RTMP audio TrackId for the
/// OneTrack multi-track layout. Mirrors `seq_header_track_id` (the
/// video equivalent), shifted by one byte because audio's multitrack
/// header lives at byte 2 vs video's byte 1.
///
/// Returns 0 for:
///   * legacy AAC / MP3 (sound_format != 9)
///   * Enhanced-RTMP single-track payloads (PacketType != Multitrack)
///   * ManyTracks / ManyTracksManyCodecs layouts (one tag holds every
///     track's data, so the single-slot key 0 captures the whole config)
///   * Truncated payloads we can't parse safely
pub fn audio_seq_header_track_id(payload: &[u8]) -> u8 {
    if payload.is_empty() {
        return 0;
    }
    // sound_format=9 sentinel
    if (payload[0] >> 4) & 0x0F != 9 {
        return 0;
    }
    // PacketType=Multitrack lives in byte 0's low nibble (NOT byte 1's
    // low nibble - that was the v0.1.0..0.1.3 wire-format mis-read).
    if payload[0] & 0x0F != 5 {
        return 0;
    }
    if payload.len() < 2 {
        return 0;
    }
    // MultiTrackType is byte 1's high nibble. Only OneTrack (=0) has a
    // per-tag id; ManyTracks/ManyTracksManyCodecs layouts pack every
    // track into one tag.
    let mt_type = (payload[1] >> 4) & 0x0F;
    if mt_type != 0 {
        return 0;
    }
    // OneTrack layout (verified against OBS's flv-mux.c):
    //   byte 0: 0x95 (SoundFormat=9 | PacketType=Multitrack)
    //   byte 1: MultiTrackType=0 | NestedPacketType
    //   bytes 2..6: FourCC
    //   byte 6: TrackId   <- here, NOT byte 7
    //   bytes 7..: payload
    if payload.len() < 7 {
        return 0;
    }
    payload[6]
}

/// Back-compat shim - sink.rs and a few other callers used this name
/// before the audio classifier existed. Kept so the call site stays terse.
pub fn is_aac_seq_header(payload: &[u8]) -> bool {
    classify_audio_tag(payload).is_seq_header
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a length-prefixed NAL unit (the format AVC video tags use
    /// after the 5-byte FLV/AVC header). `nal_type` is the 5-bit type
    /// stored in the low bits of byte 0.
    fn nal(nal_type: u8, body_len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + body_len);
        let len = (1 + body_len) as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.push(nal_type & 0x1F); // forbidden_zero=0, nal_ref_idc=0 - fine for the type check
        out.resize(out.len() + body_len, 0);
        out
    }

    /// Builds an AVC video tag: FLV header (5 bytes) + NAL units.
    /// frame_type: 1 = keyframe (high nibble); codec_id 7 (AVC, low nibble).
    fn avc_tag(packet_type: u8, nals: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0x17, packet_type, 0, 0, 0]; // FrameType=1, CodecID=7
        for n in nals {
            out.extend_from_slice(n);
        }
        out
    }

    #[test]
    fn avc_idr_detected_from_nal_walk() {
        // packet_type 1 = NALU; contains NAL type 5 (IDR slice)
        let tag = avc_tag(1, &[nal(5, 100)]);
        let info = classify_video_tag(&tag);
        assert!(info.is_idr, "single IDR NAL must be detected");
        assert!(!info.is_seq_header);
        assert_eq!(info.codec, VideoCodec::Avc);
    }

    #[test]
    fn avc_non_idr_not_flagged() {
        // NAL type 1 = non-IDR slice; encoder MIGHT lie via the FrameType
        // nibble - we trust the NAL walk and refuse to call it an IDR.
        let tag = avc_tag(1, &[nal(1, 50)]);
        let info = classify_video_tag(&tag);
        assert!(!info.is_idr);
    }

    #[test]
    fn avc_seq_header_flagged_not_idr() {
        // packet_type 0 = AVCDecoderConfigurationRecord (SPS/PPS)
        let tag = avc_tag(0, &[]);
        let info = classify_video_tag(&tag);
        assert!(info.is_seq_header);
        assert!(
            !info.is_idr,
            "seq headers carry SPS/PPS, never an IDR slice"
        );
    }

    #[test]
    fn empty_payload_returns_unknown() {
        let info = classify_video_tag(&[]);
        assert!(!info.is_idr);
        assert!(!info.is_seq_header);
        assert_eq!(info.codec, VideoCodec::Unknown);
    }

    #[test]
    fn truncated_nal_does_not_panic_or_falsely_signal_idr() {
        // Length prefix says 100 bytes follow, but only 4 are present.
        // The walk must return cleanly (no IDR found) without panicking.
        let truncated = avc_tag(1, &[vec![0x00, 0x00, 0x00, 100, 0x05]]);
        let info = classify_video_tag(&truncated);
        assert!(!info.is_idr);
    }

    #[test]
    fn enhanced_rtmp_keyframe_detected() {
        // Enhanced RTMP: byte0 = IsEx(1) | FrameType(3, keyframe=1) | PacketType(4, CodedFrames=1)
        let byte0 = 0x80 | (1u8 << 4) | 1;
        let mut payload = vec![byte0];
        payload.extend_from_slice(b"hvc1");
        payload.extend_from_slice(&[0; 16]);
        let info = classify_video_tag(&payload);
        assert!(info.is_idr);
        assert_eq!(info.codec, VideoCodec::Hevc);
    }

    #[test]
    fn enhanced_rtmp_seq_header_not_idr() {
        // PacketType=0 (SequenceStart) - must not be flagged as IDR
        let byte0 = 0x80 | (1u8 << 4);
        let mut payload = vec![byte0];
        payload.extend_from_slice(b"av01");
        let info = classify_video_tag(&payload);
        assert!(info.is_seq_header);
        assert!(!info.is_idr);
        assert_eq!(info.codec, VideoCodec::Av1);
    }

    #[test]
    fn aac_seq_header_detected() {
        // sound_format 10 (AAC), packet_type 0 (AudioSpecificConfig)
        let payload = [0xA0, 0x00, 0x12, 0x10];
        let info = classify_audio_tag(&payload);
        assert!(info.is_seq_header);
        assert_eq!(info.codec, AudioCodec::Aac);
        assert!(is_aac_seq_header(&payload));
    }

    #[test]
    fn enhanced_rtmp_opus_audio_recognised() {
        // Enhanced-RTMP single-track: byte 0 = SoundFormat<<4 | PacketType.
        // Here: 9 << 4 | 1 (CodedFrames) = 0x91, followed by FourCC.
        let mut payload = vec![0x91];
        payload.extend_from_slice(b"Opus");
        let info = classify_audio_tag(&payload);
        assert_eq!(info.codec, AudioCodec::Opus);
        assert!(!info.is_seq_header);
    }

    // ── Additional edge cases ──────────────────────────────────────

    #[test]
    fn avc_multi_nal_payload_detects_idr_anywhere() {
        // A tag with [SEI, non-IDR slice, IDR slice]. The walk must
        // keep going past the first non-IDR and still find the IDR.
        let tag = avc_tag(1, &[nal(6, 20), nal(1, 30), nal(5, 50)]);
        let info = classify_video_tag(&tag);
        assert!(info.is_idr, "IDR in 3rd NAL must be detected");
    }

    #[test]
    fn enhanced_rtmp_metadata_packettype_4_not_idr() {
        // packet_type 4 = Metadata (mid-stream HDR colorInfo etc.).
        // Even if the encoder sets FrameType=1, this packet carries no
        // slice data and must not be treated as an IDR cut point.
        let byte0 = 0x80 | (1u8 << 4) | 4;
        let mut payload = vec![byte0];
        payload.extend_from_slice(b"hvc1");
        payload.extend_from_slice(&[0; 8]);
        let info = classify_video_tag(&payload);
        assert!(info.is_metadata);
        assert!(!info.is_idr, "Metadata packets must not be IDR cuts");
        assert!(!info.is_seq_header);
        assert_eq!(info.codec, VideoCodec::Hevc);
    }

    #[test]
    fn enhanced_rtmp_multitrack_codedframes_is_idr() {
        // outer packet_type=6 (Multitrack), nested packet_type=1
        // (CodedFrames). FrameType=1 → real keyframe.
        let byte0 = 0x80 | (1u8 << 4) | 6;
        let byte1 = 0x01; // nested CodedFrames
        let payload = vec![byte0, byte1, 0, 0, 0, 0];
        let info = classify_video_tag(&payload);
        assert!(info.is_multitrack);
        assert!(info.is_idr);
        assert!(!info.is_seq_header);
    }

    #[test]
    fn enhanced_rtmp_multitrack_seqstart_caches_as_seq_header() {
        // outer packet_type=6 (Multitrack), nested packet_type=0
        // (SequenceStart). This is the decoder-config tag for an
        // Enhanced Broadcasting stream - MUST be flagged as seq
        // header or the controller never caches it and every cut
        // emits a stale or empty config to the destination, which
        // then fails to decode the post-cut frames.
        let byte0 = 0x80 | (1u8 << 4) | 6; // FrameType=1, packet_type=Multitrack
        let byte1 = 0x00; // nested SequenceStart
        let payload = vec![byte0, byte1, 0, 0, 0, 0];
        let info = classify_video_tag(&payload);
        assert!(info.is_multitrack);
        assert!(info.is_seq_header);
        // FrameType=1 set on a seq header tag must NOT be flagged
        // as IDR - seq headers carry decoder config, never slice
        // data. Same rule as the single-track classifier.
        assert!(!info.is_idr);
    }

    #[test]
    fn enhanced_aac_seq_header_via_packet_type_0() {
        // Enhanced-RTMP single-track AAC seq header: byte 0 = 0x90
        // (SoundFormat=9, AudioPacketType=0=SequenceStart), followed
        // by FourCC. This is the exact tag OBS emits for the live
        // track when VOD audio is enabled (live = idx=0, single-track
        // because not multitrack).
        let mut payload = vec![0x90];
        payload.extend_from_slice(b"mp4a");
        let info = classify_audio_tag(&payload);
        assert!(info.is_seq_header);
        assert_eq!(info.codec, AudioCodec::Aac);
        // The convenience shim agrees.
        assert!(is_aac_seq_header(&payload));
    }

    // ── select_video_bytes: per-destination flatten policy ───────────
    //
    // The pump uses this helper to decide what bytes to put on the wire
    // for a given destination. Three invariants must hold absolutely or
    // beta.7's "Enhanced Broadcasting to Twitch + flatten to everyone
    // else" promise breaks silently.

    fn avc_single_track_keyframe_bytes() -> Vec<u8> {
        // Realistic FLV AVC keyframe: FrameType=1, CodecID=7,
        // AVCPacketType=1 (NALU), 3-byte CompositionTime=0, then a
        // length-prefixed IDR NAL. Mirrors what any single-track OBS
        // encoder emits per frame.
        let nal = nal(5, 64);
        let mut tag = vec![0x17, 0x01, 0x00, 0x00, 0x00];
        tag.extend_from_slice(&nal);
        tag
    }

    fn enhanced_rtmp_onetrack_video_bytes_track(track_id: u8) -> Vec<u8> {
        // Enhanced-RTMP multi-track video, layout type 0 (OneTrack):
        //   byte 0 = IsEx(1) | FrameType(3, keyframe=1) | PacketType(4, Multitrack=6)
        //   byte 1 = MultiTrackType(4, OneTrack=0) | nested PacketType(4, CodedFrames=1)
        //   bytes 2..6 = FourCC ("hvc1")
        //   byte 6 = TrackId
        //   bytes 7.. = inner payload
        let mut payload = vec![0x80 | (1u8 << 4) | 6, 0x01];
        payload.extend_from_slice(b"hvc1");
        payload.push(track_id);
        payload.extend_from_slice(&[0xAA; 32]);
        payload
    }

    #[test]
    fn select_video_bytes_passes_single_track_through_unchanged_in_both_modes() {
        // Invariant 1: single-track tags never get rewritten. The
        // pass_through flag is meaningless when there's nothing to
        // flatten - and the borrow path means no copy happens either.
        let tag = avc_single_track_keyframe_bytes();
        let twitch = select_video_bytes(&tag, VideoEgress::Passthrough)
            .expect("single-track always forwards");
        let youtube =
            select_video_bytes(&tag, VideoEgress::Track(0)).expect("single-track always forwards");
        assert_eq!(twitch.as_ref(), tag.as_slice());
        assert_eq!(youtube.as_ref(), tag.as_slice());
        assert!(matches!(twitch, std::borrow::Cow::Borrowed(_)));
        assert!(matches!(youtube, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn select_video_bytes_passes_multitrack_through_for_twitch() {
        // Invariant 2: Twitch destinations get the raw multi-track
        // bytes verbatim - that's what unlocks the transcoded ladder.
        // True for every TrackId, not just the primary.
        for track in 0u8..=4 {
            let tag = enhanced_rtmp_onetrack_video_bytes_track(track);
            let twitch = select_video_bytes(&tag, VideoEgress::Passthrough)
                .unwrap_or_else(|| panic!("twitch must forward track {track}"));
            assert_eq!(twitch.as_ref(), tag.as_slice());
            assert!(matches!(twitch, std::borrow::Cow::Borrowed(_)));
        }
    }

    #[test]
    fn select_video_bytes_flattens_track0_multitrack_for_non_twitch() {
        // Defensive path: an encoder that sends the *primary* as
        // OneTrack-format TrackId 0 (rather than as a separate legacy
        // AVC tag) still gets a flattened single-track tag for
        // non-Twitch destinations. In the wild OBS sends the primary
        // as legacy, so this path rarely fires - but we want a future
        // encoder change not to silently blank YouTube.
        let tag = enhanced_rtmp_onetrack_video_bytes_track(0);
        let youtube =
            select_video_bytes(&tag, VideoEgress::Track(0)).expect("track 0 must be forwarded");
        let direct_flat = flatten_multitrack_video(&tag).expect("OneTrack layout must flatten");
        assert_eq!(youtube.as_ref(), direct_flat.as_slice());
        assert!(matches!(youtube, std::borrow::Cow::Owned(_)));
    }

    #[test]
    fn select_video_bytes_drops_nonzero_track_multitrack_for_non_twitch() {
        // The core fix: OBS sends the simulcast ladder as OneTrack
        // tags with TrackId 1..N, one tag per resolution per frame.
        // Forwarding every one to YouTube delivered N frames per PTS
        // → decoder storm → reconnect cascade. These tags must be
        // dropped; the primary arrives separately as a legacy /
        // Enhanced single-track tag.
        for track in 1u8..=4 {
            let tag = enhanced_rtmp_onetrack_video_bytes_track(track);
            assert!(
                select_video_bytes(&tag, VideoEgress::Track(0)).is_none(),
                "TrackId {track} must be dropped for non-Twitch",
            );
        }
    }

    // ── Twitch Dual Format (dual canvas) forwarding ──────────────────
    //
    // Dual Format = a horizontal (canvas 0) and a vertical (canvas 1)
    // layout sent together over the one Enhanced Broadcasting RTMP
    // connection. `canvas_index` lives only in the GetClientConfiguration
    // JSON; on the wire the canvases are just more TrackIds. So the
    // existing per-track passthrough already carries the vertical canvas:
    // these tests pin that down explicitly with dual-format framing.

    #[test]
    fn select_video_bytes_forwards_dual_canvas_to_twitch() {
        // Horizontal primary (TrackId 0) and a vertical-canvas rung
        // (TrackId 1) both reach Twitch byte-for-byte. Nothing about a
        // second canvas changes the Twitch passthrough contract.
        let horizontal = enhanced_rtmp_onetrack_video_bytes_track(0);
        let vertical = enhanced_rtmp_onetrack_video_bytes_track(1);
        for tag in [&horizontal, &vertical] {
            let twitch = select_video_bytes(tag, VideoEgress::Passthrough)
                .expect("twitch must forward both canvases");
            assert_eq!(twitch.as_ref(), tag.as_slice());
            assert!(matches!(twitch, std::borrow::Cow::Borrowed(_)));
        }
    }

    #[test]
    fn select_video_bytes_keeps_horizontal_drops_vertical_for_non_twitch() {
        // A non-Twitch destination (YouTube/Kick) running alongside a
        // Dual Format Twitch session keeps the horizontal primary and
        // drops the vertical canvas - it has no concept of a second
        // canvas, and TrackId != 0 is exactly the vertical rungs.
        let horizontal = enhanced_rtmp_onetrack_video_bytes_track(0);
        let vertical = enhanced_rtmp_onetrack_video_bytes_track(1);
        assert!(
            select_video_bytes(&horizontal, VideoEgress::Track(0)).is_some(),
            "horizontal canvas (TrackId 0) must reach non-Twitch"
        );
        assert!(
            select_video_bytes(&vertical, VideoEgress::Track(0)).is_none(),
            "vertical canvas (TrackId 1) must be dropped for non-Twitch"
        );
    }

    // ── Vertical-canvas selection: SPS orientation + Track(n) routing ──
    //
    // A vertical destination forwards ONLY the portrait canvas's track,
    // flattened to single-track, and drops the horizontal primary (both
    // its legacy single-track tags and its OneTrack ladder rungs).

    /// Minimal MSB-first bit writer for building synthetic SPS test
    /// vectors. Mirrors `BitReader` so the parse round-trips.
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u8,
        nbits: u8,
    }
    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn put_bit(&mut self, b: u32) {
            self.cur = (self.cur << 1) | (b as u8 & 1);
            self.nbits += 1;
            if self.nbits == 8 {
                self.bytes.push(self.cur);
                self.cur = 0;
                self.nbits = 0;
            }
        }
        fn put_bits(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.put_bit((v >> i) & 1);
            }
        }
        fn put_ue(&mut self, v: u32) {
            let code = v + 1;
            let bits = 32 - code.leading_zeros();
            for _ in 0..(bits - 1) {
                self.put_bit(0);
            }
            self.put_bits(code, bits);
        }
        fn finish(mut self) -> Vec<u8> {
            self.put_bit(1); // rbsp_stop_one_bit
            while self.nbits != 0 {
                self.put_bit(0);
            }
            self.bytes
        }
    }

    /// Baseline-profile SPS RBSP (no high-profile block, frame_mbs_only,
    /// no cropping) sized to `w_mb` x `h_mb` macroblocks.
    fn build_sps_rbsp(w_mb: u32, h_mb: u32) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put_bits(66, 8); // profile_idc = baseline
        w.put_bits(0, 8); // constraint flags + reserved
        w.put_bits(30, 8); // level_idc
        w.put_ue(0); // seq_parameter_set_id
        w.put_ue(0); // log2_max_frame_num_minus4
        w.put_ue(0); // pic_order_cnt_type
        w.put_ue(0); // log2_max_pic_order_cnt_lsb_minus4
        w.put_ue(0); // max_num_ref_frames
        w.put_bit(0); // gaps_in_frame_num_value_allowed_flag
        w.put_ue(w_mb - 1); // pic_width_in_mbs_minus1
        w.put_ue(h_mb - 1); // pic_height_in_map_units_minus1
        w.put_bit(1); // frame_mbs_only_flag
        w.put_bit(0); // direct_8x8_inference_flag
        w.put_bit(0); // frame_cropping_flag
        w.put_bit(0); // vui_parameters_present_flag
        w.finish()
    }

    fn build_avc_config(w_mb: u32, h_mb: u32) -> Vec<u8> {
        let mut sps = vec![0x67u8]; // NAL header: type 7 (SPS)
        sps.extend(build_sps_rbsp(w_mb, h_mb));
        let mut cfg = vec![1u8, 66, 0, 30, 0xFF, 0xE1]; // version..numOfSPS(1)
        cfg.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        cfg.extend_from_slice(&sps);
        cfg.push(0); // numOfPictureParameterSets = 0
        cfg
    }

    /// Legacy AVC sequence-header tag carrying the given dimensions.
    fn legacy_avc_seq_header(w_mb: u32, h_mb: u32) -> Vec<u8> {
        let mut t = vec![0x17u8, 0x00, 0, 0, 0]; // AVC seq header, CT=0
        t.extend(build_avc_config(w_mb, h_mb));
        t
    }

    /// OneTrack Enhanced-RTMP sequence-header tag (avc1) for `track_id`.
    fn onetrack_avc_seq_header(track_id: u8, w_mb: u32, h_mb: u32) -> Vec<u8> {
        let mut t = vec![0x96u8, 0x00]; // IsEx|FrameType1|Multitrack, OneTrack|SeqStart
        t.extend_from_slice(b"avc1");
        t.push(track_id);
        t.extend(build_avc_config(w_mb, h_mb));
        t
    }

    #[test]
    fn sps_dimensions_decodes_landscape_and_portrait() {
        // 40x30 MB = 640x480 landscape; 30x40 MB = 480x640 portrait.
        assert_eq!(
            sps_dimensions(&legacy_avc_seq_header(40, 30)),
            Some((640, 480))
        );
        assert_eq!(
            sps_dimensions(&legacy_avc_seq_header(30, 40)),
            Some((480, 640))
        );
        // Same dims survive the OneTrack Enhanced-RTMP framing.
        assert_eq!(
            sps_dimensions(&onetrack_avc_seq_header(2, 30, 40)),
            Some((480, 640))
        );
    }

    #[test]
    fn sps_dimensions_returns_none_on_unparseable_input() {
        // Empty, truncated, and non-AVC inputs must all fail cleanly with
        // no panic (this runs on hostile wire bytes).
        assert_eq!(sps_dimensions(&[]), None);
        assert_eq!(sps_dimensions(&[0x17, 0x00]), None);
        let mut truncated = legacy_avc_seq_header(40, 30);
        truncated.truncate(9); // cut into the SPS
        assert_eq!(sps_dimensions(&truncated), None);
        // HEVC single-track seq header (hvc1) is not AVC - unparseable.
        let mut hevc = vec![0x90u8];
        hevc.extend_from_slice(b"hvc1");
        hevc.extend_from_slice(&[0u8; 16]);
        assert_eq!(sps_dimensions(&hevc), None);
    }

    #[test]
    fn sps_dimensions_huge_crop_returns_none_not_panic() {
        // A malformed SPS can carry near-u32::MAX frame-crop Exp-Golomb
        // values; the crop math must stay checked so this returns None
        // instead of overflow-panicking in debug builds.
        let mut w = BitWriter::new();
        w.put_bits(66, 8); // baseline profile (skips the high-profile block)
        w.put_bits(0, 8);
        w.put_bits(30, 8);
        w.put_ue(0); // sps_id
        w.put_ue(0); // log2_max_frame_num_minus4
        w.put_ue(0); // pic_order_cnt_type
        w.put_ue(0); // log2_max_pic_order_cnt_lsb_minus4
        w.put_ue(0); // max_num_ref_frames
        w.put_bit(0); // gaps_in_frame_num_value_allowed_flag
        w.put_ue(39); // pic_width_in_mbs_minus1
        w.put_ue(21); // pic_height_in_map_units_minus1
        w.put_bit(1); // frame_mbs_only_flag
        w.put_bit(0); // direct_8x8_inference_flag
        w.put_bit(1); // frame_cropping_flag
        w.put_ue(4_000_000_000); // crop_left
        w.put_ue(4_000_000_000); // crop_right -> left + right overflows u32
        w.put_ue(0);
        w.put_ue(0);
        w.put_bit(0); // vui_parameters_present_flag
        let mut sps = vec![0x67u8];
        sps.extend(w.finish());
        let mut cfg = vec![1u8, 66, 0, 30, 0xFF, 0xE1];
        cfg.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        cfg.extend_from_slice(&sps);
        cfg.push(0);
        let mut tag = vec![0x17u8, 0x00, 0, 0, 0];
        tag.extend(cfg);
        assert_eq!(sps_dimensions(&tag), None);
    }

    #[test]
    fn sps_and_selection_never_panic_on_fuzz() {
        // Deterministic pseudo-random byte blobs, wrapped as AVC seq
        // headers, pushed through the parser + selector. These run on
        // hostile wire input, so the invariant is simply: never panic.
        let mut seed: u32 = 0x9e3779b9;
        for _ in 0..3000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (seed as usize % 48) + 1;
            let mut blob = Vec::with_capacity(n);
            let mut x = seed;
            for _ in 0..n {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                blob.push((x >> 16) as u8);
            }
            // Legacy AVC framing so the bytes reach the SPS Exp-Golomb path.
            let mut tag = vec![0x17u8, 0x00, 0, 0, 0];
            tag.extend_from_slice(&blob);
            let _ = sps_dimensions(&tag);
            let _ = select_video_bytes(&tag, VideoEgress::Track((seed & 0xFF) as u8));
            let mut m = std::collections::BTreeMap::new();
            m.insert((seed & 0x7) as u8, tag);
            let _ = detect_vertical_primary_track(&m);
        }
    }

    #[test]
    fn seq_header_codec_detects_framings() {
        assert_eq!(seq_header_codec(&legacy_avc_seq_header(40, 30)), VideoCodec::Avc);
        assert_eq!(seq_header_codec(&onetrack_avc_seq_header(1, 30, 40)), VideoCodec::Avc);
        // Enhanced single-track HEVC seq header (0x90 = IsEx|key|SequenceStart).
        let mut hevc = vec![0x90u8];
        hevc.extend_from_slice(b"hvc1");
        assert_eq!(seq_header_codec(&hevc), VideoCodec::Hevc);
        assert_eq!(seq_header_codec(&[]), VideoCodec::Unknown);
    }

    #[test]
    fn detect_vertical_primary_track_picks_largest_portrait() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert(0u8, legacy_avc_seq_header(120, 68)); // 1920x1088 landscape
        headers.insert(3u8, onetrack_avc_seq_header(3, 34, 60)); // 544x960 portrait
        headers.insert(4u8, onetrack_avc_seq_header(4, 68, 120)); // 1088x1920 portrait (bigger)
        assert_eq!(detect_vertical_primary_track(&headers), Some(4));
    }

    #[test]
    fn detect_vertical_primary_track_none_when_all_landscape() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert(0u8, legacy_avc_seq_header(120, 68));
        headers.insert(1u8, onetrack_avc_seq_header(1, 80, 45));
        assert_eq!(detect_vertical_primary_track(&headers), None);
    }

    #[test]
    fn select_video_bytes_vertical_forwards_only_target_track() {
        // Track(1) keeps OneTrack TrackId 1 (flattened) and drops every
        // other track plus the legacy horizontal primary.
        let track1 = enhanced_rtmp_onetrack_video_bytes_track(1);
        let track2 = enhanced_rtmp_onetrack_video_bytes_track(2);
        let legacy_horizontal = avc_single_track_keyframe_bytes();

        let kept = select_video_bytes(&track1, VideoEgress::Track(1))
            .expect("vertical target track must forward");
        let flat = flatten_multitrack_video(&track1).expect("OneTrack flattens");
        assert_eq!(kept.as_ref(), flat.as_slice());

        assert!(
            select_video_bytes(&track2, VideoEgress::Track(1)).is_none(),
            "non-target OneTrack rung must be dropped for vertical"
        );
        assert!(
            select_video_bytes(&legacy_horizontal, VideoEgress::Track(1)).is_none(),
            "legacy horizontal primary must be dropped for a vertical destination"
        );
    }

    #[test]
    fn select_video_bytes_vertical_avc_rewrites_to_legacy() {
        // A OneTrack avc1 vertical track must reach a non-Twitch dest as
        // LEGACY AVC (0x17/0x27 ...), not Enhanced-RTMP - YouTube's vertical
        // ingest is flaky with E-RTMP avc1 and aborts on a ~11 s cycle.

        // Sequence header -> legacy AVC seq header, config record intact.
        let seq = onetrack_avc_seq_header(1, 30, 40);
        let out = select_video_bytes(&seq, VideoEgress::Track(1))
            .expect("vertical avc seq header forwards");
        assert_eq!(out[0], 0x17, "legacy AVC seq-header frame byte");
        assert_eq!(out[1], 0x00, "AVCPacketType: sequence header");
        assert_eq!(&out[2..5], &[0, 0, 0], "composition time zero");
        assert_eq!(&out[5..], &build_avc_config(30, 40)[..]);

        // CodedFramesX keyframe (no composition time) -> legacy 0x17 NALU.
        let mut kf = vec![0x96u8, 0x03]; // key|Multitrack, OneTrack|CodedFramesX
        kf.extend_from_slice(b"avc1");
        kf.push(1);
        kf.extend_from_slice(&[0, 0, 0, 5, 0x65, 1, 2, 3, 4]); // len-prefixed IDR
        let out = select_video_bytes(&kf, VideoEgress::Track(1)).expect("keyframe forwards");
        assert_eq!(out[0], 0x17, "legacy keyframe head");
        assert_eq!(out[1], 0x01, "AVCPacketType: NALU");
        assert_eq!(&out[2..5], &[0, 0, 0], "CodedFramesX carries composition time 0");
        assert_eq!(&out[5..], &[0, 0, 0, 5, 0x65, 1, 2, 3, 4]);

        // CodedFrames inter frame preserves the 3-byte composition time.
        let mut inter = vec![0xA6u8, 0x01]; // inter|Multitrack, OneTrack|CodedFrames
        inter.extend_from_slice(b"avc1");
        inter.push(1);
        inter.extend_from_slice(&[0x00, 0x00, 0x10]); // composition time
        inter.extend_from_slice(&[0, 0, 0, 3, 0x41, 9, 9]); // len-prefixed P NAL
        let out = select_video_bytes(&inter, VideoEgress::Track(1)).expect("inter forwards");
        assert_eq!(out[0], 0x27, "legacy inter head");
        assert_eq!(out[1], 0x01);
        assert_eq!(&out[2..5], &[0x00, 0x00, 0x10], "composition time preserved");
        assert_eq!(&out[5..], &[0, 0, 0, 3, 0x41, 9, 9]);

        // A OneTrack avc1 metadata/colour packet has no legacy equivalent
        // and is dropped rather than mixing E-RTMP into a legacy stream.
        let mut meta = vec![0x96u8, 0x04]; // OneTrack | Metadata(4)
        meta.extend_from_slice(b"avc1");
        meta.push(1);
        meta.extend_from_slice(&[1, 2, 3, 4]);
        assert!(
            select_video_bytes(&meta, VideoEgress::Track(1)).is_none(),
            "avc1 metadata packet must be dropped for a legacy vertical stream"
        );
    }

    // ── is_primary_video_idr: cut-target gate for EB ladder cuts ──
    //
    // Regression test set for the v0.1.3 EB pixel-glitch fix. Before
    // this gate, the ring's IDR-only secondary index recorded every
    // track's keyframes equally, so `compute_delay_cut` could land on
    // a TrackId-1 IDR while the legacy primary (track 0) was mid-GOP.
    // After: only primary-track IDRs are candidates, the legacy
    // decoder always has its anchor at the cut boundary.

    #[test]
    fn is_primary_video_idr_accepts_legacy_avc_keyframe() {
        let tag = avc_single_track_keyframe_bytes();
        assert!(is_primary_video_idr(&tag));
    }

    #[test]
    fn is_primary_video_idr_rejects_legacy_avc_inter_frame() {
        // FrameType=2 (inter) + CodecID=7 (AVC) = 0x27. Same shape as
        // the keyframe builder above, just with the leading byte flipped.
        let mut tag = avc_single_track_keyframe_bytes();
        tag[0] = 0x27;
        assert!(!is_primary_video_idr(&tag));
    }

    #[test]
    fn is_primary_video_idr_accepts_onetrack_multitrack_track_0() {
        // EB ladder primary rung at TrackId=0 - the cut anchor we want.
        let tag = enhanced_rtmp_onetrack_video_bytes_track(0);
        assert!(is_primary_video_idr(&tag));
    }

    #[test]
    fn is_primary_video_idr_rejects_onetrack_multitrack_nonzero_tracks() {
        // EB ladder rungs 1..=4 each carry their own IDR cadence but
        // must NOT be picked as a cut anchor - if the cut lands here
        // the legacy decoder on the destination has no reference.
        for track in 1u8..=4 {
            let tag = enhanced_rtmp_onetrack_video_bytes_track(track);
            assert!(
                !is_primary_video_idr(&tag),
                "TrackId {track} must not be a cut candidate"
            );
        }
    }

    #[test]
    fn is_primary_video_idr_anchors_dual_format_cuts_to_horizontal_canvas() {
        // Dual Format: the cut-point index is built only from the
        // horizontal primary (TrackId 0); the vertical canvas's primary
        // (a nonzero TrackId) is NOT a cut anchor. That means a Cut lands
        // cleanly on the vertical canvas ONLY IF OBS aligned both
        // canvases' keyframes to the same input timestamp - which it does
        // when both encoders share keyint, but that is an OBS behaviour we
        // can't prove from inside this crate (it needs one real capture).
        // This test pins our half of the contract: cuts are anchored to
        // the horizontal canvas. If a future change must make cuts
        // canvas-aware, this is the regression guard that has to flip.
        let horizontal_idr = enhanced_rtmp_onetrack_video_bytes_track(0);
        let vertical_idr = enhanced_rtmp_onetrack_video_bytes_track(1);
        assert!(
            is_primary_video_idr(&horizontal_idr),
            "horizontal canvas IDR must be a cut anchor"
        );
        assert!(
            !is_primary_video_idr(&vertical_idr),
            "vertical canvas IDR must not (yet) be an independent cut anchor"
        );
    }

    #[test]
    fn is_primary_video_idr_rejects_enhanced_p_frame() {
        // Enhanced-RTMP single-track inter-frame: IsEx | FrameType=2 |
        // PacketType=CodedFrames(1) -> bits assembled as 0xA1.
        let tag = vec![0x80 | (2u8 << 4) | 1, b'a', b'v', b'c', b'1', 0xAA, 0xBB];
        assert!(!is_primary_video_idr(&tag));
    }

    #[test]
    fn is_primary_video_idr_handles_truncated_multitrack_gracefully() {
        // Multitrack header declared but payload too short to read the
        // TrackId byte - safer to refuse cut-candidacy than to land on
        // an unknown track.
        let truncated = vec![0x80 | (1u8 << 4) | 6, 0x01, b'a', b'v', b'c', b'1'];
        assert!(!is_primary_video_idr(&truncated));
    }

    #[test]
    fn select_video_bytes_falls_back_to_raw_when_flatten_cannot_parse() {
        // Pathological multi-track tag (declares multi-track but the
        // payload is too short to contain a valid inner structure).
        // The pump must NOT crash and must NOT swallow the tag -
        // letting it through unchanged matches the old ingest-side
        // fallback so the destination gets a chance to surface the
        // error properly. Truncated below the TrackId byte means we
        // can't tell which track it is; treat that as "forward".
        let truncated = vec![0x80 | (1u8 << 4) | 6, 0]; // 2 bytes total
        let youtube = select_video_bytes(&truncated, VideoEgress::Track(0))
            .expect("truncated tags must still forward");
        assert_eq!(youtube.as_ref(), truncated.as_slice());
        assert!(matches!(youtube, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn avc_legacy_with_unknown_codec_id_returns_unknown() {
        // codec_id != 7 (we only understand AVC in legacy mode)
        let tag = vec![0x12, 1, 0, 0, 0, 0x05]; // codec_id = 2 (Sorenson)
        let info = classify_video_tag(&tag);
        assert_eq!(info.codec, VideoCodec::Unknown);
        assert!(!info.is_idr);
    }

    // ── Enhanced-RTMP audio (VOD audio path) ───────────────────────
    //
    // OBS's VOD-audio feature emits Enhanced-RTMP audio packets via
    // flv_packet_audio_ex in plugins/obs-outputs/flv-mux.c. The Live
    // track (idx=0) is sent as Enhanced-RTMP SINGLE-TRACK and the VOD
    // track (idx>0) as Enhanced-RTMP OneTrack multi-track. Layouts:
    //
    //   single-track: [byte0 0x9X][bytes 1..5 FourCC][payload]
    //                     X = AudioPacketType (0=Seq, 1=Frames)
    //
    //   multi-track:  [byte0 0x95][byte1 mt_type|nested_pt][bytes 2..6 FourCC]
    //                 [byte6 TrackId][payload]
    //                     byte0 low nibble = 5 (Multitrack)
    //                     byte1 high nibble = 0 (OneTrack)
    //                     byte1 low nibble  = nested packet type
    //
    // v0.1.0..0.1.3 mis-read this format - PacketType from byte 1
    // instead of byte 0, TrackId from byte 7 instead of byte 6. With
    // VOD audio enabled the live track's seq-header was not detected
    // (the classifier saw PacketType = 'm' & 0x0F = 0x0D), so it went
    // into the ring like a regular tag and got evicted by trim before
    // the destination consumer read it - Twitch never received an
    // AudioSpecificConfig and both audio tracks rendered silent.

    /// Helper: build an Enhanced-RTMP single-track AAC audio tag.
    /// packet_type: 0=SequenceStart, 1=CodedFrames.
    fn enhanced_audio_single_track(packet_type: u8) -> Vec<u8> {
        let mut payload = vec![
            0x90 | (packet_type & 0x0F), // SoundFormat=9 | AudioPacketType
            b'm',
            b'p',
            b'4',
            b'a', // FourCC=mp4a
        ];
        // AudioSpecificConfig stub (or AAC frame data; bytes don't
        // matter for classification, only their position relative to
        // the header).
        payload.extend_from_slice(&[0x12, 0x10, 0x56, 0xe5]);
        payload
    }

    /// Helper: build an Enhanced-RTMP OneTrack multi-track audio tag.
    /// nested_pt: 0=SequenceStart, 1=CodedFrames.
    fn enhanced_audio_onetrack(track_id: u8, nested_pt: u8) -> Vec<u8> {
        let mut payload = vec![
            0x95,             // SoundFormat=9 | PacketType=Multitrack
            nested_pt & 0x0F, // MultiTrackType=0 (high nibble) | NestedPT (low)
            b'm',
            b'p',
            b'4',
            b'a',     // FourCC=mp4a
            track_id, // TrackId at byte 6
        ];
        payload.extend_from_slice(&[0x12, 0x10, 0x56, 0xe5]);
        payload
    }

    #[test]
    fn enhanced_single_track_seq_header_is_detected() {
        // The user-visible regression for issue #9 (post-v0.1.3 manual
        // test): with VOD audio enabled, OBS sends the live track as
        // Enhanced-RTMP single-track. v0.1.0..0.1.3 read PacketType
        // from byte 1 (which is 'm' = 0x6D, low nibble 0x0D) and never
        // matched the SequenceStart value - so the AudioSpecificConfig
        // tag flowed into the ring like a normal frame and got
        // evicted, leaving Twitch's decoder unconfigured.
        let tag = enhanced_audio_single_track(0);
        let info = classify_audio_tag(&tag);
        assert!(
            info.is_seq_header,
            "Enhanced-RTMP single-track byte 0 = 0x90 must classify as seq header"
        );
        assert!(!info.is_multitrack);
        assert_eq!(info.codec, AudioCodec::Aac);
    }

    #[test]
    fn enhanced_single_track_coded_frame_is_not_seq_header() {
        let tag = enhanced_audio_single_track(1);
        let info = classify_audio_tag(&tag);
        assert!(!info.is_seq_header);
        assert!(!info.is_multitrack);
        assert_eq!(info.codec, AudioCodec::Aac);
    }

    #[test]
    fn multitrack_audio_seqstart_detected_as_seq_header() {
        // Pre-Phase-A, this tag would have been treated as a regular
        // runtime tag (is_seq_header=false) because the outer
        // PacketType is 5 (Multitrack), not 0 (SequenceStart). With
        // the nested-packet-type check it correctly reads as a seq
        // header that needs caching for replay across cuts.
        let tag = enhanced_audio_onetrack(0, 0);
        let info = classify_audio_tag(&tag);
        assert!(
            info.is_seq_header,
            "nested PT=0 must be flagged as seq header"
        );
        assert!(info.is_multitrack);
        assert_eq!(info.codec, AudioCodec::Aac);
    }

    #[test]
    fn multitrack_audio_codedframes_is_not_seq_header() {
        let tag = enhanced_audio_onetrack(0, 1);
        let info = classify_audio_tag(&tag);
        assert!(!info.is_seq_header);
        assert!(info.is_multitrack);
    }

    #[test]
    fn audio_seq_header_track_id_reads_onetrack_byte_6() {
        // OBS writes TrackId at byte 6 of the audio tag payload
        // (byte 0 = 0x95, byte 1 = mt|nested, bytes 2..6 = FourCC,
        // byte 6 = TrackId). Our cache keys on this so a publisher
        // reconnect can replay every track's config.
        for track in 0u8..=3 {
            let tag = enhanced_audio_onetrack(track, 0);
            assert_eq!(audio_seq_header_track_id(&tag), track);
        }
    }

    #[test]
    fn audio_seq_header_track_id_returns_zero_for_legacy_aac() {
        // Legacy AAC seq header: sound_format=10, AACPacketType=0.
        // No track-id field exists; helper must return 0.
        let tag = vec![0xaf, 0x00, 0x12, 0x10, 0x56, 0xe5];
        assert_eq!(audio_seq_header_track_id(&tag), 0);
    }

    #[test]
    fn select_audio_bytes_passes_single_track_through_unchanged() {
        // Legacy AAC and Enhanced single-track must always pass
        // through, both in passthrough and flatten mode. The helper
        // is only allowed to drop OneTrack multi-track tags with
        // TrackId != 0.
        let legacy_aac = vec![0xaf, 0x01, 0x12, 0x10, 0x56];
        for passthrough in [true, false] {
            let out = select_audio_bytes(&legacy_aac, passthrough)
                .expect("single-track audio must forward");
            assert_eq!(out.as_ref(), legacy_aac.as_slice());
            assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
        }
    }

    #[test]
    fn select_audio_bytes_passes_multitrack_through_for_twitch() {
        // Twitch destination: every track's bytes flow through
        // bit-faithfully. The IVS pipeline binds each track id to
        // its session-allocated slot.
        for track in 0u8..=3 {
            let tag = enhanced_audio_onetrack(track, 1);
            let out = select_audio_bytes(&tag, true)
                .unwrap_or_else(|| panic!("twitch must forward track {track}"));
            assert_eq!(out.as_ref(), tag.as_slice());
        }
    }

    #[test]
    fn select_audio_bytes_drops_nonzero_track_for_non_twitch() {
        // Non-Twitch destinations only get TrackId 0. Track 1 (VOD
        // audio) is meaningless to YouTube / Kick - drop it rather
        // than confuse the destination's decoder.
        for track in 1u8..=3 {
            let tag = enhanced_audio_onetrack(track, 1);
            assert!(
                select_audio_bytes(&tag, false).is_none(),
                "TrackId {track} must be dropped on non-Twitch egress",
            );
        }
    }

    #[test]
    fn select_audio_bytes_keeps_track_zero_for_non_twitch() {
        let tag = enhanced_audio_onetrack(0, 1);
        let out = select_audio_bytes(&tag, false).expect("track 0 must reach every destination");
        assert_eq!(out.as_ref(), tag.as_slice());
    }
}
