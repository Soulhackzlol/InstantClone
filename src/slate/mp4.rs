//! A minimal MP4 writer (ISO/IEC 14496-12 and -15) for the dashboard's
//! preview of a soft screen: the H.264 loop the stream would carry, in one
//! track a browser plays with `<video loop>`. One loop pass, every sample
//! in a single chunk, no edit list and no audio.

use super::encoder::ParameterSets;
use super::SlateLoop;

/// The identity transform every track header and movie header carries.
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
/// ISO 639-2 "und" packed into 15 bits.
const LANGUAGE_UNDETERMINED: u16 = 0x55c4;
/// Movie header timescale: milliseconds.
const MOVIE_TIMESCALE: u32 = 1000;

/// An MP4 holding one pass of `slate`, an H.264 loop whose parameter sets
/// are `sets`, at `fps`.
pub fn h264_loop(slate: &SlateLoop, sets: &ParameterSets, fps: u32) -> Vec<u8> {
    let samples: Vec<&[u8]> = slate
        .intervals
        .iter()
        .enumerate()
        .flat_map(|(index, interval)| {
            std::iter::once(&interval.keyframes[index % 2][..])
                .chain(interval.deltas.iter().map(Vec::as_slice))
        })
        .collect();
    let per_interval = slate.intervals.first().map_or(1, |i| 1 + i.deltas.len());
    let keyframes: Vec<u32> = (0..slate.intervals.len())
        .map(|index| (index * per_interval + 1) as u32)
        .collect();
    let track = Track {
        width: slate.shape.width as u16,
        height: slate.shape.height as u16,
        fps,
        sizes: samples.iter().map(|sample| sample.len() as u32).collect(),
        keyframes,
        sets,
    };
    let file_type = file_type_box();
    // The chunk offset counts the bytes before the samples, which include
    // the movie box itself; its length doesn't depend on the offset.
    let movie_length = track.movie_box(0).len();
    let data_offset = (file_type.len() + movie_length + 8) as u32;
    let data: Vec<u8> = samples.concat();
    [
        file_type,
        track.movie_box(data_offset),
        boxed(b"mdat", &data),
    ]
    .concat()
}

struct Track<'a> {
    width: u16,
    height: u16,
    fps: u32,
    sizes: Vec<u32>,
    /// 1-based numbers of the samples that are keyframes.
    keyframes: Vec<u32>,
    sets: &'a ParameterSets,
}

impl Track<'_> {
    fn frame_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    fn movie_box(&self, data_offset: u32) -> Vec<u8> {
        let duration_ms = self.frame_count() * MOVIE_TIMESCALE / self.fps;
        let mut header = Body::new();
        header.u32s(&[0, 0, MOVIE_TIMESCALE, duration_ms]);
        header.u32s(&[0x0001_0000]); // rate 1.0
        header.u16s(&[0x0100, 0]); // volume 1.0, reserved
        header.u32s(&[0, 0]);
        header.u32s(&UNITY_MATRIX);
        header.u32s(&[0; 6]); // pre_defined
        header.u32s(&[2]); // next_track_ID
        let movie_header = full_box(b"mvhd", 0, 0, &header.0);
        boxed(
            b"moov",
            &[movie_header, self.track_box(duration_ms, data_offset)].concat(),
        )
    }

    fn track_box(&self, duration_ms: u32, data_offset: u32) -> Vec<u8> {
        let mut header = Body::new();
        header.u32s(&[0, 0, 1, 0, duration_ms, 0, 0]); // times, track 1, duration
        header.u16s(&[0, 0, 0, 0]); // layer, alternate group, volume, reserved
        header.u32s(&UNITY_MATRIX);
        header.u32s(&[u32::from(self.width) << 16, u32::from(self.height) << 16]);
        // Flags: enabled, in movie.
        let track_header = full_box(b"tkhd", 0, 3, &header.0);
        let mut media_header = Body::new();
        media_header.u32s(&[0, 0, self.fps, self.frame_count()]);
        media_header.u16s(&[LANGUAGE_UNDETERMINED, 0]);
        let mut handler = Body::new();
        handler.u32s(&[0]);
        handler.0.extend_from_slice(b"vide");
        handler.u32s(&[0, 0, 0]);
        handler.0.extend_from_slice(b"VideoHandler\0");
        let media = boxed(
            b"mdia",
            &[
                full_box(b"mdhd", 0, 0, &media_header.0),
                full_box(b"hdlr", 0, 0, &handler.0),
                self.media_information_box(data_offset),
            ]
            .concat(),
        );
        boxed(b"trak", &[track_header, media].concat())
    }

    fn media_information_box(&self, data_offset: u32) -> Vec<u8> {
        // Graphics mode copy, no opcolor; flag 1 as the spec requires.
        let video_header = full_box(b"vmhd", 0, 1, &[0; 8]);
        // One data reference: this file (flag 1, no URL).
        let mut references = Body::new();
        references.u32s(&[1]);
        references.0.extend(full_box(b"url ", 0, 1, &[]));
        let data_information = boxed(b"dinf", &full_box(b"dref", 0, 0, &references.0));
        boxed(
            b"minf",
            &[
                video_header,
                data_information,
                self.sample_table_box(data_offset),
            ]
            .concat(),
        )
    }

    fn sample_table_box(&self, data_offset: u32) -> Vec<u8> {
        let mut description = Body::new();
        description.u32s(&[1]);
        description.0.extend(self.visual_entry());
        let mut times = Body::new();
        times.u32s(&[1, self.frame_count(), 1]); // every frame one tick long
        let mut sync = Body::new();
        sync.u32s(&[self.keyframes.len() as u32]);
        sync.u32s(&self.keyframes);
        let mut chunks = Body::new();
        chunks.u32s(&[1, 1, self.frame_count(), 1]); // all samples in chunk 1
        let mut sizes = Body::new();
        sizes.u32s(&[0, self.frame_count()]);
        sizes.u32s(&self.sizes);
        let mut offsets = Body::new();
        offsets.u32s(&[1, data_offset]);
        boxed(
            b"stbl",
            &[
                full_box(b"stsd", 0, 0, &description.0),
                full_box(b"stts", 0, 0, &times.0),
                full_box(b"stss", 0, 0, &sync.0),
                full_box(b"stsc", 0, 0, &chunks.0),
                full_box(b"stsz", 0, 0, &sizes.0),
                full_box(b"stco", 0, 0, &offsets.0),
            ]
            .concat(),
        )
    }

    /// The avc1 sample entry with its decoder configuration (avcC).
    fn visual_entry(&self) -> Vec<u8> {
        let mut entry = Body::new();
        entry.u16s(&[0, 0, 0, 1]); // reserved, data_reference_index 1
        entry.u32s(&[0, 0, 0, 0]); // pre_defined and reserved
        entry.u16s(&[self.width, self.height]);
        entry.u32s(&[0x0048_0000, 0x0048_0000, 0]); // 72 dpi, reserved
        entry.u16s(&[1]); // frame_count
        entry.0.extend_from_slice(&[0; 32]); // compressorname
        entry.u16s(&[0x0018, 0xffff]); // depth 24, pre_defined -1
        let (sps, pps) = (&self.sets.sps, &self.sets.pps);
        let mut config = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
        config.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        config.extend_from_slice(sps);
        config.push(1);
        config.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        config.extend_from_slice(pps);
        entry.0.extend(boxed(b"avcC", &config));
        boxed(b"avc1", &entry.0)
    }
}

fn file_type_box() -> Vec<u8> {
    let mut body = b"isom".to_vec();
    body.extend_from_slice(&0x200u32.to_be_bytes());
    for brand in [b"isom", b"iso2", b"avc1", b"mp41"] {
        body.extend_from_slice(brand);
    }
    boxed(b"ftyp", &body)
}

/// A box: its size, its type, then `body`.
fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

/// A full box: a box whose body starts with a version and 24 bits of flags.
fn full_box(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut with_header = (u32::from(version) << 24 | flags).to_be_bytes().to_vec();
    with_header.extend_from_slice(body);
    boxed(kind, &with_header)
}

/// Big-endian fields, in order.
struct Body(Vec<u8>);

impl Body {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn u32s(&mut self, values: &[u32]) {
        for value in values {
            self.0.extend_from_slice(&value.to_be_bytes());
        }
    }

    fn u16s(&mut self, values: &[u16]) {
        for value in values {
            self.0.extend_from_slice(&value.to_be_bytes());
        }
    }
}
