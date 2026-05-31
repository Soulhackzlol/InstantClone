//! RTMP chunk stream reader and writer.
//!
//! A "message" is the unit of logical communication (audio frame, video
//! frame, AMF command, control). Messages are sliced into "chunks" of at
//! most `chunk_size` bytes (default 128, almost always renegotiated to
//! ~4096) and interleaved on the TCP socket per-CSID (chunk stream id).
//!
//! Chunk headers come in four `fmt` flavors that compress fields by
//! referencing the previous chunk on the same CSID:
//!   fmt 0 — full 11-byte header: timestamp, length, type, msg-stream-id
//!   fmt 1 —  7 bytes: timestamp delta, length, type   (reuse msg-stream-id)
//!   fmt 2 —  3 bytes: timestamp delta                (reuse length/type/msid)
//!   fmt 3 —  0 bytes: continuation                   (reuse everything)
//!
//! Timestamps that overflow 24 bits use an extended 32-bit field appended
//! after the message header (and on all continuation chunks for that
//! message). This is the single most-broken-in-practice corner of RTMP.

use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::io::{self, ErrorKind};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const DEFAULT_CHUNK_SIZE: usize = 128;

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
    /// Total wire bytes consumed from the peer since the connection
    /// opened — chunk headers + payload + extended timestamps + control
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
    /// Caller is responsible for actually writing the ack message —
    /// the reader cannot write because it doesn't own the writer half
    /// of the socket. librtmp's exact rule, reproduced here so the
    /// behaviour is byte-for-byte compatible with what every OBS-class
    /// publisher / consumer expects from a well-behaved RTMP peer.
    pub fn take_pending_ack(&mut self) -> Option<u32> {
        let threshold = self.bytes_in_at_last_ack + (self.window_ack_size as u64) / 10;
        if self.bytes_in > threshold {
            self.bytes_in_at_last_ack = self.bytes_in;
            // RTMP BYTES_READ_REPORT is a u32 field — let it wrap.
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
                    st.buf = BytesMut::with_capacity(length as usize);
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
                    st.buf = BytesMut::with_capacity(length as usize);
                    st.receiving = true;
                }
                ChunkHeader::Fmt2 {
                    delta,
                    ext_ts_present,
                } => {
                    st.timestamp = st.timestamp.wrapping_add(delta);
                    st.timestamp_delta = delta;
                    st.last_had_ext_ts = ext_ts_present;
                    st.buf = BytesMut::with_capacity(st.length as usize);
                    st.receiving = true;
                }
                ChunkHeader::Fmt3 => {
                    if !st.receiving {
                        // Whole-message replay: same header as before,
                        // so the absolute timestamp advances by the
                        // last delta and the buffer restarts.
                        st.timestamp = st.timestamp.wrapping_add(st.timestamp_delta);
                        st.buf = BytesMut::with_capacity(st.length as usize);
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

            if st.buf.len() as u32 == st.length {
                let msg = Message {
                    timestamp: st.timestamp,
                    type_id: st.type_id,
                    stream_id: st.stream_id,
                    payload: st.buf.split().freeze(),
                };
                st.receiving = false;

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
                    2 => continue, // Abort Message
                    3 => continue, // Acknowledgement
                    5 => {
                        // Window Acknowledgement Size from the peer —
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
                    6 => continue, // Set Peer Bandwidth — informational only
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
    /// without an ack landing — so callers should fire this from the
    /// same task that drives the chunk reader, after every read.
    pub async fn send_ack(&mut self, seq: u32) -> io::Result<()> {
        self.write_message(2, 0, 3, 0, &seq.to_be_bytes()).await
    }

    /// Write a complete RTMP message, fragmenting into chunks as needed.
    /// Emits one fmt-0 chunk followed by fmt-3 continuations — the
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

fn push_u24_be(out: &mut Vec<u8>, v: u32) {
    out.push(((v >> 16) & 0xFF) as u8);
    out.push(((v >> 8) & 0xFF) as u8);
    out.push((v & 0xFF) as u8);
}

fn u24_be(b: &[u8]) -> u32 {
    ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32)
}
