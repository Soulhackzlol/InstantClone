//! Disk-backed ring buffer plus an in-memory tag index.
//!
//! The buffer file is pre-allocated to a fixed size (`capacity` bytes) at
//! startup and never grows. Writes advance a wrap-around cursor; reads are
//! random-access by `(offset, len)`. The kernel's page cache handles short-term
//! locality; we never hold large slices of payload in user-space.
//!
//! The in-memory `VecDeque<TagMeta>` is the canonical view of *what* is in the
//! buffer. Each entry is 32 bytes, so 10 minutes of audio+video indexes in
//! under 2 MB regardless of the bitrate of the underlying media.

use crate::sync::Mutex;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Seek, SeekFrom, Write};
use std::path::Path;
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug)]
pub struct TagMeta {
    pub seq: u64,    // monotonic id, stable across eviction
    pub offset: u64, // byte offset into the ring file
    pub len: u32,    // payload length in bytes
    /// Original input presentation time, promoted from the on-the-wire
    /// u32 milliseconds to u64 by the Controller's `expand_ts` wrap
    /// detector. RTMP timestamps wrap every ~49.7 days; storing u64 here
    /// means every comparison / subtraction in the rest of the codebase
    /// is naturally correct without `wrapping_sub` gymnastics. The
    /// downstream send path narrows back to u32 at the wire boundary.
    pub ts_ms: u64,
    pub kind: u8,     // FLV tag type: 8=audio, 9=video, 18=script-data
    pub is_idr: bool, // verified IDR keyframe (NAL type 5 present)
}

pub struct DiskRing {
    file: Mutex<File>,
    capacity: u64,
    inner: Mutex<RingInner>,

    // Sequence headers and onMetaData live outside the ring - they are tiny,
    // never expire, and must be resendable on every reconnect and every cut.
    //
    // Video seq headers are keyed by track id (0..255) to handle Enhanced
    // Broadcasting / multi-track streams: OBS sends one OneTrack-format
    // multi-track seq-header tag PER track at session start (with the
    // track id encoded in byte 6 of the payload), each carrying that
    // track's SPS/PPS. A single-slot cache would overwrite every
    // earlier track's config with the last one received - which is
    // exactly the bug we traced down to Twitch Inspector showing
    // per-track resolutions as "x" and the IVS transcoder pipeline
    // failing to bind after 60 s. Single-track tags occupy slot 0 and
    // the map degenerates to one entry, so the storage cost vs. the
    // old `Option` is negligible in the common case. BTreeMap so the
    // re-emit order is deterministic (track 0 first).
    pub video_seq_headers: Mutex<std::collections::BTreeMap<u8, Vec<u8>>>,
    /// Audio seq-headers keyed by track id (0..255). Same shape as
    /// `video_seq_headers` and for the same reason: OBS's VOD-audio
    /// feature (and Twitch's Enhanced Broadcasting multi-track audio
    /// in general) sends one OneTrack-format AudioSpecificConfig per
    /// audio track at session start. A single-slot cache would
    /// overwrite the live track's config with the VOD track's the
    /// instant the second one arrived - same failure mode that left
    /// the video tracks reading 'x' resolution in Twitch Inspector
    /// before we fixed it. Single-track audio sits in slot 0 and the
    /// map degenerates to one entry.
    pub audio_seq_headers: Mutex<std::collections::BTreeMap<u8, Vec<u8>>>,
    pub metadata: Mutex<Option<Vec<u8>>>,

    // Signaled by the producer whenever a new tag is appended. The egress
    // loop awaits this when it has caught up to the producer.
    pub on_append: Notify,
}

struct RingInner {
    write_cursor: u64,
    next_seq: u64,
    /// All indexed tags in append order (== monotonic by seq AND by ts).
    index: VecDeque<TagMeta>,
    /// SECONDARY index of just the IDR keyframes - same ordering, just
    /// filtered. Lets `find_idr_near` do a binary search on a
    /// small list (~1 IDR per 2 s of stream) instead of a linear walk
    /// over every audio + video tag (~150-300/s). Kept in lockstep with
    /// `index`: every IDR push_back also push_backs here; every eviction
    /// also pops the IDR-front if it matches by seq.
    idr_index: VecDeque<TagMeta>,
}

impl DiskRing {
    pub fn create(path: &Path, capacity: u64) -> Result<Self> {
        // Make sure the parent directory exists. With a hand-edited
        // config the user might point buffer_path at a path whose
        // parent doesn't exist yet - OpenOptions returns a bare
        // "path not found" io::Error in that case and the binary
        // exits silently under windows_subsystem=windows. Eagerly
        // creating the directory turns one class of cold-start
        // failure into a no-op.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        // On Windows the buffer file is briefly shared with whatever
        // touched it last (antivirus scan, Indexer, Explorer preview
        // pane, a still-shutting-down prior instance). The
        // `set_len(capacity)` call below maps to NtSetInformationFile
        // which fails with SHARING_VIOLATION (os error 32) when
        // another handle is still open. Retry a handful of times
        // with backoff so a 100 ms AV scan doesn't permanently
        // block the user's cold start.
        let mut file = open_with_retry(path)?;
        set_len_with_retry(&file, capacity)?;
        // Seek back to 0 so the first append writes at the start.
        // open_with_retry positions the cursor at end-of-file when
        // the file existed before.
        file.seek(SeekFrom::Start(0))?;
        let file = file; // freeze rebinding
        Ok(Self {
            file: Mutex::new(file),
            capacity,
            inner: Mutex::new(RingInner {
                write_cursor: 0,
                next_seq: 0,
                index: VecDeque::with_capacity(65_536),
                idr_index: VecDeque::with_capacity(2_048),
            }),
            video_seq_headers: Mutex::new(std::collections::BTreeMap::new()),
            audio_seq_headers: Mutex::new(std::collections::BTreeMap::new()),
            metadata: Mutex::new(None),
            on_append: Notify::new(),
        })
    }

    /// Append a tag's payload to the ring and index it.
    ///
    /// Sequence headers and onMetaData bypass the ring (stored in their own
    /// slots) so they are never evicted.
    pub fn append(
        &self,
        kind: u8,
        ts_ms: u64,
        payload: &[u8],
        is_idr: bool,
        is_seq_header: bool,
    ) -> Result<Option<u64>> {
        if is_seq_header {
            match kind {
                9 => {
                    // Cache per-track for multi-track streams. For single-track
                    // or ManyTracks-format multi-track tags this collapses to a
                    // single slot at key 0, matching the old single-Option
                    // behaviour. For OneTrack-format Enhanced Broadcasting
                    // streams (what OBS actually sends) each track id gets its
                    // own slot, so the re-emit on cuts / reconnects carries the
                    // SPS/PPS for every track Twitch's session expects.
                    let track_id = crate::h264::seq_header_track_id(payload);
                    self.video_seq_headers
                        .lock()
                        .insert(track_id, payload.to_vec());
                }
                8 => {
                    // Same per-track keying as video. For legacy AAC or
                    // single-track Enhanced-RTMP audio the helper returns
                    // 0, so the map degenerates to one slot at key 0.
                    let track_id = crate::h264::audio_seq_header_track_id(payload);
                    self.audio_seq_headers
                        .lock()
                        .insert(track_id, payload.to_vec());
                }
                _ => {}
            }
            return Ok(None);
        }
        if kind == 18 {
            *self.metadata.lock() = Some(payload.to_vec());
            return Ok(None);
        }

        // Reject tags larger than half the buffer outright - they cannot
        // coexist with any other tag without immediately evicting themselves.
        if (payload.len() as u64) > self.capacity / 2 {
            return Ok(None);
        }

        let len = payload.len() as u64;
        let mut inner = self.inner.lock();
        let offset = inner.write_cursor;
        let new_cursor = (offset + len) % self.capacity;
        let wraps = offset + len > self.capacity;

        // Evict any indexed tag whose stored byte range overlaps the bytes
        // we are about to overwrite. This is what keeps the index in sync
        // with the bytes on disk. The IDR-only secondary index is popped
        // in lockstep so binary-search lookups never return evicted IDRs.
        while let Some(front) = inner.index.front().copied() {
            if write_overlaps(offset, len, self.capacity, front.offset, front.len as u64) {
                inner.index.pop_front();
                if front.is_idr {
                    // Front of idr_index MUST be this same IDR - both
                    // queues are time-ordered and we only ever push at
                    // the back. Defensive `if let` keeps us robust to
                    // any future ordering invariant change.
                    if inner.idr_index.front().map(|m| m.seq) == Some(front.seq) {
                        inner.idr_index.pop_front();
                    }
                }
            } else {
                break;
            }
        }

        // Perform the disk write. We hold the file lock only for the duration
        // of the syscalls; the index lock is held for the whole append so the
        // index and write cursor advance atomically together.
        {
            let mut file = self.file.lock();
            if !wraps {
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(payload)?;
            } else {
                let first = (self.capacity - offset) as usize;
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(&payload[..first])?;
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&payload[first..])?;
            }
        }

        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.write_cursor = new_cursor;
        let meta = TagMeta {
            seq,
            offset,
            len: payload.len() as u32,
            ts_ms,
            kind,
            is_idr,
        };
        inner.index.push_back(meta);
        if is_idr && crate::h264::is_primary_video_idr(payload) {
            // Only the PRIMARY track's IDRs become cut candidates. Each
            // EB ladder rung has its own IDR cadence; OBS aligns them
            // all to the same encoder PTS, so we don't lose cut points,
            // we just stop landing on a non-primary rung's IDR where
            // the legacy decoder has no reference. See
            // `h264::is_primary_video_idr` for the full classification.
            inner.idr_index.push_back(meta);
        }
        drop(inner);

        self.on_append.notify_waiters();
        Ok(Some(seq))
    }

    /// Read the bytes of the tag at `seq` into the caller's reusable
    /// buffer. Returns `Ok(None)` if the tag has been evicted between
    /// when the caller obtained its meta and now - eviction-safe by
    /// design.
    ///
    /// Atomicity: holds the index lock for the full read, so a concurrent
    /// `append` (which takes the index lock first, then the file lock)
    /// cannot overwrite the bytes mid-read. Lock order matches `append`'s,
    /// so deadlock is impossible.
    pub fn try_read_seq(&self, seq: u64, buf: &mut Vec<u8>) -> Result<Option<()>> {
        let inner = self.inner.lock();
        let front = match inner.index.front() {
            Some(m) => m.seq,
            None => return Ok(None),
        };
        if seq < front {
            return Ok(None);
        }
        let idx = (seq - front) as usize;
        let meta = match inner.index.get(idx).copied() {
            Some(m) => m,
            None => return Ok(None),
        };

        buf.clear();
        buf.resize(meta.len as usize, 0);
        let end = meta.offset + meta.len as u64;
        let mut file = self.file.lock();
        if end <= self.capacity {
            file.seek(SeekFrom::Start(meta.offset))?;
            file.read_exact(buf)?;
        } else {
            let first = (self.capacity - meta.offset) as usize;
            file.seek(SeekFrom::Start(meta.offset))?;
            file.read_exact(&mut buf[..first])?;
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut buf[first..])?;
        }
        Ok(Some(()))
    }

    /// Find the entry with the given seq, returning its position in the
    /// VecDeque and the meta. `None` if it's been evicted or never existed.
    pub fn find_by_seq(&self, seq: u64) -> Option<(usize, TagMeta)> {
        let inner = self.inner.lock();
        let front = inner.index.front()?.seq;
        if seq < front {
            return None;
        }
        let idx = (seq - front) as usize;
        inner.index.get(idx).map(|m| (idx, *m))
    }

    pub fn front_seq(&self) -> Option<u64> {
        self.inner.lock().index.front().map(|m| m.seq)
    }

    pub fn latest_ts(&self) -> Option<u64> {
        self.inner.lock().index.back().map(|m| m.ts_ms)
    }

    pub fn oldest_ts(&self) -> Option<u64> {
        self.inner.lock().index.front().map(|m| m.ts_ms)
    }

    /// Pick the IDR closest to `target_ts` within ±`tolerance_ms`.
    /// Returns the keyframe that minimises `|ts - target_ts|`.
    ///
    /// Binary-search on the IDR-only secondary index - O(log n) over
    /// just the keyframes (~one IDR per 2 s of stream → ~300 entries
    /// for a 10-minute delay) instead of an O(n) walk over every
    /// audio + video tag (~90k entries).
    ///
    /// CHOICE: closest, not "prefer at-or-before". The earlier policy
    /// was "never undershoot the user's requested delay" - but that
    /// caused a cut loop. After a cut to an over-delayed IDR, the
    /// dead-band check would fire again (delivered > target by more
    /// than the dead band), and we'd cut to the SAME old IDR every
    /// 500 ms, replaying the same content. Closest-pick lets us land
    /// nearer the target on the first try and converges cleanly.
    pub fn find_idr_near(&self, target_ts: u64, tolerance_ms: u32) -> Option<TagMeta> {
        let inner = self.inner.lock();
        let tol = tolerance_ms as u64;
        let pos = inner.idr_index.partition_point(|m| m.ts_ms <= target_ts);
        let below = if pos > 0 {
            inner.idr_index.get(pos - 1).copied()
        } else {
            None
        };
        let above = inner.idr_index.get(pos).copied();

        let dist = |m: &TagMeta| -> u64 { m.ts_ms.abs_diff(target_ts) };

        match (below, above) {
            (Some(b), Some(a)) => {
                let bd = dist(&b);
                let ad = dist(&a);
                if bd <= ad {
                    if bd <= tol {
                        Some(b)
                    } else if ad <= tol {
                        Some(a)
                    } else {
                        None
                    }
                } else {
                    if ad <= tol {
                        Some(a)
                    } else if bd <= tol {
                        Some(b)
                    } else {
                        None
                    }
                }
            }
            (Some(b), None) => {
                if dist(&b) <= tol {
                    Some(b)
                } else {
                    None
                }
            }
            (None, Some(a)) => {
                if dist(&a) <= tol {
                    Some(a)
                } else {
                    None
                }
            }
            (None, None) => None,
        }
    }

    /// Return the most recent IDR (used to seed the egress state cleanly
    /// without walking the entire index entry-by-entry).
    pub fn newest_idr(&self) -> Option<TagMeta> {
        let inner = self.inner.lock();
        inner.index.iter().rev().find(|m| m.is_idr).copied()
    }

    /// Most recent IDR whose seq is strictly greater than `min_seq`.
    /// Used after a publisher reconnect to skip stale IDRs from the
    /// previous session that still happen to live in the ring.
    pub fn newest_idr_after(&self, min_seq: u64) -> Option<TagMeta> {
        let inner = self.inner.lock();
        inner
            .index
            .iter()
            .rev()
            .find(|m| m.is_idr && m.seq > min_seq)
            .copied()
    }

    /// OLDEST IDR whose seq is >= `min_seq`. Used by the egress pump
    /// after eviction skip-ahead - landing on a random P-frame would
    /// stream P-frames that reference absent reference frames and the
    /// player would show macroblocking until the next IDR. Returning
    /// the *earliest* IDR at or after the skip target loses the least
    /// content while keeping the decode chain valid.
    pub fn oldest_idr_at_or_after(&self, min_seq: u64) -> Option<TagMeta> {
        let inner = self.inner.lock();
        inner
            .index
            .iter()
            .find(|m| m.is_idr && m.seq >= min_seq)
            .copied()
    }

    /// Seq of the most recently appended tag, or None if the ring is empty.
    pub fn latest_seq(&self) -> Option<u64> {
        self.inner.lock().index.back().map(|m| m.seq)
    }

    /// Trim oldest indexed tags whose timestamp is older than
    /// `(current_ts - max_age_ms)`, never crossing `min_seq` (the
    /// consumer's last-acknowledged position - protects in-flight reads).
    ///
    /// One lock acquisition; pop_front is O(1). Bytes on disk are left
    /// untouched - the natural write-over-old-tags path reclaims them
    /// as the ring wraps. Trimming only the index lets us keep the
    /// buffer's *useful contents* exactly at the user's armed delay
    /// without juggling actual disk layout.
    pub fn trim_older_than(&self, max_age_ms: u32, current_ts: u64, min_seq: u64) {
        let mut inner = self.inner.lock();
        let cutoff = current_ts.saturating_sub(max_age_ms as u64);
        while let Some(front) = inner.index.front().copied() {
            // Never evict a tag the consumer is still reading or hasn't
            // reached yet - otherwise pace_and_send's read_tag could race
            // with a future overwrite of the same byte offset.
            if front.seq >= min_seq {
                break;
            }
            if front.ts_ms < cutoff {
                inner.index.pop_front();
                // Keep the IDR-only index in sync - same defensive front
                // check as the byte-overlap eviction path in `append`.
                if front.is_idr && inner.idr_index.front().map(|m| m.seq) == Some(front.seq) {
                    inner.idr_index.pop_front();
                }
            } else {
                break;
            }
        }
    }

    /// Drop every indexed tag. Called from `begin_publish` so a fresh
    /// OBS session, whose RTMP wire timestamps restart from ~0, does not
    /// get measured against the prior session's tags still sitting at
    /// the front of the index at much higher ts_ms values. Without this,
    /// `oldest_ts()` returns the stale session's first tag and
    /// `latest_ts() - oldest_ts()` saturates to 0, freezing
    /// `buffer_fill_ms` at zero for the new session even as tags pour
    /// in. `trim_older_than` cannot rescue it either, because
    /// `cutoff = current_ts - max_age` also saturates to 0 when
    /// `current_ts` is small.
    ///
    /// The seq counter and write cursor are deliberately preserved.
    /// Consumer seqs held by destinations stay valid (the new tags get
    /// seqs above the old high-water mark, so reads naturally advance
    /// onto fresh data), and the disk bytes get overwritten as new tags
    /// land.
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.index.clear();
        inner.idr_index.clear();
    }
}

/// Open the buffer file with read+write+create, retrying briefly on
/// transient Windows sharing violations (antivirus scan, prior-instance
/// still-shutting-down, Explorer preview pane). The retry window is
/// short and bounded - if the file is truly locked we surface the OS
/// error after ~1 s rather than spin indefinitely.
fn open_with_retry(path: &Path) -> Result<File> {
    let mut attempt = 0;
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
        {
            Ok(f) => return Ok(f),
            Err(e) if attempt < 5 && is_transient_lock(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Resize the buffer file to the configured capacity, retrying on
/// the same transient-lock errors as `open_with_retry`. set_len maps
/// to NtSetInformationFile on Windows; the kernel returns
/// SHARING_VIOLATION while another handle is still scanning the file.
fn set_len_with_retry(file: &File, capacity: u64) -> Result<()> {
    let mut attempt = 0;
    loop {
        match file.set_len(capacity) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 5 && is_transient_lock(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Windows ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33),
/// and the unix-side PermissionDenied mapping all indicate a file is
/// briefly held by another process. WouldBlock covers async-locked
/// handles. Anything else (NotFound, PermissionDenied without an OS
/// code we recognise, etc.) means a retry won't help.
fn is_transient_lock(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33))
        || matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        )
}

/// Does the write spanning `[w_off, w_off+w_len)` (mod cap) cover any byte
/// of the tag spanning `[t_off, t_off+t_len)` (mod cap)?
fn write_overlaps(w_off: u64, w_len: u64, cap: u64, t_off: u64, t_len: u64) -> bool {
    fn contains(start: u64, len: u64, cap: u64, pos: u64) -> bool {
        if len == 0 {
            return false;
        }
        let end = start + len;
        if end <= cap {
            pos >= start && pos < end
        } else {
            pos >= start || pos < end - cap
        }
    }
    contains(w_off, w_len, cap, t_off) || contains(t_off, t_len, cap, w_off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static UNIQ: AtomicU32 = AtomicU32::new(0);

    /// Build a video payload whose leading byte (`0x17`) matches the
    /// legacy-AVC keyframe shape that the v0.1.3 primary-IDR gate
    /// expects. Used everywhere a test wants to seed the IDR-only
    /// index - before v0.1.3 a generic `[0u8; N]` slice also landed
    /// in idr_index because the gate didn't exist.
    fn primary_idr_payload(len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        v.push(0x17);
        v.resize(len, 0);
        v
    }

    /// Test-scoped DiskRing in a fresh temp file. Deletes the file on drop
    /// so a run can re-create cleanly. Capacity must be > 2× any test tag.
    struct Tmp(DiskRing, std::path::PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.1);
        }
    }

    fn tmp(cap: u64) -> Tmp {
        let n = UNIQ.fetch_add(1, Ordering::SeqCst);
        let path = env::temp_dir().join(format!("ic-test-ring-{}-{}.buf", std::process::id(), n));
        let _ = std::fs::remove_file(&path);
        let ring = DiskRing::create(&path, cap).expect("create ring");
        Tmp(ring, path)
    }

    #[test]
    fn append_then_read_roundtrip() {
        let t = tmp(4096);
        let payload = b"hello world";
        let seq =
            t.0.append(9, 100, payload, false, false)
                .unwrap()
                .expect("seq");
        assert_eq!(seq, 0);

        let mut buf = Vec::new();
        let r = t.0.try_read_seq(seq, &mut buf).unwrap();
        assert!(r.is_some());
        assert_eq!(buf, payload);
    }

    #[test]
    fn seq_headers_bypass_the_ring() {
        let t = tmp(4096);
        let r = t.0.append(9, 0, b"AVCDecoderConfig", false, true).unwrap();
        assert!(r.is_none(), "seq headers must not return a ring seq");
        // Single-track seq headers cache under track id 0 - same slot
        // they used before the per-track refactor for multi-track.
        let map = t.0.video_seq_headers.lock();
        assert_eq!(map.get(&0), Some(&b"AVCDecoderConfig".to_vec()));
    }

    #[test]
    fn multitrack_seq_headers_cache_per_track_id() {
        // Two OneTrack-format Enhanced-RTMP seq-header tags for tracks
        // 0 and 4 must both survive in the cache - the bug we fixed.
        // The pre-change single-Option storage would have kept only
        // the last-received one, which is why Twitch Inspector showed
        // tracks 1-4 with resolution "x" during the EB rollout.
        let t = tmp(4096);
        let track_0 = vec![
            0x96, 0x00, 0x61, 0x76, 0x63, 0x31, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
        ];
        let track_4 = vec![
            0x96, 0x00, 0x61, 0x76, 0x63, 0x31, 0x04, 0xff, 0xfe, 0xfd, 0xfc, 0xfb,
        ];
        t.0.append(9, 0, &track_0, false, true).unwrap();
        t.0.append(9, 0, &track_4, false, true).unwrap();
        let map = t.0.video_seq_headers.lock();
        assert_eq!(map.get(&0), Some(&track_0));
        assert_eq!(map.get(&4), Some(&track_4));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn multitrack_audio_seq_headers_cache_per_track_id() {
        // OneTrack Enhanced-RTMP audio per OBS's flv_packet_audio_ex:
        //   byte 0: 0x95 (SoundFormat=9 | PacketType=Multitrack)
        //   byte 1: MultiTrackType=0 | NestedPacketType=0 (Seq)
        //   bytes 2..6: FourCC = "mp4a"
        //   byte 6:    TrackId
        //   bytes 7..: AudioSpecificConfig
        let t = tmp(4096);
        let live = vec![
            0x95, 0x00, b'm', b'p', b'4', b'a', 0x00, 0x12, 0x10, 0x56, 0xe5,
        ];
        let vod = vec![
            0x95, 0x00, b'm', b'p', b'4', b'a', 0x01, 0x12, 0x08, 0x44, 0x00,
        ];
        t.0.append(8, 0, &live, false, true).unwrap();
        t.0.append(8, 0, &vod, false, true).unwrap();
        let map = t.0.audio_seq_headers.lock();
        assert_eq!(map.get(&0), Some(&live));
        assert_eq!(map.get(&1), Some(&vod));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn legacy_aac_seq_header_caches_under_track_zero() {
        // Legacy AAC seq header has no track id; the helper returns 0
        // so the cache stays single-slot for the common case.
        let t = tmp(4096);
        let aac = vec![0xaf, 0x00, 0x12, 0x10, 0x56, 0xe5];
        t.0.append(8, 0, &aac, false, true).unwrap();
        let map = t.0.audio_seq_headers.lock();
        assert_eq!(map.get(&0), Some(&aac));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn metadata_kind_18_bypasses_the_ring() {
        let t = tmp(4096);
        let r = t.0.append(18, 0, b"onMetaData", false, false).unwrap();
        assert!(r.is_none());
        assert_eq!(*t.0.metadata.lock(), Some(b"onMetaData".to_vec()));
    }

    #[test]
    fn oversized_tag_is_rejected_silently() {
        // Capacity 4096, tag of 3000 bytes (> cap/2) - must be dropped.
        let t = tmp(4096);
        let big = vec![0u8; 3000];
        let r = t.0.append(9, 0, &big, false, false).unwrap();
        assert!(r.is_none());
        assert_eq!(t.0.front_seq(), None, "ring must remain empty");
    }

    #[test]
    fn wrapping_write_evicts_oldest_and_reads_correctly() {
        // Capacity 256. Write 6 × 80 = 480 bytes total - wraps the cursor
        // twice. Oldest tags get evicted; we read the latest one back.
        let t = tmp(256);
        let mut last_seq = 0;
        for i in 0..6 {
            let payload = vec![(i + 1) as u8; 80];
            last_seq =
                t.0.append(9, i as u64, &payload, false, false)
                    .unwrap()
                    .unwrap();
        }
        // Front is no longer seq=0
        let front = t.0.front_seq().unwrap();
        assert!(front > 0, "oldest tags must have been evicted");

        let mut buf = Vec::new();
        assert!(t.0.try_read_seq(last_seq, &mut buf).unwrap().is_some());
        assert_eq!(buf, vec![6u8; 80]);

        // The evicted oldest seq must read as None (it's gone).
        assert!(t.0.try_read_seq(0, &mut buf).unwrap().is_none());
    }

    #[test]
    fn find_idr_near_picks_closest() {
        let t = tmp(8192);
        // Three IDRs at ts 1000, 2000, 3000. v0.1.3 added a primary-
        // track gate on idr_index pushes - use the helper that emits
        // a payload classifying as primary so the cut-candidate index
        // actually fills up.
        let payload = primary_idr_payload(50);
        for ts in [1000u64, 2000, 3000] {
            t.0.append(9, ts, &payload, true, false).unwrap();
        }
        // Target 1900 with tolerance 500 → closest is 2000
        let m = t.0.find_idr_near(1900, 500).expect("found IDR");
        assert_eq!(m.ts_ms, 2000);

        // Target 4000, tolerance 500 → none in range
        assert!(t.0.find_idr_near(4000, 500).is_none());

        // Tolerance 1500 → 3000 is closest valid
        let m = t.0.find_idr_near(4000, 1500).expect("found IDR");
        assert_eq!(m.ts_ms, 3000);
    }

    #[test]
    fn write_overlaps_handles_both_wrap_cases() {
        // Linear case
        assert!(write_overlaps(50, 30, 1000, 60, 10));
        assert!(!write_overlaps(50, 30, 1000, 100, 10));
        // Write wraps, tag near start of buffer - tag at 5 is inside the
        // wrap segment [0, 20) (write spans 980..1000 ∪ 0..20).
        assert!(write_overlaps(980, 40, 1000, 5, 10));
        // Write wraps, tag past wrap segment - no overlap
        assert!(!write_overlaps(980, 40, 1000, 500, 10));
    }

    #[test]
    fn trim_older_than_respects_min_seq() {
        // Critical safety property: a consumer that's still reading seq N
        // must not have any tag at seq <= N evicted by trim. Otherwise
        // pace_and_send's `read_tag` races with overwrite-by-new-tag and
        // could deliver garbled bytes.
        let t = tmp(8192);
        // Push 6 tags at 100, 200, 300, 400, 500, 600 ms.
        let mut seqs = Vec::new();
        for i in 1..=6u32 {
            let s =
                t.0.append(9, (i * 100) as u64, &[0u8; 60], false, false)
                    .unwrap()
                    .unwrap();
            seqs.push(s);
        }
        // Consumer is on seq=2 (third tag, ts=300).
        // Trim everything older than max_age=100 ms from current_ts=600 →
        // cutoff = 500, so seqs 0..=3 (ts 100..400) would normally evict.
        // BUT min_seq=2 must protect seq 2 and onward.
        t.0.trim_older_than(100, 600, /*min_seq=*/ seqs[2]);

        // Front should be exactly seq[2], not later.
        assert_eq!(
            t.0.front_seq(),
            Some(seqs[2]),
            "trim must stop at min_seq, never evict an in-flight tag"
        );

        // Sanity: seq[0] and seq[1] (which were < min_seq AND older than
        // cutoff) should be gone.
        let mut buf = Vec::new();
        assert!(t.0.try_read_seq(seqs[0], &mut buf).unwrap().is_none());
        assert!(t.0.try_read_seq(seqs[1], &mut buf).unwrap().is_none());
        // And the protected seq[2] is still readable.
        assert!(t.0.try_read_seq(seqs[2], &mut buf).unwrap().is_some());
    }

    #[test]
    fn newest_idr_returns_the_last_one() {
        let t = tmp(4096);
        let idr = primary_idr_payload(30);
        t.0.append(9, 100, &idr, true, false).unwrap();
        t.0.append(9, 200, &[0u8; 30], false, false).unwrap();
        t.0.append(9, 300, &idr, true, false).unwrap();
        t.0.append(9, 400, &[0u8; 30], false, false).unwrap();
        let m = t.0.newest_idr().expect("has IDR");
        assert_eq!(m.ts_ms, 300);
        assert!(m.is_idr);
    }

    #[test]
    fn read_past_latest_seq_returns_none() {
        // A consumer asking for a seq that hasn't been written yet must
        // get None, not stale bytes from a wrap or a buffer overread.
        let t = tmp(4096);
        let s =
            t.0.append(9, 0, &[0xAB; 40], false, false)
                .unwrap()
                .unwrap();
        let mut buf = Vec::new();
        assert!(t.0.try_read_seq(s + 1, &mut buf).unwrap().is_none());
        assert!(t.0.try_read_seq(u64::MAX, &mut buf).unwrap().is_none());
    }

    #[test]
    fn tag_exactly_at_half_capacity_is_accepted() {
        // The rejection threshold is `> cap/2`. A tag exactly at cap/2
        // bytes must be accepted (boundary check, not off-by-one).
        let t = tmp(2048);
        let payload = vec![0u8; 1024]; // exactly cap/2
        let r = t.0.append(9, 100, &payload, false, false).unwrap();
        assert!(
            r.is_some(),
            "tag at cap/2 must be accepted, not the rejection branch"
        );
    }

    #[test]
    fn ring_survives_multiple_full_wraps() {
        // Cap 512. Write 30 tags of 80 bytes = 2400 bytes total, ~4.7×
        // the capacity. The ring must keep the most recent tag readable
        // and the index must not corrupt itself across multiple wraps.
        let t = tmp(512);
        let mut last_seq = 0;
        for i in 0..30u32 {
            let payload = vec![i as u8; 80];
            last_seq =
                t.0.append(9, (i * 100) as u64, &payload, false, false)
                    .unwrap()
                    .unwrap();
        }
        let mut buf = Vec::new();
        let r = t.0.try_read_seq(last_seq, &mut buf).unwrap();
        assert!(r.is_some());
        assert_eq!(
            buf.first(),
            Some(&29u8),
            "latest tag's bytes must be intact"
        );
        // Front seq is well past 0 (many evictions happened)
        assert!(t.0.front_seq().unwrap() > 20);
    }

    #[test]
    fn all_idr_queries_return_none_on_empty_ring() {
        // Every IDR-lookup variant must early-return None instead of
        // touching its (empty) underlying VecDeque. Collapsing into one
        // test because they all share the same trivial precondition.
        let t = tmp(2048);
        assert!(t.0.find_idr_near(1000, 500).is_none());
        assert!(t.0.newest_idr().is_none());
        assert!(t.0.newest_idr_after(0).is_none());
        assert!(t.0.oldest_idr_at_or_after(0).is_none());
    }

    #[test]
    fn find_idr_near_with_zero_tolerance_demands_exact_match() {
        let t = tmp(4096);
        t.0.append(9, 1000, &primary_idr_payload(40), true, false)
            .unwrap();
        // ts 999 with tolerance 0 → no match
        assert!(t.0.find_idr_near(999, 0).is_none());
        // ts 1000 with tolerance 0 → exact hit
        assert_eq!(t.0.find_idr_near(1000, 0).unwrap().ts_ms, 1000);
    }

    #[test]
    fn newest_idr_after_skips_stale_publisher_idrs() {
        // Models the publisher-reconnect case: there are old IDRs in the
        // ring from before the reconnect; we want only the new session's.
        let t = tmp(4096);
        let idr = primary_idr_payload(30);
        let old_seq = t.0.append(9, 100, &idr, true, false).unwrap().unwrap();
        let new_seq = t.0.append(9, 200, &idr, true, false).unwrap().unwrap();
        let m = t.0.newest_idr_after(old_seq).expect("found");
        assert_eq!(m.seq, new_seq);
        assert!(t.0.newest_idr_after(new_seq + 100).is_none());
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn ring(capacity: u64) -> (DiskRing, std::path::PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("ic-edge-ring-{nanos}-{capacity}.buf"));
        let _ = std::fs::remove_file(&path);
        let r = DiskRing::create(&path, capacity).expect("ring create");
        (r, path)
    }

    /// A tag too big for the ring to hold half of can never be stored
    /// without eating its own tail, so it is refused rather than corrupting
    /// the index. The ring has to stay usable afterwards.
    #[test]
    fn an_oversized_tag_is_refused_and_leaves_the_ring_usable() {
        let (r, path) = ring(64 * 1024);
        let half = 32 * 1024;

        assert_eq!(
            r.append(9, 1_000, &vec![0x27; half + 1], false, false)
                .expect("no io error"),
            None,
            "a tag over half the ring is refused"
        );
        assert_eq!(r.latest_ts(), None, "and nothing was indexed");

        // Exactly half still fits.
        assert!(r
            .append(9, 2_000, &vec![0x27; half], false, false)
            .expect("no io error")
            .is_some());
        assert_eq!(r.latest_ts(), Some(2_000));

        let _ = std::fs::remove_file(&path);
    }

    /// An empty payload is a degenerate tag, not a reason to panic or to
    /// desynchronise the write cursor from the index.
    #[test]
    fn an_empty_payload_does_not_break_the_cursor() {
        let (r, path) = ring(64 * 1024);
        r.append(9, 1_000, &[], false, false).expect("no io error");
        r.append(9, 2_000, &[0x27; 100], false, false)
            .expect("no io error");
        assert_eq!(r.latest_ts(), Some(2_000));
        let mut buf = Vec::new();
        let seq = r.latest_seq().expect("a populated ring");
        assert!(r
            .try_read_seq(seq, &mut buf)
            .expect("no io error")
            .is_some());
        assert_eq!(
            buf.len(),
            100,
            "the tag after an empty one reads back whole"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Trim takes a cutoff derived from the newest timestamp. When every tag
    /// is newer than it, nothing may be evicted - the guard that stops a
    /// clock jump from emptying the buffer.
    #[test]
    fn trim_keeps_everything_newer_than_its_cutoff() {
        let (r, path) = ring(64 * 1024);
        for i in 1..=10u64 {
            r.append(9, i * 100, &[0x27; 64], false, false)
                .expect("no io error");
        }
        r.trim_older_than(60_000, 1_000, u64::MAX);
        assert_eq!(r.oldest_ts(), Some(100), "nothing is older than the cutoff");

        // And a cutoff past everything empties it, which is what a bogus
        // future timestamp used to trigger.
        r.trim_older_than(0, 1_000_000, u64::MAX);
        assert_eq!(r.oldest_ts(), None);
        let _ = std::fs::remove_file(&path);
    }

    /// Writes wrap the file. The index has to keep pointing at bytes that
    /// still belong to the tag it names, across a full lap of the ring.
    #[test]
    fn tags_read_back_correctly_after_the_ring_laps() {
        let (r, path) = ring(16 * 1024);
        let mut last_seq = 0;
        for i in 1..=200u64 {
            let payload = vec![(i % 251) as u8; 300];
            if let Some(seq) = r.append(9, i * 10, &payload, false, false).expect("no io") {
                last_seq = seq;
            }
        }
        let mut buf = Vec::new();
        assert!(r
            .try_read_seq(last_seq, &mut buf)
            .expect("no io error")
            .is_some());
        assert_eq!(buf.len(), 300);
        assert!(
            buf.iter().all(|b| *b == buf[0]),
            "the newest tag read back as a mix of two writes"
        );
        let _ = std::fs::remove_file(&path);
    }
}
