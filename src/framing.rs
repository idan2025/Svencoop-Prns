//! Tiny length-prefix framing for GoldSrc datagrams over Reticulum links.
//!
//! Reticulum's link payload ceiling (the MDU) is far smaller than a GoldSrc
//! UDP datagram can be (~1400 bytes for signon/world-state). A single
//! `SendToLink` call can't carry those, so the bridge fragments each datagram
//! into MDU-sized chunks and reassembles them on the far side.
//!
//! Wire format per chunk: one header byte + up to MAX_CHUNK payload bytes.
//!   header bit 0 (0x01): final chunk of this datagram
//! Everything else in the header is reserved/zero.
//!
//! Reassembly: accumulate chunks until the "final" bit arrives, then emit the
//! whole datagram. This is per-link state; the relay owns one reassembly
//! buffer per direction it cares about.

use personal_rns::prelude::IDENTITY_SECRET_KEY_LEN;

/// The largest payload we'll hand to `SendToLink` in one chunk. The Reticulum
/// link MDU depends on the negotiated link MTU (which itself depends on the
/// interface MTU); the default `BROADCAST_MTU` of 500 yields an MDU around
/// 415 bytes. We pick 384 to stay safely under the smallest plausible MDU
/// while leaving headroom for the one-byte framing header.
pub const MAX_CHUNK: usize = 384;

const FLAG_FINAL: u8 = 0x01;

/// Frame one GoldSrc datagram into a sequence of link-packet payloads (each
/// `<= MAX_CHUNK + 1`, ready for `SendToLink`). Returns the chunks in order.
pub fn frame(datagram: &[u8]) -> Vec<Vec<u8>> {
    if datagram.is_empty() {
        // A zero-length datagram is degenerate; emit a single final chunk with
        // no payload so the reassembler still produces an empty datagram.
        return vec![vec![FLAG_FINAL]];
    }
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < datagram.len() {
        let end = (offset + MAX_CHUNK).min(datagram.len());
        let mut chunk = Vec::with_capacity(1 + (end - offset));
        let is_final = end == datagram.len();
        chunk.push(if is_final { FLAG_FINAL } else { 0u8 });
        chunk.extend_from_slice(&datagram[offset..end]);
        out.push(chunk);
        offset = end;
    }
    out
}

/// A per-direction reassembly buffer. Feed it chunks from `frame`; `push`
/// returns `Some(complete_datagram)` when the final chunk arrives.
#[derive(Default)]
pub struct Reassembler {
    buf: Vec<u8>,
}

impl Reassembler {
    pub fn push(&mut self, chunk: &[u8]) -> Option<Vec<u8>> {
        if chunk.is_empty() {
            return None;
        }
        let header = chunk[0];
        let body = &chunk[1..];
        self.buf.extend_from_slice(body);
        if header & FLAG_FINAL != 0 {
            let complete = std::mem::take(&mut self.buf);
            // Guard: a GoldSrc datagram is at most ~64 KiB (UDP max). If
            // reassembly somehow runs past that, drop the buffer to avoid
            // unbounded growth on a malformed stream.
            if complete.len() > 65_535 {
                self.buf.clear();
                return None;
            }
            Some(complete)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
    }
}

// Silence the unused-import warning on builds that don't reference it.
#[allow(dead_code)]
const _: usize = IDENTITY_SECRET_KEY_LEN;