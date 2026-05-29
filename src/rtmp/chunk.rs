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

pub struct ChunkReader<R> {
    inner: R,
    chunk_size: usize,
    streams: HashMap<u32, CsState>,
}

impl<R: AsyncReadExt + Unpin> ChunkReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            chunk_size: DEFAULT_CHUNK_SIZE,
            streams: HashMap::with_capacity(8),
        }
    }

    pub fn set_chunk_size(&mut self, size: usize) {
        self.chunk_size = size.clamp(1, 0xFF_FFFF);
    }

    /// Read until one full RTMP message is reassembled, then yield it.
    /// Control messages (Set Chunk Size = 1) are handled in-band so the
    /// caller never has to think about them.
    pub async fn read_message(&mut self) -> io::Result<Message> {
        loop {
            // --- Basic header (1-3 bytes): fmt(2) + csid(6/14/22) ---
            let mut b0 = [0u8; 1];
            self.inner.read_exact(&mut b0).await?;
            let fmt = (b0[0] >> 6) & 0x03;
            let csid_low = b0[0] & 0x3F;
            let csid: u32 = match csid_low {
                0 => {
                    let mut b = [0u8; 1];
                    self.inner.read_exact(&mut b).await?;
                    b[0] as u32 + 64
                }
                1 => {
                    let mut b = [0u8; 2];
                    self.inner.read_exact(&mut b).await?;
                    (b[1] as u32) * 256 + b[0] as u32 + 64
                }
                n => n as u32,
            };

            let st = self.streams.entry(csid).or_default();

            // --- Message header (0/3/7/11 bytes by fmt) ---
            // We only fully parse a fresh header on fmt < 3 and on the
            // first chunk of a message. fmt == 3 continuations either
            // continue an in-flight message or start a new one identical
            // to the previous (whole-message replay), per spec.
            match fmt {
                0 => {
                    let mut h = [0u8; 11];
                    self.inner.read_exact(&mut h).await?;
                    let ts24 = u24_be(&h[0..3]);
                    let len = u24_be(&h[3..6]);
                    let type_id = h[6];
                    // Message stream ID is little-endian per spec.
                    let msid = u32::from_le_bytes([h[7], h[8], h[9], h[10]]);
                    let ext_ts_present = ts24 == 0x00FF_FFFF;
                    let timestamp = if ext_ts_present {
                        read_u32_be(&mut self.inner).await?
                    } else {
                        ts24
                    };
                    st.timestamp = timestamp;
                    st.timestamp_delta = timestamp;
                    st.length = len;
                    st.type_id = type_id;
                    st.stream_id = msid;
                    st.last_had_ext_ts = ext_ts_present;
                    st.buf = BytesMut::with_capacity(len as usize);
                    st.receiving = true;
                }
                1 => {
                    let mut h = [0u8; 7];
                    self.inner.read_exact(&mut h).await?;
                    let delta24 = u24_be(&h[0..3]);
                    let len = u24_be(&h[3..6]);
                    let type_id = h[6];
                    let ext_ts_present = delta24 == 0x00FF_FFFF;
                    let delta = if ext_ts_present {
                        read_u32_be(&mut self.inner).await?
                    } else {
                        delta24
                    };
                    st.timestamp = st.timestamp.wrapping_add(delta);
                    st.timestamp_delta = delta;
                    st.length = len;
                    st.type_id = type_id;
                    st.last_had_ext_ts = ext_ts_present;
                    st.buf = BytesMut::with_capacity(len as usize);
                    st.receiving = true;
                }
                2 => {
                    let mut h = [0u8; 3];
                    self.inner.read_exact(&mut h).await?;
                    let delta24 = u24_be(&h[0..3]);
                    let ext_ts_present = delta24 == 0x00FF_FFFF;
                    let delta = if ext_ts_present {
                        read_u32_be(&mut self.inner).await?
                    } else {
                        delta24
                    };
                    st.timestamp = st.timestamp.wrapping_add(delta);
                    st.timestamp_delta = delta;
                    st.last_had_ext_ts = ext_ts_present;
                    st.buf = BytesMut::with_capacity(st.length as usize);
                    st.receiving = true;
                }
                3 => {
                    // Continuation. If we have a message in progress, keep
                    // appending; otherwise this is the spec-allowed "repeat
                    // previous message header" shortcut.
                    if !st.receiving {
                        st.timestamp = st.timestamp.wrapping_add(st.timestamp_delta);
                        st.buf = BytesMut::with_capacity(st.length as usize);
                        st.receiving = true;
                    }
                    // Continuation chunks re-emit the extended timestamp
                    // iff the message header indicated one. Many encoders
                    // get this wrong; we follow spec.
                    if st.last_had_ext_ts {
                        let _ = read_u32_be(&mut self.inner).await?;
                    }
                }
                _ => unreachable!(),
            }

            // --- Payload chunk ---
            let remaining = st.length as usize - st.buf.len();
            let to_read = remaining.min(self.chunk_size);
            let start = st.buf.len();
            st.buf.resize(start + to_read, 0);
            self.inner.read_exact(&mut st.buf[start..]).await?;

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
                    2 => continue,     // Abort Message
                    3 => continue,     // Acknowledgement
                    5 | 6 => continue, // Window Ack Size / Set Peer Bandwidth
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

async fn read_u32_be<R: AsyncReadExt + Unpin>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).await?;
    Ok(u32::from_be_bytes(b))
}
