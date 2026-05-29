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

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
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

    // Sequence headers and onMetaData live outside the ring — they are tiny,
    // never expire, and must be resendable on every reconnect and every cut.
    pub video_seq_header: Mutex<Option<Vec<u8>>>,
    pub audio_seq_header: Mutex<Option<Vec<u8>>>,
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
    /// SECONDARY index of just the IDR keyframes — same ordering, just
    /// filtered. Lets `find_idr_near` do a binary search on a
    /// small list (~1 IDR per 2 s of stream) instead of a linear walk
    /// over every audio + video tag (~150-300/s). Kept in lockstep with
    /// `index`: every IDR push_back also push_backs here; every eviction
    /// also pops the IDR-front if it matches by seq.
    idr_index: VecDeque<TagMeta>,
}

impl DiskRing {
    pub fn create(path: &Path, capacity: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.set_len(capacity)?;
        Ok(Self {
            file: Mutex::new(file),
            capacity,
            inner: Mutex::new(RingInner {
                write_cursor: 0,
                next_seq: 0,
                index: VecDeque::with_capacity(65_536),
                idr_index: VecDeque::with_capacity(2_048),
            }),
            video_seq_header: Mutex::new(None),
            audio_seq_header: Mutex::new(None),
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
                9 => *self.video_seq_header.lock().unwrap() = Some(payload.to_vec()),
                8 => *self.audio_seq_header.lock().unwrap() = Some(payload.to_vec()),
                _ => {}
            }
            return Ok(None);
        }
        if kind == 18 {
            *self.metadata.lock().unwrap() = Some(payload.to_vec());
            return Ok(None);
        }

        // Reject tags larger than half the buffer outright — they cannot
        // coexist with any other tag without immediately evicting themselves.
        if (payload.len() as u64) > self.capacity / 2 {
            return Ok(None);
        }

        let len = payload.len() as u64;
        let mut inner = self.inner.lock().unwrap();
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
                    // Front of idr_index MUST be this same IDR — both
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
            let mut file = self.file.lock().unwrap();
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
        if is_idr {
            inner.idr_index.push_back(meta);
        }
        drop(inner);

        self.on_append.notify_waiters();
        Ok(Some(seq))
    }

    /// Read the bytes of the tag at `seq` into the caller's reusable
    /// buffer. Returns `Ok(None)` if the tag has been evicted between
    /// when the caller obtained its meta and now — eviction-safe by
    /// design.
    ///
    /// Atomicity: holds the index lock for the full read, so a concurrent
    /// `append` (which takes the index lock first, then the file lock)
    /// cannot overwrite the bytes mid-read. Lock order matches `append`'s,
    /// so deadlock is impossible.
    pub fn try_read_seq(&self, seq: u64, buf: &mut Vec<u8>) -> Result<Option<()>> {
        let inner = self.inner.lock().unwrap();
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
        let mut file = self.file.lock().unwrap();
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
        let inner = self.inner.lock().unwrap();
        let front = inner.index.front()?.seq;
        if seq < front {
            return None;
        }
        let idx = (seq - front) as usize;
        inner.index.get(idx).map(|m| (idx, *m))
    }

    pub fn front_seq(&self) -> Option<u64> {
        self.inner.lock().unwrap().index.front().map(|m| m.seq)
    }

    pub fn latest_ts(&self) -> Option<u64> {
        self.inner.lock().unwrap().index.back().map(|m| m.ts_ms)
    }

    pub fn oldest_ts(&self) -> Option<u64> {
        self.inner.lock().unwrap().index.front().map(|m| m.ts_ms)
    }

    /// Pick the IDR closest to `target_ts` within ±`tolerance_ms`.
    /// Returns the keyframe that minimises `|ts - target_ts|`.
    ///
    /// Binary-search on the IDR-only secondary index — O(log n) over
    /// just the keyframes (~one IDR per 2 s of stream → ~300 entries
    /// for a 10-minute delay) instead of an O(n) walk over every
    /// audio + video tag (~90k entries).
    ///
    /// CHOICE: closest, not "prefer at-or-before". The earlier policy
    /// was "never undershoot the user's requested delay" — but that
    /// caused a cut loop. After a cut to an over-delayed IDR, the
    /// dead-band check would fire again (delivered > target by more
    /// than the dead band), and we'd cut to the SAME old IDR every
    /// 500 ms, replaying the same content. Closest-pick lets us land
    /// nearer the target on the first try and converges cleanly.
    pub fn find_idr_near(&self, target_ts: u64, tolerance_ms: u32) -> Option<TagMeta> {
        let inner = self.inner.lock().unwrap();
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
        let inner = self.inner.lock().unwrap();
        inner.index.iter().rev().find(|m| m.is_idr).copied()
    }

    /// Most recent IDR whose seq is strictly greater than `min_seq`.
    /// Used after a publisher reconnect to skip stale IDRs from the
    /// previous session that still happen to live in the ring.
    pub fn newest_idr_after(&self, min_seq: u64) -> Option<TagMeta> {
        let inner = self.inner.lock().unwrap();
        inner
            .index
            .iter()
            .rev()
            .find(|m| m.is_idr && m.seq > min_seq)
            .copied()
    }

    /// OLDEST IDR whose seq is >= `min_seq`. Used by the egress pump
    /// after eviction skip-ahead — landing on a random P-frame would
    /// stream P-frames that reference absent reference frames and the
    /// player would show macroblocking until the next IDR. Returning
    /// the *earliest* IDR at or after the skip target loses the least
    /// content while keeping the decode chain valid.
    pub fn oldest_idr_at_or_after(&self, min_seq: u64) -> Option<TagMeta> {
        let inner = self.inner.lock().unwrap();
        inner
            .index
            .iter()
            .find(|m| m.is_idr && m.seq >= min_seq)
            .copied()
    }

    /// Seq of the most recently appended tag, or None if the ring is empty.
    pub fn latest_seq(&self) -> Option<u64> {
        self.inner.lock().unwrap().index.back().map(|m| m.seq)
    }

    /// Trim oldest indexed tags whose timestamp is older than
    /// `(current_ts - max_age_ms)`, never crossing `min_seq` (the
    /// consumer's last-acknowledged position — protects in-flight reads).
    ///
    /// One lock acquisition; pop_front is O(1). Bytes on disk are left
    /// untouched — the natural write-over-old-tags path reclaims them
    /// as the ring wraps. Trimming only the index lets us keep the
    /// buffer's *useful contents* exactly at the user's armed delay
    /// without juggling actual disk layout.
    pub fn trim_older_than(&self, max_age_ms: u32, current_ts: u64, min_seq: u64) {
        let mut inner = self.inner.lock().unwrap();
        let cutoff = current_ts.saturating_sub(max_age_ms as u64);
        while let Some(front) = inner.index.front().copied() {
            // Never evict a tag the consumer is still reading or hasn't
            // reached yet — otherwise pace_and_send's read_tag could race
            // with a future overwrite of the same byte offset.
            if front.seq >= min_seq {
                break;
            }
            if front.ts_ms < cutoff {
                inner.index.pop_front();
                // Keep the IDR-only index in sync — same defensive front
                // check as the byte-overlap eviction path in `append`.
                if front.is_idr && inner.idr_index.front().map(|m| m.seq) == Some(front.seq) {
                    inner.idr_index.pop_front();
                }
            } else {
                break;
            }
        }
    }
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
        assert_eq!(
            *t.0.video_seq_header.lock().unwrap(),
            Some(b"AVCDecoderConfig".to_vec())
        );
    }

    #[test]
    fn metadata_kind_18_bypasses_the_ring() {
        let t = tmp(4096);
        let r = t.0.append(18, 0, b"onMetaData", false, false).unwrap();
        assert!(r.is_none());
        assert_eq!(*t.0.metadata.lock().unwrap(), Some(b"onMetaData".to_vec()));
    }

    #[test]
    fn oversized_tag_is_rejected_silently() {
        // Capacity 4096, tag of 3000 bytes (> cap/2) — must be dropped.
        let t = tmp(4096);
        let big = vec![0u8; 3000];
        let r = t.0.append(9, 0, &big, false, false).unwrap();
        assert!(r.is_none());
        assert_eq!(t.0.front_seq(), None, "ring must remain empty");
    }

    #[test]
    fn wrapping_write_evicts_oldest_and_reads_correctly() {
        // Capacity 256. Write 6 × 80 = 480 bytes total — wraps the cursor
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
        // Three IDRs at ts 1000, 2000, 3000
        for ts in [1000u64, 2000, 3000] {
            t.0.append(9, ts, &[0u8; 50], true, false).unwrap();
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
        // Write wraps, tag near start of buffer — tag at 5 is inside the
        // wrap segment [0, 20) (write spans 980..1000 ∪ 0..20).
        assert!(write_overlaps(980, 40, 1000, 5, 10));
        // Write wraps, tag past wrap segment — no overlap
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
        t.0.append(9, 100, &[0u8; 30], true, false).unwrap();
        t.0.append(9, 200, &[0u8; 30], false, false).unwrap();
        t.0.append(9, 300, &[0u8; 30], true, false).unwrap();
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
        t.0.append(9, 1000, &[0u8; 40], true, false).unwrap();
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
        let old_seq =
            t.0.append(9, 100, &[0u8; 30], true, false)
                .unwrap()
                .unwrap();
        let new_seq =
            t.0.append(9, 200, &[0u8; 30], true, false)
                .unwrap()
                .unwrap();
        let m = t.0.newest_idr_after(old_seq).expect("found");
        assert_eq!(m.seq, new_seq);
        assert!(t.0.newest_idr_after(new_seq + 100).is_none());
    }
}
