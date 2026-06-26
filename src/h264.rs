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

/// Per-destination video-tag selection. Decides which bytes to put on
/// the wire given the destination's Enhanced Broadcasting policy:
///
/// - `pass_through_multitrack = true`: Twitch destinations. Multi-track
///   tags go through bit-faithfully so Twitch's edge can populate the
///   transcoded ladder from the simulcast.
/// - `pass_through_multitrack = false`: every other RTMP ingest we
///   know of. Multi-track tags get flattened down to the primary
///   track (matching what beta.6 emitted from the ingest-side flatten),
///   single-track tags pass through unchanged.
///
/// Single-track tags ALWAYS pass through unchanged regardless of the
/// flag - there's nothing to flatten and no destination policy that
/// would want them changed. Returns a borrow when no copy is needed
/// (the common case) and an owned `Vec` only when a flatten was
/// actually performed.
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

pub fn select_video_bytes<'a>(
    payload: &'a [u8],
    pass_through_multitrack: bool,
) -> Option<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    if pass_through_multitrack {
        return Some(Cow::Borrowed(payload));
    }
    let info = classify_video_tag(payload);
    if !info.is_multitrack {
        return Some(Cow::Borrowed(payload));
    }
    // For OneTrack layout, OBS sends one tag per track per frame.
    // Forwarding every track to a non-Twitch destination would deliver
    // N frames per PTS - decoders read that as a multi-frame storm and
    // either drop the connection (YouTube did, ~12 s per ladder rung)
    // or render heavy artefacts. The primary stream arrives separately
    // as a legacy AVC / Enhanced single-track tag, so dropping
    // TrackId != 0 here is lossless from the destination's POV.
    if payload.len() >= 7 && (payload[1] >> 4) & 0x0F == 0 {
        let track_id = payload[6];
        if track_id != 0 {
            return None;
        }
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
        let twitch = select_video_bytes(&tag, true).expect("single-track always forwards");
        let youtube = select_video_bytes(&tag, false).expect("single-track always forwards");
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
            let twitch = select_video_bytes(&tag, true)
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
        let youtube = select_video_bytes(&tag, false).expect("track 0 must be forwarded");
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
                select_video_bytes(&tag, false).is_none(),
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
            let twitch = select_video_bytes(tag, true).expect("twitch must forward both canvases");
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
            select_video_bytes(&horizontal, false).is_some(),
            "horizontal canvas (TrackId 0) must reach non-Twitch"
        );
        assert!(
            select_video_bytes(&vertical, false).is_none(),
            "vertical canvas (TrackId 1) must be dropped for non-Twitch"
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
        let youtube =
            select_video_bytes(&truncated, false).expect("truncated tags must still forward");
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
