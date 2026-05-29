//! Codec parsing for FLV video/audio tag payloads.
//!
//! Covers both legacy (AVC, AAC, MP3) and Enhanced RTMP (HEVC, AV1, VP9,
//! Opus, AC-3, EAC-3, FLAC, multi-track) tag headers. We only need to
//! identify three things — actual decoding never happens:
//!
//!   * sequence headers (cache + resend on every cut and reconnect)
//!   * keyframes / IDRs (cut points the player can resync to)
//!   * multi-track packets (Twitch Enhanced Broadcasting — handled at
//!     ingest by extracting the first/primary track and rewriting the
//!     tag as standard single-track)
//!
//! The rest of the pipeline is bit-transparent on the payload, so once
//! ingest classifies and (optionally) flattens multi-track, every
//! downstream send is just `writer.write_message(... payload ...)`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Avc,    // H.264
    Hevc,   // H.265
    Av1,
    Vp9,
    Unknown,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::Avc     => "H.264",
            VideoCodec::Hevc    => "HEVC",
            VideoCodec::Av1     => "AV1",
            VideoCodec::Vp9     => "VP9",
            VideoCodec::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VideoTagInfo {
    pub is_seq_header: bool,
    pub is_idr: bool,
    pub is_multitrack: bool,
    pub codec: VideoCodec,
}

const FOURCC_HVC1: [u8; 4] = *b"hvc1";
const FOURCC_HEV1: [u8; 4] = *b"hev1";
const FOURCC_AV01: [u8; 4] = *b"av01";
const FOURCC_VP09: [u8; 4] = *b"vp09";
const FOURCC_AVC1: [u8; 4] = *b"avc1";

pub fn classify_video_tag(payload: &[u8]) -> VideoTagInfo {
    let unknown = VideoTagInfo {
        is_seq_header: false, is_idr: false,
        is_multitrack: false, codec: VideoCodec::Unknown,
    };
    if payload.is_empty() { return unknown; }
    let b0 = payload[0];
    let is_ex_header = b0 & 0x80 != 0;

    if !is_ex_header {
        if payload.len() < 5 { return unknown; }
        let codec_id = b0 & 0x0F;
        if codec_id != 7 {
            return unknown; // We only understand AVC in legacy mode.
        }
        let avc_packet_type = payload[1];
        let is_seq_header = avc_packet_type == 0;
        // Sequence headers carry SPS/PPS, never an IDR slice — trust the
        // NAL walk rather than the FLV FrameType nibble (some encoders set
        // FrameType=key on the seq header too).
        let is_idr = avc_packet_type == 1 && contains_idr_nalu(&payload[5..]);
        return VideoTagInfo {
            is_seq_header, is_idr,
            is_multitrack: false,
            codec: VideoCodec::Avc,
        };
    }

    // Enhanced RTMP: bit7=IsEx, bits6:4=FrameType, bits3:0=PacketType.
    //   PacketType 0=SequenceStart, 1=CodedFrames, 2=SequenceEnd,
    //              3=CodedFramesX (no composition time),
    //              4=Metadata, 5=MPEG2TSSequenceStart, 6=Multitrack.
    let frame_type  = (b0 >> 4) & 0x07;
    let packet_type =  b0       & 0x0F;
    let is_seq_header = packet_type == 0;
    // Enhanced encoders always set FrameType=1 on keyframes. For AVC we
    // walked NALUs because some encoders flag P-frames as key incorrectly;
    // enhanced-rtmp tightens this and FrameType is the spec-blessed signal.
    let is_keyframe = frame_type == 1 && packet_type != 0;
    let is_multitrack = packet_type == 6;

    // For multi-track, the FourCC sits behind the multitrack header; we
    // don't bother classifying the inner codec here because the ingest
    // path flattens it to single-track before any downstream code runs.
    if is_multitrack {
        return VideoTagInfo {
            is_seq_header, is_idr: is_keyframe,
            is_multitrack: true,
            codec: VideoCodec::Unknown,
        };
    }

    if payload.len() < 5 {
        return VideoTagInfo {
            is_seq_header, is_idr: is_keyframe,
            is_multitrack: false,
            codec: VideoCodec::Unknown,
        };
    }
    let fourcc = [payload[1], payload[2], payload[3], payload[4]];
    let codec = match fourcc {
        FOURCC_HVC1 | FOURCC_HEV1 => VideoCodec::Hevc,
        FOURCC_AV01               => VideoCodec::Av1,
        FOURCC_VP09               => VideoCodec::Vp9,
        FOURCC_AVC1               => VideoCodec::Avc,
        _                         => VideoCodec::Unknown,
    };
    VideoTagInfo {
        is_seq_header,
        is_idr: is_keyframe,
        is_multitrack: false,
        codec,
    }
}

fn contains_idr_nalu(mut data: &[u8]) -> bool {
    while data.len() >= 4 {
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if 4 + len > data.len() { return false; }
        let nal_unit_type = data[4] & 0x1F;
        if nal_unit_type == 5 { return true; }
        data = &data[4 + len..];
    }
    false
}

/// Flatten an Enhanced RTMP multi-track video tag down to a standard
/// single-track tag carrying the first listed track only.
///
/// Twitch's Enhanced Broadcasting feeds simulcast resolutions in a single
/// RTMP stream — picking the first track keeps the user's primary
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
    if payload.len() < 6 { return None; }
    let b0 = payload[0];
    if (b0 & 0x80) == 0 || (b0 & 0x0F) != 6 { return None; }
    let frame_type = (b0 >> 4) & 0x07;
    let mt_header  = payload[1];
    let mt_type    = (mt_header >> 4) & 0x0F;
    let nested_pt  =  mt_header       & 0x0F;

    // Stitch the rewritten single-track tag header.
    let mut out = Vec::with_capacity(payload.len());
    let header_byte = 0x80 | ((frame_type & 0x07) << 4) | (nested_pt & 0x0F);

    match mt_type {
        0 => {
            // OneTrack: [FourCC(4)][TrackId(1)][payload..]
            if payload.len() < 7 { return None; }
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
            if payload.len() < 6 + 4 { return None; }
            let fourcc = &payload[2..6];
            let rest = &payload[6..];
            if rest.len() < 4 { return None; }
            let track_len = u32::from_be_bytes([0, rest[1], rest[2], rest[3]]) as usize;
            if rest.len() < 4 + track_len { return None; }
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
            if rest.len() < 8 { return None; }
            let fourcc = &rest[1..5];
            let track_len = u32::from_be_bytes([0, rest[5], rest[6], rest[7]]) as usize;
            if rest.len() < 8 + track_len { return None; }
            let track_payload = &rest[8..8 + track_len];
            out.push(header_byte);
            out.extend_from_slice(fourcc);
            out.extend_from_slice(track_payload);
            Some(out)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Aac, Mp3, Opus, Ac3, Eac3, Flac, Unknown,
}

impl AudioCodec {
    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Aac     => "AAC",
            AudioCodec::Mp3     => "MP3",
            AudioCodec::Opus    => "Opus",
            AudioCodec::Ac3     => "AC-3",
            AudioCodec::Eac3    => "E-AC-3",
            AudioCodec::Flac    => "FLAC",
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
const FOURCC_AC3:  [u8; 4] = *b"ac-3";
const FOURCC_EAC3: [u8; 4] = *b"ec-3";
const FOURCC_FLAC: [u8; 4] = *b"fLaC";
const FOURCC_MP4A: [u8; 4] = *b"mp4a";

pub fn classify_audio_tag(payload: &[u8]) -> AudioTagInfo {
    let unknown = AudioTagInfo {
        is_seq_header: false, is_multitrack: false, codec: AudioCodec::Unknown,
    };
    if payload.is_empty() { return unknown; }
    let sound_format = (payload[0] >> 4) & 0x0F;

    // Legacy AAC: byte 1 is AACPacketType (0=AudioSpecificConfig).
    if sound_format == 10 {
        let is_seq_header = payload.len() >= 2 && payload[1] == 0;
        return AudioTagInfo {
            is_seq_header, is_multitrack: false, codec: AudioCodec::Aac,
        };
    }
    if sound_format == 2 {
        return AudioTagInfo { codec: AudioCodec::Mp3, ..unknown };
    }
    // Enhanced RTMP audio uses sound_format=9 as the sentinel for
    // ExAudioTagHeader. We pass these tags through bit-faithfully so
    // Twitch's VOD audio track (OBS "Audio Track 2") keeps working.
    if sound_format == 9 {
        if payload.len() < 2 { return unknown; }
        // byte 1: AudioPacketModEx(4) | AudioPacketType(4)
        //   PacketType 0=SequenceStart, 1=CodedFrames, 5=Multitrack.
        let packet_type = payload[1] & 0x0F;
        let is_seq_header = packet_type == 0;
        let is_multitrack = packet_type == 5;
        let codec = if !is_multitrack && payload.len() >= 6 {
            let fourcc = [payload[2], payload[3], payload[4], payload[5]];
            match fourcc {
                FOURCC_OPUS => AudioCodec::Opus,
                FOURCC_AC3  => AudioCodec::Ac3,
                FOURCC_EAC3 => AudioCodec::Eac3,
                FOURCC_FLAC => AudioCodec::Flac,
                FOURCC_MP4A => AudioCodec::Aac,
                _           => AudioCodec::Unknown,
            }
        } else {
            AudioCodec::Unknown
        };
        return AudioTagInfo { is_seq_header, is_multitrack, codec };
    }
    unknown
}

/// Back-compat shim — sink.rs and a few other callers used this name
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
        out.push(nal_type & 0x1F); // forbidden_zero=0, nal_ref_idc=0 — fine for the type check
        out.resize(out.len() + body_len, 0);
        out
    }

    /// Builds an AVC video tag: FLV header (5 bytes) + NAL units.
    /// frame_type: 1 = keyframe (high nibble); codec_id 7 (AVC, low nibble).
    fn avc_tag(packet_type: u8, nals: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0x17, packet_type, 0, 0, 0]; // FrameType=1, CodecID=7
        for n in nals { out.extend_from_slice(n); }
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
        // nibble — we trust the NAL walk and refuse to call it an IDR.
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
        assert!(!info.is_idr, "seq headers carry SPS/PPS, never an IDR slice");
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
        // PacketType=0 (SequenceStart) — must not be flagged as IDR
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
        // sound_format 9 (ExAudio), packet_type 1 (CodedFrames), FourCC "Opus"
        let mut payload = vec![0x90, 0x01];
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
    fn enhanced_rtmp_multitrack_packettype_6() {
        // packet_type 6 = Multitrack. Should be flagged as multitrack
        // but FrameType=1 still surfaces is_idr.
        let byte0 = 0x80 | (1u8 << 4) | 6;
        let payload = vec![byte0, 0, 0, 0, 0];
        let info = classify_video_tag(&payload);
        assert!(info.is_multitrack);
        assert!(info.is_idr); // keyframe nibble still wins
    }

    #[test]
    fn enhanced_aac_seq_header_via_packet_type_0() {
        // sound_format 9 (ExAudio), packet_type 0 (SequenceStart), FourCC "mp4a"
        let mut payload = vec![0x90, 0x00];
        payload.extend_from_slice(b"mp4a");
        let info = classify_audio_tag(&payload);
        assert!(info.is_seq_header);
        assert_eq!(info.codec, AudioCodec::Aac);
        // The convenience shim agrees.
        assert!(is_aac_seq_header(&payload));
    }

    #[test]
    fn avc_legacy_with_unknown_codec_id_returns_unknown() {
        // codec_id != 7 (we only understand AVC in legacy mode)
        let tag = vec![0x12, 1, 0, 0, 0, 0x05]; // codec_id = 2 (Sorenson)
        let info = classify_video_tag(&tag);
        assert_eq!(info.codec, VideoCodec::Unknown);
        assert!(!info.is_idr);
    }
}
