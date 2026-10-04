//! RTMP chunk stream reader and writer.
//!
//! A "message" is the unit of logical communication (audio frame, video
//! frame, AMF command, control). Messages are sliced into "chunks" of at
//! most `chunk_size` bytes (default 128, almost always renegotiated to
//! ~4096) and interleaved on the TCP socket per-CSID (chunk stream id).
//!
//! Chunk headers come in four `fmt` flavors that compress fields by
//! referencing the previous chunk on the same CSID:
//!   fmt 0 - full 11-byte header: timestamp, length, type, msg-stream-id
//!   fmt 1 -  7 bytes: timestamp delta, length, type   (reuse msg-stream-id)
//!   fmt 2 -  3 bytes: timestamp delta                (reuse length/type/msid)
//!   fmt 3 -  0 bytes: continuation                   (reuse everything)
//!
//! Timestamps that overflow 24 bits use an extended 32-bit field appended
//! after the message header (and on all continuation chunks for that
//! message). This is the single most-broken-in-practice corner of RTMP.

use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::io::{self, ErrorKind};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const DEFAULT_CHUNK_SIZE: usize = 128;

/// Most bytes of unfinished messages a connection may hold across all its
/// chunk streams. A publisher has one or two messages in flight (a large
/// keyframe on one stream, audio on another); each can be up to 16 MB, so
/// this leaves room for several. Without a total, a peer could open
/// thousands of chunk streams and park a nearly finished 16 MB message on
/// each, holding memory without ever publishing.
pub const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Message {
    pub timestamp: u32,
    pub type_id: u8,
    pub stream_id: u32,
    pub payload: Bytes,
}

struct CsState {
    timestamp: u32,        // absolute timestamp of the in-progress message
    timestamp_delta: u32,  // last delta (reused by fmt 3 within a message run)
    last_had_ext_ts: bool, // continuation chunks repeat the ext ts iff so
    length: u32,
    type_id: u8,
    stream_id: u32,
    buf: BytesMut,   // accumulating payload across chunks
    receiving: bool, // true while we have partial bytes for a message
}

impl Default for CsState {
    fn default() -> Self {
        Self {
            timestamp: 0,
            timestamp_delta: 0,
            last_had_ext_ts: false,
            length: 0,
            type_id: 0,
            stream_id: 0,
            buf: BytesMut::new(),
            receiving: false,
        }
    }
}

/// Parsed message-header for one chunk, keyed off the wire `fmt`. The
/// enum exists purely to thread the parsed fields out of the async
/// read code (which holds `&mut self`) into the per-CSID state update
/// (which holds `&mut self.streams`) without a tuple-of-Options.
enum ChunkHeader {
    Fmt0 {
        timestamp: u32,
        length: u32,
        type_id: u8,
        msid: u32,
        ext_ts_present: bool,
    },
    Fmt1 {
        delta: u32,
        length: u32,
        type_id: u8,
        ext_ts_present: bool,
    },
    Fmt2 {
        delta: u32,
        ext_ts_present: bool,
    },
    Fmt3,
}

pub struct ChunkReader<R> {
    inner: R,
    chunk_size: usize,
    streams: HashMap<u32, CsState>,
    /// Bytes of unfinished messages across all of `streams`, capped at
    /// `MAX_PENDING_BYTES`.
    pending: usize,
    /// Total wire bytes consumed from the peer since the connection
    /// opened - chunk headers + payload + extended timestamps + control
    /// messages. Used to emit RTMP Acknowledgement (BYTES_READ_REPORT,
    /// msg type 3) when we cross the peer-declared window threshold.
    /// Wraps at 2^64 (~146 years at 4 GB/s) so saturating arithmetic
    /// isn't needed.
    bytes_in: u64,
    /// Window acknowledgement size declared by the peer (msg type 5).
    /// librtmp's threshold rule fires when `bytes_in - last_ack >
    /// window/10`. We default to 2_500_000 (Twitch's and most CDNs'
    /// default) so the first ack fires at a sane interval even if the
    /// peer never sent a Window Ack Size message.
    window_ack_size: u32,
    /// Cumulative byte count we last reported in an Acknowledgement.
    /// The next ack fires when `bytes_in` exceeds this plus
    /// `window_ack_size / 10`.
    bytes_in_at_last_ack: u64,
}

impl<R: AsyncReadExt + Unpin> ChunkReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            chunk_size: DEFAULT_CHUNK_SIZE,
            streams: HashMap::with_capacity(8),
            pending: 0,
            bytes_in: 0,
            window_ack_size: 2_500_000,
            bytes_in_at_last_ack: 0,
        }
    }

    pub fn set_chunk_size(&mut self, size: usize) {
        self.chunk_size = size.clamp(1, 0xFF_FFFF);
    }

    /// If we have received more than `window/10` bytes since the last
    /// Acknowledgement we sent, returns the current cumulative byte
    /// count (truncated to u32 per spec) and advances the watermark.
    /// Caller is responsible for actually writing the ack message -
    /// the reader cannot write because it doesn't own the writer half
    /// of the socket. librtmp's exact rule, reproduced here so the
    /// behaviour is byte-for-byte compatible with what every OBS-class
    /// publisher / consumer expects from a well-behaved RTMP peer.
    pub fn take_pending_ack(&mut self) -> Option<u32> {
        let threshold = self.bytes_in_at_last_ack + (self.window_ack_size as u64) / 10;
        if self.bytes_in > threshold {
            self.bytes_in_at_last_ack = self.bytes_in;
            // RTMP BYTES_READ_REPORT is a u32 field - let it wrap.
            Some(self.bytes_in as u32)
        } else {
            None
        }
    }

    /// Internal `read_exact` wrapper that updates the wire-bytes
    /// counter. Used for every header byte and payload byte so the
    /// `bytes_in` accounting matches "everything we pulled off the
    /// socket" (which is what the peer's window-ack-size threshold is
    /// counting on its side, mirror-image).
    async fn read_exact_counted(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact(buf).await?;
        self.bytes_in += buf.len() as u64;
        Ok(())
    }

    async fn read_u32_be_counted(&mut self) -> io::Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact_counted(&mut b).await?;
        Ok(u32::from_be_bytes(b))
    }

    /// Parse the per-fmt chunk message header from the wire. Returns a
    /// `ChunkHeader` enum the caller dispatches on. Reads (and counts)
    /// all header bytes including any extended timestamp; the per-CSID
    /// state in `streams` is left untouched.
    async fn read_chunk_header(&mut self, fmt: u8, csid: u32) -> io::Result<ChunkHeader> {
        match fmt {
            0 => {
                let mut h = [0u8; 11];
                self.read_exact_counted(&mut h).await?;
                let ts24 = u24_be(&h[0..3]);
                let length = u24_be(&h[3..6]);
                let type_id = h[6];
                let msid = u32::from_le_bytes([h[7], h[8], h[9], h[10]]);
                let ext_ts_present = ts24 == 0x00FF_FFFF;
                let timestamp = if ext_ts_present {
                    self.read_u32_be_counted().await?
                } else {
                    ts24
                };
                Ok(ChunkHeader::Fmt0 {
                    timestamp,
                    length,
                    type_id,
                    msid,
                    ext_ts_present,
                })
            }
            1 => {
                let mut h = [0u8; 7];
                self.read_exact_counted(&mut h).await?;
                let delta24 = u24_be(&h[0..3]);
                let length = u24_be(&h[3..6]);
                let type_id = h[6];
                let ext_ts_present = delta24 == 0x00FF_FFFF;
                let delta = if ext_ts_present {
                    self.read_u32_be_counted().await?
                } else {
                    delta24
                };
                Ok(ChunkHeader::Fmt1 {
                    delta,
                    length,
                    type_id,
                    ext_ts_present,
                })
            }
            2 => {
                let mut h = [0u8; 3];
                self.read_exact_counted(&mut h).await?;
                let delta24 = u24_be(&h[0..3]);
                let ext_ts_present = delta24 == 0x00FF_FFFF;
                let delta = if ext_ts_present {
                    self.read_u32_be_counted().await?
                } else {
                    delta24
                };
                Ok(ChunkHeader::Fmt2 {
                    delta,
                    ext_ts_present,
                })
            }
            3 => {
                // Continuation. Whether to consume an extended-timestamp
                // field depends on what the last header on this CSID
                // indicated, so we peek (read-only) into streams first.
                let needs_ext = self
                    .streams
                    .get(&csid)
                    .map(|st| st.last_had_ext_ts)
                    .unwrap_or(false);
                if needs_ext {
                    let _ = self.read_u32_be_counted().await?;
                }
                Ok(ChunkHeader::Fmt3)
            }
            _ => unreachable!(),
        }
    }

    /// Read until one full RTMP message is reassembled, then yield it.
    /// Control messages (Set Chunk Size = 1) are handled in-band so the
    /// caller never has to think about them.
    pub async fn read_message(&mut self) -> io::Result<Message> {
        loop {
            // --- Basic header (1-3 bytes): fmt(2) + csid(6/14/22) ---
            let mut b0 = [0u8; 1];
            self.read_exact_counted(&mut b0).await?;
            let fmt = (b0[0] >> 6) & 0x03;
            let csid_low = b0[0] & 0x3F;
            let csid: u32 = match csid_low {
                0 => {
                    let mut b = [0u8; 1];
                    self.read_exact_counted(&mut b).await?;
                    b[0] as u32 + 64
                }
                1 => {
                    let mut b = [0u8; 2];
                    self.read_exact_counted(&mut b).await?;
                    (b[1] as u32) * 256 + b[0] as u32 + 64
                }
                n => n as u32,
            };

            // --- Message header (0/3/7/11 bytes by fmt) ---
            // Read it first (no borrow on self.streams in flight), then
            // dispatch into the per-CSID state. The two phases exist so
            // the async byte-counting reads can hold &mut self exclusively.
            let header = self.read_chunk_header(fmt, csid).await?;
            let chunk_size = self.chunk_size;
            let st = self.streams.entry(csid).or_default();
            match header {
                ChunkHeader::Fmt0 {
                    timestamp,
                    length,
                    type_id,
                    msid,
                    ext_ts_present,
                } => {
                    st.timestamp = timestamp;
                    st.timestamp_delta = timestamp;
                    st.length = length;
                    st.type_id = type_id;
                    st.stream_id = msid;
                    st.last_had_ext_ts = ext_ts_present;
                    self.pending = self.pending.saturating_sub(st.buf.len());
                    st.buf = message_buf(length, chunk_size);
                    st.receiving = true;
                }
                ChunkHeader::Fmt1 {
                    delta,
                    length,
                    type_id,
                    ext_ts_present,
                } => {
                    st.timestamp = st.timestamp.wrapping_add(delta);
                    st.timestamp_delta = delta;
                    st.length = length;
                    st.type_id = type_id;
                    st.last_had_ext_ts = ext_ts_present;
                    self.pending = self.pending.saturating_sub(st.buf.len());
                    st.buf = message_buf(length, chunk_size);
                    st.receiving = true;
                }
                ChunkHeader::Fmt2 {
                    delta,
                    ext_ts_present,
                } => {
                    st.timestamp = st.timestamp.wrapping_add(delta);
                    st.timestamp_delta = delta;
                    st.last_had_ext_ts = ext_ts_present;
                    self.pending = self.pending.saturating_sub(st.buf.len());
                    st.buf = message_buf(st.length, chunk_size);
                    st.receiving = true;
                }
                ChunkHeader::Fmt3 => {
                    if !st.receiving {
                        // Whole-message replay: same header as before,
                        // so the absolute timestamp advances by the
                        // last delta and the buffer restarts.
                        st.timestamp = st.timestamp.wrapping_add(st.timestamp_delta);
                        st.buf = message_buf(st.length, chunk_size);
                        st.receiving = true;
                    }
                }
            }

            // --- Payload chunk ---
            let remaining = st.length as usize - st.buf.len();
            let to_read = remaining.min(self.chunk_size);
            let start = st.buf.len();
            st.buf.resize(start + to_read, 0);
            // Re-borrow `inner` directly here: we can't call
            // read_exact_counted while st is also borrowed from
            // self.streams. We bump the byte counter manually afterwards.
            self.inner.read_exact(&mut st.buf[start..]).await?;
            self.bytes_in += to_read as u64;
            self.pending += to_read;
            if self.pending > MAX_PENDING_BYTES {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "too much unfinished message data on this connection",
                ));
            }

            if st.buf.len() as u32 == st.length {
                let msg = Message {
                    timestamp: st.timestamp,
                    type_id: st.type_id,
                    stream_id: st.stream_id,
                    payload: st.buf.split().freeze(),
                };
                st.receiving = false;
                self.pending = self.pending.saturating_sub(msg.payload.len());

                // Handle protocol-control messages in-band so callers
                // only ever see semantic messages.
                match msg.type_id {
                    1 => {
                        // Set Chunk Size
                        if msg.payload.len() >= 4 {
                            let new_size = u32::from_be_bytes([
                                msg.payload[0],
                                msg.payload[1],
                                msg.payload[2],
                                msg.payload[3],
                            ]) as usize;
                            self.set_chunk_size(new_size);
                        }
                        continue;
                    }
                    2 => {
                        // Abort Message: drop the partial message on the
                        // named csid, so the next fmt-3 there starts a new
                        // message instead of being appended to stale bytes.
                        if msg.payload.len() >= 4 {
                            let aborted = u32::from_be_bytes([
                                msg.payload[0],
                                msg.payload[1],
                                msg.payload[2],
                                msg.payload[3],
                            ]);
                            if let Some(st) = self.streams.get_mut(&aborted) {
                                self.pending = self.pending.saturating_sub(st.buf.len());
                                st.buf = BytesMut::new();
                                st.receiving = false;
                            }
                        }
                        continue;
                    }
                    3 => continue, // Acknowledgement
                    5 => {
                        // Window Acknowledgement Size from the peer -
                        // record it so our `take_pending_ack` threshold
                        // uses the peer-declared value rather than the
                        // 2.5 MB default. Zero is meaningless per spec;
                        // ignore it.
                        if msg.payload.len() >= 4 {
                            let n = u32::from_be_bytes([
                                msg.payload[0],
                                msg.payload[1],
                                msg.payload[2],
                                msg.payload[3],
                            ]);
                            if n > 0 {
                                self.window_ack_size = n;
                            }
                        }
                        continue;
                    }
                    6 => continue, // Set Peer Bandwidth - informational only
                    _ => return Ok(msg),
                }
            }
        }
    }
}

pub struct ChunkWriter<W> {
    inner: W,
    chunk_size: usize,
    out_buf: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> ChunkWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            chunk_size: DEFAULT_CHUNK_SIZE,
            out_buf: Vec::with_capacity(64 * 1024),
        }
    }

    pub fn set_chunk_size(&mut self, size: usize) {
        self.chunk_size = size.clamp(1, 0xFF_FFFF);
    }

    /// Send "Set Chunk Size" to the peer and update our own outbound size.
    pub async fn send_set_chunk_size(&mut self, size: u32) -> io::Result<()> {
        let mut payload = [0u8; 4];
        payload.copy_from_slice(&size.to_be_bytes());
        // Sent on CSID 2 with msg type 1, message stream 0.
        self.write_message(2, 0, 1, 0, &payload).await?;
        self.set_chunk_size(size as usize);
        Ok(())
    }

    /// Send an RTMP Acknowledgement (BYTES_READ_REPORT, msg type 3) on
    /// the control channel. `seq` is the cumulative wire-bytes-received
    /// count, truncated to u32 per spec. Real-world peers (Twitch's
    /// edges, OBS's librtmp consumer) silently disconnect a stalled
    /// receiver when their `window_ack_size/10` threshold is breached
    /// without an ack landing - so callers should fire this from the
    /// same task that drives the chunk reader, after every read.
    pub async fn send_ack(&mut self, seq: u32) -> io::Result<()> {
        self.write_message(2, 0, 3, 0, &seq.to_be_bytes()).await
    }

    /// Write a complete RTMP message, fragmenting into chunks as needed.
    /// Emits one fmt-0 chunk followed by fmt-3 continuations - the
    /// simplest and most broadly compatible pattern.
    pub async fn write_message(
        &mut self,
        csid: u32,
        timestamp: u32,
        type_id: u8,
        stream_id: u32,
        payload: &[u8],
    ) -> io::Result<()> {
        if payload.len() > 0x00FF_FFFF {
            return Err(io::Error::new(ErrorKind::InvalidInput, "payload too large"));
        }
        self.out_buf.clear();

        let ext_ts = timestamp >= 0x00FF_FFFF;
        let ts24 = if ext_ts { 0x00FF_FFFF } else { timestamp };

        // First chunk: fmt 0, full header.
        write_basic_header(&mut self.out_buf, 0, csid);
        // Message header (11 bytes)
        push_u24_be(&mut self.out_buf, ts24);
        push_u24_be(&mut self.out_buf, payload.len() as u32);
        self.out_buf.push(type_id);
        // Message stream ID is little-endian here.
        self.out_buf.extend_from_slice(&stream_id.to_le_bytes());
        if ext_ts {
            self.out_buf.extend_from_slice(&timestamp.to_be_bytes());
        }

        let first_chunk = payload.len().min(self.chunk_size);
        self.out_buf.extend_from_slice(&payload[..first_chunk]);
        let mut written = first_chunk;

        // Continuation chunks: fmt 3, 0-byte message header. Per spec, if
        // the message used an extended timestamp, every continuation chunk
        // must repeat that extended timestamp.
        while written < payload.len() {
            write_basic_header(&mut self.out_buf, 3, csid);
            if ext_ts {
                self.out_buf.extend_from_slice(&timestamp.to_be_bytes());
            }
            let n = (payload.len() - written).min(self.chunk_size);
            self.out_buf
                .extend_from_slice(&payload[written..written + n]);
            written += n;
        }

        self.inner.write_all(&self.out_buf).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }

    /// Close the sending side (TCP FIN). The peer still gets everything
    /// already written, and this side can keep reading until it closes.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }
}

fn write_basic_header(out: &mut Vec<u8>, fmt: u8, csid: u32) {
    let f = (fmt & 0x03) << 6;
    if csid < 64 {
        out.push(f | csid as u8);
    } else if csid < 320 {
        out.push(f);
        out.push((csid - 64) as u8);
    } else {
        out.push(f | 0x01);
        let v = csid - 64;
        out.push((v & 0xFF) as u8);
        out.push(((v >> 8) & 0xFF) as u8);
    }
}

/// Empty buffer for a message the peer declares to be `length` bytes.
///
/// Reserves at most one chunk. The length is only the peer's claim, read
/// before `publish` is gated, and every csid (up to 65599 of them) holds its
/// own partial message: reserving the full 24-bit length up front let each
/// ~140-byte header pin 16 MB. Capped here, the buffer grows only as chunks
/// actually arrive, so memory tracks bytes received.
fn message_buf(length: u32, chunk_size: usize) -> BytesMut {
    BytesMut::with_capacity((length as usize).min(chunk_size))
}

fn push_u24_be(out: &mut Vec<u8>, v: u32) {
    out.push(((v >> 16) & 0xFF) as u8);
    out.push(((v >> 8) & 0xFF) as u8);
    out.push((v & 0xFF) as u8);
}

fn u24_be(b: &[u8]) -> u32 {
    ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // A `Cursor<Vec<u8>>` is a tokio `AsyncRead`, so it feeds the reader
    // straight from an in-memory wire capture - no socket, no task.
    fn reader(bytes: Vec<u8>) -> ChunkReader<Cursor<Vec<u8>>> {
        ChunkReader::new(Cursor::new(bytes))
    }

    // Encode one full message with the real writer, then decode it with the
    // real reader. Proves the encoder and decoder agree on the wire format -
    // the single most valuable invariant for this module.
    async fn roundtrip(csid: u32, ts: u32, type_id: u8, msid: u32, payload: &[u8]) -> Message {
        let mut wire: Vec<u8> = Vec::new();
        {
            // `&mut Vec<u8>` is a tokio `AsyncWrite`; scoping the borrow lets
            // us inspect `wire` afterwards without a test-only accessor.
            let mut w = ChunkWriter::new(&mut wire);
            w.write_message(csid, ts, type_id, msid, payload)
                .await
                .unwrap();
        }
        reader(wire).read_message().await.unwrap()
    }

    fn u24(v: u32) -> [u8; 3] {
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    // --- Hand-built chunks the WRITER never emits ---------------------
    // ChunkWriter only ever produces fmt-0 + fmt-3 continuations, so the
    // reader's fmt-1 / fmt-2 / fmt-3-replay decode paths are exercised only
    // here - even though OBS and Twitch send them constantly. Each helper
    // assumes csid < 64 (1-byte basic header), ts/delta < 0xFF_FFFF (no
    // extended timestamp), and a payload that fits in one chunk.
    fn fmt0(csid: u8, ts: u32, type_id: u8, msid: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![csid & 0x3F]; // fmt bits are 0
        b.extend_from_slice(&u24(ts));
        b.extend_from_slice(&u24(payload.len() as u32));
        b.push(type_id);
        b.extend_from_slice(&msid.to_le_bytes());
        b.extend_from_slice(payload);
        b
    }

    /// fmt-0 header declaring `declared` bytes (msid 1), followed by only
    /// the first chunk's bytes: the rest must come as fmt-3 continuations.
    fn fmt0_first_chunk(csid: u8, ts: u32, type_id: u8, declared: u32, first: &[u8]) -> Vec<u8> {
        let mut b = fmt0(csid, ts, type_id, 1, first);
        b[4..7].copy_from_slice(&u24(declared));
        b
    }

    fn fmt1(csid: u8, delta: u32, type_id: u8, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![(1 << 6) | (csid & 0x3F)];
        b.extend_from_slice(&u24(delta));
        b.extend_from_slice(&u24(payload.len() as u32));
        b.push(type_id);
        b.extend_from_slice(payload);
        b
    }

    fn fmt2(csid: u8, delta: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![(2 << 6) | (csid & 0x3F)];
        b.extend_from_slice(&u24(delta));
        b.extend_from_slice(payload);
        b
    }

    fn fmt3(csid: u8, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![(3 << 6) | (csid & 0x3F)];
        b.extend_from_slice(payload);
        b
    }

    #[tokio::test]
    async fn roundtrip_small_message() {
        let payload = b"hello-rtmp";
        let msg = roundtrip(6, 1000, 9, 1, payload).await;
        assert_eq!(msg.timestamp, 1000);
        assert_eq!(msg.type_id, 9);
        assert_eq!(msg.stream_id, 1);
        assert_eq!(&msg.payload[..], payload);
    }

    #[tokio::test]
    async fn roundtrip_fragmented_message() {
        // 300 bytes at the default 128-byte chunk size forces one fmt-0
        // header chunk plus two fmt-3 continuations; the reader must
        // reassemble them back into the exact original payload.
        let payload: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let msg = roundtrip(6, 42, 8, 3, &payload).await;
        assert_eq!(msg.payload.len(), 300);
        assert_eq!(&msg.payload[..], &payload[..]);
    }

    #[tokio::test]
    async fn writer_emits_spec_bytes_for_an_extended_timestamp_at_the_boundary() {
        // Round trips can't catch a mistake the writer and reader share, so
        // pin the wire bytes. Exactly 0xFF_FFFF is the first value that
        // needs the extended field (the 24-bit slot then holds the 0xFFFFFF
        // marker), message stream id is the one little-endian field, and the
        // fmt-3 continuation repeats the 4-byte extended timestamp.
        let payload: Vec<u8> = (0..200u32).map(|i| i as u8).collect();
        let mut wire: Vec<u8> = Vec::new();
        ChunkWriter::new(&mut wire)
            .write_message(6, 0x00FF_FFFF, 9, 1, &payload)
            .await
            .unwrap();

        let mut expected = vec![0x06, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0xC8, 0x09];
        expected.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // msid 1, LE
        expected.extend_from_slice(&[0x00, 0xFF, 0xFF, 0xFF]); // ext ts, BE
        expected.extend_from_slice(&payload[..128]);
        expected.extend_from_slice(&[0xC6, 0x00, 0xFF, 0xFF, 0xFF]); // fmt 3 + ext ts
        expected.extend_from_slice(&payload[128..]);
        assert_eq!(wire, expected);

        // One below the boundary still fits in 24 bits: no extended field.
        let mut wire: Vec<u8> = Vec::new();
        ChunkWriter::new(&mut wire)
            .write_message(6, 0x00FF_FFFE, 9, 1, b"x")
            .await
            .unwrap();
        assert_eq!(wire, [0x06, 0xFF, 0xFF, 0xFE, 0, 0, 1, 9, 1, 0, 0, 0, b'x']);
    }

    #[tokio::test]
    async fn extended_timestamp_fragmented_roundtrips() {
        // With an extended timestamp AND fragmentation, every continuation
        // chunk must repeat the 4-byte ext-ts; the reader must consume it on
        // each fmt-3 or the payload bytes desync.
        let payload: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        let msg = roundtrip(6, 0x00FF_FFFF + 5, 9, 1, &payload).await;
        assert_eq!(msg.timestamp, 0x00FF_FFFF + 5);
        assert_eq!(&msg.payload[..], &payload[..]);
    }

    #[tokio::test]
    async fn set_chunk_size_control_is_consumed_and_applied() {
        // Writer announces a 4096-byte chunk size, then sends a 200-byte
        // message (now a single chunk). The reader must apply the new size
        // in-band and surface ONLY the media message, never the control one.
        let mut wire: Vec<u8> = Vec::new();
        {
            let mut w = ChunkWriter::new(&mut wire);
            w.send_set_chunk_size(4096).await.unwrap();
            let payload = vec![0xABu8; 200];
            w.write_message(6, 5, 9, 1, &payload).await.unwrap();
        }
        let mut r = reader(wire);
        let msg = r.read_message().await.unwrap();
        assert_eq!(msg.type_id, 9);
        assert_eq!(msg.payload.len(), 200);
    }

    #[tokio::test]
    async fn fmt1_reuses_stream_id_and_advances_timestamp() {
        let mut wire = fmt0(4, 1000, 9, 7, b"aaaa");
        wire.extend(fmt1(4, 33, 8, b"bb")); // new delta/length/type, reuse msid
        let mut r = reader(wire);
        let m1 = r.read_message().await.unwrap();
        let m2 = r.read_message().await.unwrap();
        assert_eq!((m1.timestamp, m1.type_id, m1.stream_id), (1000, 9, 7));
        // fmt-1 carries a delta and inherits the fmt-0 message-stream-id.
        assert_eq!((m2.timestamp, m2.type_id, m2.stream_id), (1033, 8, 7));
        assert_eq!(&m2.payload[..], b"bb");
    }

    #[tokio::test]
    async fn fmt2_reuses_length_type_and_stream() {
        // fmt-2 carries only a timestamp delta; length, type, and stream-id
        // all come from the prior chunk, so the payload MUST match the
        // reused length (4 bytes here).
        let mut wire = fmt0(4, 1000, 9, 7, b"aaaa");
        wire.extend(fmt2(4, 33, b"bbbb"));
        let mut r = reader(wire);
        let _ = r.read_message().await.unwrap();
        let m2 = r.read_message().await.unwrap();
        assert_eq!((m2.timestamp, m2.type_id, m2.stream_id), (1033, 9, 7));
        assert_eq!(&m2.payload[..], b"bbbb");
    }

    #[tokio::test]
    async fn fmt3_replays_previous_message_header() {
        // A fmt-3 basic header with no message in flight replays the last
        // header: same length/type/stream, timestamp advanced by the last
        // delta. Because fmt-0 seeds the delta to its absolute timestamp,
        // the replayed message lands at 2000 (1000 + 1000).
        let mut wire = fmt0(4, 1000, 9, 7, b"aaaa");
        wire.extend(fmt3(4, b"cccc"));
        let mut r = reader(wire);
        let _ = r.read_message().await.unwrap();
        let m2 = r.read_message().await.unwrap();
        assert_eq!((m2.timestamp, m2.type_id, m2.stream_id), (2000, 9, 7));
        assert_eq!(&m2.payload[..], b"cccc");
    }

    #[tokio::test]
    async fn truncated_header_errors_not_panics() {
        // fmt-0 basic header promises an 11-byte message header but only 5
        // are present. The reader must return an error, never panic.
        let wire = vec![0x06u8, 0, 0, 1, 0, 0]; // 1 basic + 5 of 11 header bytes
        let r = reader(wire).read_message().await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn truncated_payload_errors_not_panics() {
        // Header claims a 300-byte payload but the stream ends after 100.
        let mut wire = {
            let mut b = vec![0x06u8];
            b.extend_from_slice(&u24(0)); // ts
            b.extend_from_slice(&u24(300)); // length
            b.push(9); // type
            b.extend_from_slice(&1u32.to_le_bytes()); // msid
            b
        };
        wire.extend(std::iter::repeat_n(0u8, 100));
        let r = reader(wire).read_message().await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn zero_length_message_yields_empty_payload() {
        // A length-0 message (e.g. some script-data pings) must complete
        // immediately with an empty payload and not stall or divide-by-zero.
        let wire = fmt0(6, 5, 18, 0, b"");
        let msg = reader(wire).read_message().await.unwrap();
        assert_eq!(msg.type_id, 18);
        assert!(msg.payload.is_empty());
    }

    #[tokio::test]
    async fn take_pending_ack_none_before_threshold() {
        // Fresh reader: 0 bytes in, default 2.5 MB window -> nothing to ack.
        let mut r = reader(vec![]);
        assert!(r.take_pending_ack().is_none());
    }

    #[tokio::test]
    async fn window_ack_size_message_lowers_ack_threshold() {
        // A type-5 Window Ack Size message shrinks the window to 200, so the
        // ack threshold drops to 20 bytes. After reading a media message we
        // are well past it, so one ack is due - and only one, because
        // take_pending_ack advances its watermark.
        let mut wire = fmt0(2, 0, 5, 0, &200u32.to_be_bytes()); // Window Ack Size
        wire.extend(fmt0(6, 100, 9, 1, &[0u8; 10])); // media
        let mut r = reader(wire);
        let msg = r.read_message().await.unwrap();
        assert_eq!(msg.type_id, 9); // control consumed in-band, media surfaced
                                    // The ack reports every wire byte: (1 + 11 + 4) + (1 + 11 + 10).
        assert_eq!(r.take_pending_ack(), Some(38));
        assert!(r.take_pending_ack().is_none());
    }

    #[tokio::test]
    async fn ack_count_includes_extended_timestamps_and_continuation_headers() {
        // The peer counts what it put on the wire, so a reader that skips
        // the 4-byte extended timestamps or the fmt-3 basic headers
        // under-reports, and a strict peer stalls waiting for the window.
        let mut wire = fmt0(2, 0, 5, 0, &200u32.to_be_bytes()); // 16 bytes
        ChunkWriter::new(&mut wire)
            .write_message(6, 0x0100_0000, 9, 1, &[0u8; 200])
            .await
            .unwrap();
        let mut r = reader(wire);
        r.read_message().await.unwrap();
        // 16 + (1 + 11 + 4 + 128) + (1 + 4 + 72)
        assert_eq!(r.take_pending_ack(), Some(237));
    }

    #[tokio::test]
    async fn reader_decodes_two_and_three_byte_basic_headers() {
        // csid 64..=319 uses [fmt|0][csid - 64]; 320..=65599 uses
        // [fmt|1][lo][hi] of csid - 64. Each must land in its own csid state
        // or two streams would share one message buffer.
        for csid in [64u32, 200, 319, 320, 1000, 65_599] {
            let mut wire: Vec<u8> = Vec::new();
            ChunkWriter::new(&mut wire)
                .write_message(csid, 7, 9, 1, &[0xAB; 300])
                .await
                .unwrap();
            let mut r = reader(wire);
            let msg = r.read_message().await.unwrap();
            assert_eq!(&msg.payload[..], &[0xAB; 300][..], "csid {csid}");
            assert!(r.streams.contains_key(&csid), "csid {csid} decoded");
            assert_eq!(r.streams.len(), 1, "csid {csid}: one state only");
        }
    }

    #[tokio::test]
    async fn interleaved_messages_on_different_csids_stay_intact() {
        // Publishers interleave audio between the chunks of a large video
        // frame. The audio completes first; the video resumes on its own
        // csid with a fmt-3 and still reassembles byte for byte.
        let video: Vec<u8> = (0..200u32).map(|i| (i * 3) as u8).collect();
        let mut wire = fmt0_first_chunk(6, 40, 9, 200, &video[..128]);
        wire.extend(fmt0(4, 41, 8, 1, b"audio"));
        wire.extend(fmt3(6, &video[128..]));
        let mut r = reader(wire);
        let audio = r.read_message().await.unwrap();
        assert_eq!((audio.type_id, audio.timestamp), (8, 41));
        assert_eq!(&audio.payload[..], b"audio");
        let frame = r.read_message().await.unwrap();
        assert_eq!((frame.type_id, frame.timestamp), (9, 40));
        assert_eq!(&frame.payload[..], &video[..]);
    }

    #[tokio::test]
    async fn set_chunk_size_mid_message_applies_to_the_next_chunk() {
        // A Set Chunk Size that arrives between two chunks of another csid's
        // message governs the rest of that message: here the remaining 172
        // bytes come as one chunk instead of 128 + 44.
        let video: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let mut wire = fmt0_first_chunk(6, 0, 9, 300, &video[..128]);
        wire.extend(fmt0(2, 0, 1, 0, &4096u32.to_be_bytes()));
        wire.extend(fmt3(6, &video[128..]));
        wire.extend(fmt0(4, 0, 8, 1, b"after"));
        let mut r = reader(wire);
        assert_eq!(&r.read_message().await.unwrap().payload[..], &video[..]);
        assert_eq!(&r.read_message().await.unwrap().payload[..], b"after");
    }

    #[tokio::test]
    async fn extended_deltas_on_fmt1_and_fmt2_carry_into_their_continuations() {
        // A delta of 0xFFFFFF or more also moves to the extended field, and
        // then every fmt-3 continuation of that message repeats it. The
        // writer never produces this (it only emits fmt 0), so hand-build it.
        let body: Vec<u8> = (0..200u32).map(|i| (i * 5) as u8).collect();
        let big_delta = 0x0123_4567u32;
        let mut wire = fmt0(6, 1000, 9, 1, b"seed");

        let mut fmt1_hdr = vec![0x46]; // fmt 1, csid 6
        fmt1_hdr.extend_from_slice(&u24(0xFF_FFFF));
        fmt1_hdr.extend_from_slice(&u24(200));
        fmt1_hdr.push(9);
        fmt1_hdr.extend_from_slice(&big_delta.to_be_bytes());
        wire.extend(fmt1_hdr);
        wire.extend_from_slice(&body[..128]);
        wire.extend_from_slice(&[0xC6]);
        wire.extend_from_slice(&big_delta.to_be_bytes());
        wire.extend_from_slice(&body[128..]);

        let mut fmt2_hdr = vec![0x86]; // fmt 2, csid 6: reuses length 200
        fmt2_hdr.extend_from_slice(&u24(0xFF_FFFF));
        fmt2_hdr.extend_from_slice(&big_delta.to_be_bytes());
        wire.extend(fmt2_hdr);
        wire.extend_from_slice(&body[..128]);
        wire.extend_from_slice(&[0xC6]);
        wire.extend_from_slice(&big_delta.to_be_bytes());
        wire.extend_from_slice(&body[128..]);

        let mut r = reader(wire);
        r.read_message().await.unwrap();
        let first = r.read_message().await.unwrap();
        assert_eq!(first.timestamp, 1000 + big_delta);
        assert_eq!(&first.payload[..], &body[..]);
        let second = r.read_message().await.unwrap();
        assert_eq!(second.timestamp, 1000 + 2 * big_delta);
        assert_eq!(&second.payload[..], &body[..]);
    }

    #[tokio::test]
    async fn timestamp_deltas_wrap_at_32_bits() {
        // RTMP time is u32 milliseconds and wraps every ~49.7 days. The
        // controller widens it; the reader just has to wrap, not panic.
        let mut wire = vec![0x06];
        wire.extend_from_slice(&u24(0xFF_FFFF));
        wire.extend_from_slice(&u24(1));
        wire.push(9);
        wire.extend_from_slice(&1u32.to_le_bytes());
        wire.extend_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        wire.push(b'a');
        wire.extend(fmt1(6, 0x20, 9, b"b"));
        let mut r = reader(wire);
        assert_eq!(r.read_message().await.unwrap().timestamp, 0xFFFF_FFF0);
        assert_eq!(r.read_message().await.unwrap().timestamp, 0x10);
    }

    #[tokio::test]
    async fn out_of_range_set_chunk_size_is_clamped_not_trusted() {
        // 0 would make no progress, so it clamps to 1: every later byte of
        // payload then rides in its own chunk and still reassembles.
        let mut wire = fmt0(2, 0, 1, 0, &0u32.to_be_bytes());
        wire.extend(fmt0_first_chunk(6, 0, 9, 3, b"x"));
        wire.extend(fmt3(6, b"y"));
        wire.extend(fmt3(6, b"z"));
        let mut r = reader(wire);
        assert_eq!(&r.read_message().await.unwrap().payload[..], b"xyz");
        assert_eq!(r.chunk_size, 1);

        // The spec reserves the top bit. The value is clamped to the 24-bit
        // maximum rather than read as a ~2 GB chunk.
        let mut r = reader(fmt0(2, 0, 1, 0, &0x8000_0400u32.to_be_bytes()));
        assert!(r.read_message().await.is_err(), "only the control message");
        assert_eq!(r.chunk_size, 0xFF_FFFF);
    }

    #[tokio::test]
    async fn short_control_payloads_are_ignored_without_panicking() {
        // Set Chunk Size, Abort, Ack and Window Ack Size all carry a u32.
        // A peer that sends fewer bytes must not make us index past the
        // payload; the message is dropped and the stream carries on.
        let mut wire = Vec::new();
        for type_id in [1u8, 2, 3, 5, 6] {
            wire.extend(fmt0(2, 0, type_id, 0, &[0x01, 0x02, 0x03]));
            wire.extend(fmt0(2, 0, type_id, 0, &[]));
        }
        wire.extend(fmt0(4, 9, 8, 1, b"media"));
        let mut r = reader(wire);
        assert_eq!(&r.read_message().await.unwrap().payload[..], b"media");
        assert_eq!(r.chunk_size, DEFAULT_CHUNK_SIZE);
        assert_eq!(r.window_ack_size, 2_500_000);
    }

    /// Deterministic LCG for the fuzz test: fixed seed, no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, n: u32) -> u32 {
            self.next_u32() % n
        }
    }

    /// Feed `wire` to a fresh reader until it errors. Returns the messages
    /// it surfaced. A capture always ends, so an Err must always come: a
    /// reader that loops without consuming bytes would hang the test.
    async fn drain(wire: Vec<u8>) -> Vec<Message> {
        let mut r = reader(wire);
        let mut out = Vec::new();
        while let Ok(msg) = r.read_message().await {
            out.push(msg);
        }
        out
    }

    /// A publisher-shaped capture: control messages, fmt 0-3, interleaving,
    /// an extended timestamp with continuations, and an Abort.
    async fn realistic_capture() -> Vec<u8> {
        let mut wire: Vec<u8> = Vec::new();
        {
            let mut w = ChunkWriter::new(&mut wire);
            w.write_message(2, 0, 5, 0, &2_500_000u32.to_be_bytes())
                .await
                .unwrap();
            w.write_message(3, 0, 20, 0, b"\x02\x00\x07connect")
                .await
                .unwrap();
            w.write_message(6, 0x0100_0000, 9, 1, &[0x17; 300])
                .await
                .unwrap();
        }
        wire.extend(fmt1(6, 33, 9, &[0x27; 40]));
        wire.extend(fmt2(6, 33, &[0x27; 40]));
        wire.extend(fmt3(6, &[0x27; 40]));
        wire.extend(fmt0(4, 5, 8, 1, &[0xAF; 9]));
        wire.extend(fmt0(2, 0, 2, 0, &6u32.to_be_bytes()));
        {
            let mut w = ChunkWriter::new(&mut wire);
            w.send_set_chunk_size(64).await.unwrap();
            w.write_message(4, 30, 8, 1, &[0xAF; 150]).await.unwrap();
        }
        wire
    }

    #[tokio::test]
    async fn every_prefix_and_random_stream_terminates_without_panicking() {
        // The reader runs on bytes from anyone who completes a handshake.
        // Whatever arrives, it must return an error or a message, never
        // panic and never spin: every prefix of a real capture (each cut
        // lands mid-header or mid-payload somewhere), then random streams.
        let capture = realistic_capture().await;
        assert_eq!(drain(capture.clone()).await.len(), 7, "the full capture");
        for end in 0..capture.len() {
            drain(capture[..end].to_vec()).await;
        }

        let mut rng = Lcg(0xC4_2026);
        for _ in 0..3_000 {
            let mut wire = Vec::new();
            for _ in 0..rng.below(6) {
                // Mostly small csids and lengths so chunks actually complete
                // and state carries across chunks, with the odd huge value.
                wire.push(rng.next_u32() as u8 & 0xC7);
                let n = rng.below(16) as usize;
                wire.extend((0..n).map(|_| rng.next_u32() as u8 & 0x0F));
                if rng.below(4) == 0 {
                    wire.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
                }
            }
            drain(wire).await;
        }
    }

    #[tokio::test]
    async fn declared_lengths_do_not_reserve_memory_before_the_bytes_arrive() {
        // The 24-bit length is the peer's claim, readable before publish is
        // gated. Reserving it up front let every fmt-0 header on a fresh
        // csid pin ~16 MB, and there are 65599 csids: memory exhaustion for
        // the price of 140 wire bytes per header. What is held has to track
        // what was actually received.
        let mut wire = Vec::new();
        for csid in 3u8..11 {
            wire.push(csid);
            wire.extend_from_slice(&u24(0));
            wire.extend_from_slice(&u24(0xFF_FFFF));
            wire.push(9);
            wire.extend_from_slice(&1u32.to_le_bytes());
            wire.extend(std::iter::repeat_n(0u8, DEFAULT_CHUNK_SIZE));
        }
        let mut r = reader(wire);
        assert!(
            r.read_message().await.is_err(),
            "the capture ends mid-message"
        );
        assert_eq!(r.streams.len(), 8, "every csid started a message");
        for (csid, st) in &r.streams {
            assert!(
                st.buf.capacity() <= 4 * DEFAULT_CHUNK_SIZE,
                "csid {csid} reserved {} bytes after receiving {}",
                st.buf.capacity(),
                st.buf.len()
            );
        }
    }

    #[tokio::test]
    async fn abort_message_discards_the_partial_message_on_its_csid() {
        // Abort (type 2) tells us to drop whatever is half-received on the
        // named csid. Ignoring it meant the next fmt-3 on that csid was
        // appended to the stale half, surfacing a message stitched from two
        // frames and desyncing the rest of the stream.
        let stale = [0x11u8; 200];
        let fresh: Vec<u8> = (0..200u32).map(|i| i as u8).collect();
        let mut wire = fmt0_first_chunk(6, 1000, 9, 200, &stale[..128]);
        wire.extend(fmt0(2, 0, 2, 0, &6u32.to_be_bytes())); // Abort csid 6
        wire.extend(fmt3(6, &fresh[..128])); // a new message, header replayed
        wire.extend(fmt3(6, &fresh[128..]));
        wire.extend(fmt0(4, 0, 8, 1, b"next")); // the stream stays in sync
        let mut r = reader(wire);
        let msg = r.read_message().await.unwrap();
        assert_eq!(&msg.payload[..], &fresh[..]);
        // fmt-3 opening a message advances by the last delta (fmt-0 seeds
        // it with the absolute timestamp), same as any other replay.
        assert_eq!((msg.timestamp, msg.type_id, msg.stream_id), (2000, 9, 1));
        assert_eq!(&r.read_message().await.unwrap().payload[..], b"next");
    }

    #[test]
    fn basic_header_encodes_all_csid_ranges() {
        // 1-byte form (csid < 64), 2-byte form (< 320), 3-byte form (>= 320).
        let mut out = Vec::new();
        write_basic_header(&mut out, 0, 3);
        assert_eq!(out, [0x03]);

        out.clear();
        write_basic_header(&mut out, 0, 200);
        assert_eq!(out, [0x00, 200 - 64]);

        out.clear();
        write_basic_header(&mut out, 0, 1000);
        let v = 1000u32 - 64;
        assert_eq!(out, [0x01, (v & 0xFF) as u8, (v >> 8) as u8]);
    }

    #[test]
    fn u24_roundtrip() {
        let mut out = Vec::new();
        push_u24_be(&mut out, 0x12_3456);
        assert_eq!(out, [0x12, 0x34, 0x56]);
        assert_eq!(u24_be(&out), 0x12_3456);
    }

    /// A peer that parks a nearly finished message on stream after stream
    /// is cut off once the unfinished bytes pass `MAX_PENDING_BYTES`,
    /// instead of holding that memory for as long as it likes.
    #[tokio::test]
    async fn unfinished_messages_are_capped_per_connection() {
        const CHUNK: usize = 8 * 1024 * 1024;
        let mut wire = fmt0(2, 0, 1, 0, &(CHUNK as u32).to_be_bytes());
        let first = vec![0u8; CHUNK];
        let streams = MAX_PENDING_BYTES / CHUNK + 1;
        for csid in 0..streams {
            // Declares 12 MB, sends only the first 8 MB chunk.
            wire.extend(fmt0_first_chunk(
                3 + csid as u8,
                0,
                9,
                12 * 1024 * 1024,
                &first,
            ));
        }
        let mut reader = ChunkReader::new(std::io::Cursor::new(wire));
        let err = reader.read_message().await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData, "{err}");
    }
}
