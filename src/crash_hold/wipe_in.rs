//! The luma wipe a delay transition opens with, fitted to the stream it
//! carries on from.
//!
//! A wipe's pictures are built with the screen's loop (`slate::Wipe`);
//! their slice headers must carry on the stream's own picture numbering,
//! so they are written as the transition starts, from the pictures this
//! destination was sent since the last keyframe before the pause (read
//! back from the ring, as the pump sent them).

use super::{
    video_tag, Framing, ScreenTarget, AVC_NALU, EX_CODED_FRAMES, EX_VIDEO, EX_VIDEO_MULTITRACK,
    FLV_VIDEO_AVC, FOURCC_AVC1, ONE_TRACK,
};
use crate::controller::Controller;
use crate::h264::VideoEgress;
use crate::slate::{read_pps_id, read_sps, wipe_pictures, Follower, Wipe};

/// Enhanced RTMP CodedFramesX: coded frames with no composition time.
const EX_CODED_FRAMES_X: u8 = 3;
/// How far back through the ring to look for the keyframe the pictures
/// before the pause hang off: a 2 s GOP at 60 fps with audio and a few
/// multitrack renditions is some hundreds of tags.
const MOST_TAGS_BACK: u64 = 4000;
/// FLV video tags in the ring.
const TAG_VIDEO: u8 = 9;
const NAL_IDR_SLICE: u8 = 5;
/// The NAL length size the wipe's samples use, which the stream's must
/// match.
const NAL_LENGTH_SIZE: usize = 4;

/// The wipe's pictures as `target`'s tags, carrying on from what the
/// destination was sent of the track whose sequence header is `header`,
/// up to ring seq `paused_at`. `None` when the stream can't be followed:
/// not H.264 in 4-byte NAL lengths, an SPS the wipe can't continue, a
/// picture size other than the wipe's, or no keyframe in reach.
pub(super) fn wipe_tags(
    ctrl: &Controller,
    egress: VideoEgress,
    (header, target): (&[u8], ScreenTarget),
    paused_at: u64,
    wipe: &Wipe,
) -> Option<Vec<Vec<u8>>> {
    let record = crate::h264::avc_config_record(header)?;
    if usize::from(record.get(4)? & 0x03) + 1 != NAL_LENGTH_SIZE {
        return None;
    }
    let (sps_nals, pps_nals) = parameter_sets(record)?;
    let sps = read_sps(sps_nals.first()?)?;
    if (sps.width_mbs, sps.height_mbs) != (wipe.width_mbs, wipe.height_mbs) {
        return None;
    }
    let mut pps_ids: Vec<u32> = pps_nals.iter().filter_map(|nal| read_pps_id(nal)).collect();
    let mut follower = Follower::new(sps);
    for sample in samples_since_keyframe(ctrl, egress, target.framing, paused_at)? {
        for nal in nal_units(&sample) {
            pps_ids.extend(read_pps_id(nal));
            follower.nal(nal)?;
        }
    }
    let numbering = follower.numbering(&pps_ids)?;
    let tags = wipe_pictures(wipe, &numbering)
        .iter()
        .map(|sample| video_tag(target.framing, target.fourcc, false, sample))
        .collect();
    Some(tags)
}

/// NAL units borrowed from a config record.
type NalList<'a> = Vec<&'a [u8]>;

/// The SPS and PPS NAL units of an AVCDecoderConfigurationRecord.
fn parameter_sets(record: &[u8]) -> Option<(NalList<'_>, NalList<'_>)> {
    let sps_count = usize::from(record.get(5)? & 0x1F);
    let mut at = 6;
    let sps = length_prefixed_list(record, &mut at, sps_count)?;
    let pps_count = usize::from(*record.get(at)?);
    at += 1;
    let pps = length_prefixed_list(record, &mut at, pps_count)?;
    Some((sps, pps))
}

/// `count` NAL units from `at` on, each after its 2-byte length.
fn length_prefixed_list<'a>(
    record: &'a [u8],
    at: &mut usize,
    count: usize,
) -> Option<Vec<&'a [u8]>> {
    let mut nals = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::from(u16::from_be_bytes([
            *record.get(*at)?,
            *record.get(*at + 1)?,
        ]));
        nals.push(record.get(*at + 2..*at + 2 + len)?);
        *at += 2 + len;
    }
    Some(nals)
}

/// The samples of the track sent in `framing`, from its last keyframe
/// before ring seq `paused_at` up to the pause, oldest first.
fn samples_since_keyframe(
    ctrl: &Controller,
    egress: VideoEgress,
    framing: Framing,
    paused_at: u64,
) -> Option<Vec<Vec<u8>>> {
    let mut samples = Vec::new();
    let mut tag = Vec::new();
    for back in 1..=MOST_TAGS_BACK {
        let seq = paused_at.checked_sub(back)?;
        let (_, meta) = ctrl.ring.find_by_seq(seq)?;
        if meta.kind != TAG_VIDEO {
            continue;
        }
        ctrl.ring.try_read_seq(seq, &mut tag).ok()??;
        let Some(sent) = crate::h264::select_video_bytes(&tag, egress) else {
            continue;
        };
        let Some(sample) = avc_sample(&sent, framing) else {
            continue;
        };
        let is_keyframe = nal_units(sample).any(|nal| nal[0] & 0x1F == NAL_IDR_SLICE);
        samples.push(sample.to_vec());
        if is_keyframe {
            samples.reverse();
            return Some(samples);
        }
    }
    None
}

/// The coded H.264 picture a video tag in `framing` carries, as
/// length-prefixed NAL units; `None` for any other tag (a config, another
/// track, another codec).
fn avc_sample(tag: &[u8], framing: Framing) -> Option<&[u8]> {
    let &first = tag.first()?;
    // Where the NAL units start, after the composition time if any.
    let coded = |packet_type: u8, at: usize| match packet_type {
        EX_CODED_FRAMES => tag.get(at + 3..),
        EX_CODED_FRAMES_X => tag.get(at..),
        _ => None,
    };
    match framing {
        Framing::Legacy => {
            let is_avc = first & EX_VIDEO == 0 && first & 0x0F == FLV_VIDEO_AVC;
            (is_avc && *tag.get(1)? == AVC_NALU)
                .then(|| tag.get(5..))
                .flatten()
        }
        Framing::Enhanced => {
            let is_avc = first & EX_VIDEO != 0 && tag.get(1..5)? == FOURCC_AVC1;
            is_avc.then(|| coded(first & 0x0F, 5)).flatten()
        }
        Framing::OneTrack(track) => {
            let layout = *tag.get(1)?;
            let is_track = first & EX_VIDEO != 0
                && first & 0x0F == EX_VIDEO_MULTITRACK
                && layout >> 4 == ONE_TRACK
                && tag.get(2..6)? == FOURCC_AVC1
                && *tag.get(6)? == track;
            is_track.then(|| coded(layout & 0x0F, 7)).flatten()
        }
    }
}

/// The NAL units of a sample with 4-byte lengths, until one doesn't fit.
fn nal_units(mut sample: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        let length = u32::from_be_bytes(sample.get(..NAL_LENGTH_SIZE)?.try_into().ok()?) as usize;
        let nal = sample.get(NAL_LENGTH_SIZE..NAL_LENGTH_SIZE + length)?;
        sample = &sample[NAL_LENGTH_SIZE + length..];
        Some(nal)
    })
    .filter(|nal| !nal.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_pictures_are_found_in_every_framing() {
        let nals = [0, 0, 0, 1, 0x65];
        let legacy = [&[0x27, AVC_NALU, 0, 0, 0][..], &nals].concat();
        assert_eq!(avc_sample(&legacy, Framing::Legacy), Some(&nals[..]));
        let enhanced = [
            &[EX_VIDEO | 0x20 | EX_CODED_FRAMES][..],
            &FOURCC_AVC1,
            &[0, 0, 0],
            &nals,
        ]
        .concat();
        assert_eq!(avc_sample(&enhanced, Framing::Enhanced), Some(&nals[..]));
        let one_track = [
            &[
                EX_VIDEO | 0x20 | EX_VIDEO_MULTITRACK,
                ONE_TRACK << 4 | EX_CODED_FRAMES_X,
            ][..],
            &FOURCC_AVC1,
            &[2],
            &nals,
        ]
        .concat();
        assert_eq!(
            avc_sample(&one_track, Framing::OneTrack(2)),
            Some(&nals[..])
        );
        assert_eq!(
            avc_sample(&one_track, Framing::OneTrack(1)),
            None,
            "another track"
        );
        assert_eq!(avc_sample(&legacy, Framing::Enhanced), None);
        let config = [0x17, 0, 0, 0, 0, 1];
        assert_eq!(
            avc_sample(&config, Framing::Legacy),
            None,
            "a sequence header"
        );
    }

    /// The NAL units of an Annex B stream, without their start codes.
    fn annex_b_nals(stream: &[u8]) -> Vec<&[u8]> {
        let starts: Vec<usize> = (0..stream.len().saturating_sub(3))
            .filter(|at| stream[*at..*at + 3] == [0, 0, 1])
            .map(|at| at + 3)
            .collect();
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| {
                let end = starts.get(index + 1).map_or(stream.len(), |next| next - 3);
                // A 4-byte start code leaves its first zero on the unit before.
                let end = if stream[..end].ends_with(&[0]) && end > *start {
                    end - 1
                } else {
                    end
                };
                &stream[*start..end]
            })
            .collect()
    }

    /// A stream as the ring holds it (legacy FLV tags from x264, with
    /// B-frames) is followed up to the pause, and the wipe carries on its
    /// numbering: the same pictures `wipe_pictures` gives for the numbering
    /// read straight off the stream.
    #[test]
    fn the_wipe_follows_the_stream_in_the_ring() {
        use crate::crash_protection::CrashProtection;
        use std::sync::Arc;
        let Some(stream) = crate::slate::x264_stand_in(320, 180, 2) else {
            return;
        };
        let nals = annex_b_nals(&stream);
        let of_type = |t: u8| {
            *nals
                .iter()
                .find(|nal| nal[0] & 0x1F == t)
                .expect("a parameter set")
        };
        let (sps, pps) = (of_type(7), of_type(8));
        let mut header = vec![0x17, 0, 0, 0, 0, 1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
        header.extend((sps.len() as u16).to_be_bytes());
        header.extend(sps);
        header.push(1);
        header.extend((pps.len() as u16).to_be_bytes());
        header.extend(pps);

        let scratch = crate::test_support::TestDir::new("ic-wipe-follow");
        let ring =
            Arc::new(crate::buffer::DiskRing::create(&scratch.join("ring.buf"), 8 << 20).unwrap());
        let ctrl = Controller::new(ring, 0);
        ctrl.ring
            .append(TAG_VIDEO, 0, &header, false, true)
            .unwrap();
        let mut direct = Follower::new(read_sps(sps).unwrap());
        let mut pending: Vec<&[u8]> = Vec::new();
        let (mut keyframes, mut ts, mut paused_at) = (0, 0, None);
        for nal in nals.iter().filter(|nal| !matches!(nal[0] & 0x1F, 7 | 8)) {
            pending.push(nal);
            let nal_type = nal[0] & 0x1F;
            if nal_type != 1 && nal_type != NAL_IDR_SLICE {
                continue;
            }
            let is_idr = nal_type == NAL_IDR_SLICE;
            keyframes += usize::from(is_idr);
            let mut tag = vec![if is_idr { 0x17 } else { 0x27 }, AVC_NALU, 0, 0, 0];
            for unit in pending.drain(..) {
                tag.extend((unit.len() as u32).to_be_bytes());
                tag.extend(unit);
            }
            let seq = ctrl
                .ring
                .append(TAG_VIDEO, ts, &tag, is_idr, false)
                .unwrap();
            ts += 100;
            if keyframes == 2 {
                paused_at = seq;
                break;
            }
            for unit in nal_units(&tag[5..]) {
                direct.nal(unit).unwrap();
            }
        }
        let paused_at = paused_at.expect("a second keyframe");

        let target = super::super::screen_target(&header).expect("a screen for x264");
        let settings = CrashProtection::default();
        let slate = crate::slate::build_loop(&settings, target.shape, target.codec)
            .unwrap()
            .with_wipe(&settings, target.codec);
        let wipe = slate.wipe.as_ref().expect("an H.264 loop has a wipe");
        let tags = wipe_tags(
            &ctrl,
            VideoEgress::Passthrough,
            (&header, target),
            paused_at,
            wipe,
        )
        .expect("the stream followed");
        let numbering = direct.numbering(&[0]).unwrap();
        let expected: Vec<Vec<u8>> = wipe_pictures(wipe, &numbering)
            .iter()
            .map(|sample| video_tag(target.framing, target.fourcc, false, sample))
            .collect();
        assert_eq!(tags, expected);
    }

    #[test]
    fn a_config_record_gives_its_parameter_sets() {
        let sps = [0x67, 0x64, 0, 0x1F];
        let pps = [0x68, 0xEE];
        let mut record = vec![1, 0x64, 0, 0x1F, 0xFF, 0xE1, 0, sps.len() as u8];
        record.extend_from_slice(&sps);
        record.extend_from_slice(&[1, 0, pps.len() as u8]);
        record.extend_from_slice(&pps);
        let (sps_list, pps_list) = parameter_sets(&record).unwrap();
        assert_eq!(sps_list, [&sps[..]]);
        assert_eq!(pps_list, [&pps[..]]);
        assert_eq!(
            parameter_sets(&record[..record.len() - 1]),
            None,
            "truncated"
        );
    }
}
