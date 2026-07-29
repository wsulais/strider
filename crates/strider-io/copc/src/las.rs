// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! LAS 1.4 public header block and variable-length records, parsed from delivered
//! bytes.
//!
//! Written here rather than taken from a crate for one reason: every published LAS
//! reader takes a path or a seekable stream, and [[RFC-0004:C-HOST]] 1 and 2 admit
//! neither. The format is a fixed-layout header followed by length-prefixed records,
//! so parsing it from a `&[u8]` is smaller than adapting a reader that wants to seek.

use crate::error::{Error, Result};
use strider_core::{Aabb, Range};

/// Size of the LAS 1.4 public header block. Fixed, which is what lets the very first
/// retrieval be issued before anything about the source is known.
pub const HEADER_LEN: u64 = 375;

/// Fixed size of a VLR header; its payload length is read from it.
pub const VLR_HEADER_LEN: usize = 54;

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

pub(crate) fn f64_at(b: &[u8], o: usize) -> f64 {
    f64::from_bits(u64_at(b, o))
}

/// The scale and offset that turn a stored integer coordinate into a real one.
///
/// Kept as declared rather than folded silently into the points: it is the source's own
/// statement about its quantisation, and a stage that needs to know it — a point
/// reference, a tolerance — cannot recover it once applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

impl Transform {
    pub fn apply(&self, raw: [i32; 3]) -> [f64; 3] {
        [
            raw[0] as f64 * self.scale[0] + self.offset[0],
            raw[1] as f64 * self.scale[1] + self.offset[1],
            raw[2] as f64 * self.scale[2] + self.offset[2],
        ]
    }
}

/// One variable-length record: header parsed, payload borrowed.
#[derive(Clone, Copy, Debug)]
pub struct Vlr<'a> {
    pub user_id: [u8; 16],
    pub record_id: u16,
    pub data: &'a [u8],
}

impl Vlr<'_> {
    /// User id compared case-insensitively against an ASCII name, ignoring the zero
    /// padding writers fill the field with.
    pub fn is(&self, user_id: &str, record_id: u16) -> bool {
        if self.record_id != record_id {
            return false;
        }
        let end = self
            .user_id
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(self.user_id.len());
        self.user_id[..end].eq_ignore_ascii_case(user_id.as_bytes())
    }
}

/// The parts of the public header block later reads depend on.
///
/// Not every field. A source adapter carrying the whole header asserts that all of it
/// is load-bearing; a reader is easier to audit when what it holds is what it uses.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub version: (u8, u8),
    pub header_len: u16,
    pub offset_to_point_data: u32,
    pub vlr_count: u32,
    pub point_format: u8,
    pub point_record_len: u16,
    pub point_count: u64,
    pub transform: Transform,
    pub bounds: Aabb,
    pub evlr_offset: u64,
    pub evlr_count: u32,
}

impl Header {
    /// Parse the public header block.
    ///
    /// Rejects rather than adapts. COPC is defined over LAS 1.4 with the extended point
    /// formats only, so a 1.2 file or a format-3 record is not a source to read at
    /// reduced fidelity — it is a different format, and saying so names what is wrong.
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < HEADER_LEN as usize {
            return Err(Error::ShortRead {
                what: "the public header block",
                declared: HEADER_LEN as usize,
                delivered: b.len(),
            });
        }
        if &b[0..4] != b"LASF" {
            return Err(Error::NotLas {
                found: [b[0], b[1], b[2], b[3]],
            });
        }
        let version = (b[24], b[25]);
        if version != (1, 4) {
            return Err(Error::UnsupportedVersion {
                major: version.0,
                minor: version.1,
            });
        }
        // The top two bits are the legacy compression flags, not part of the format id.
        let point_format = b[104] & 0x3f;
        if !matches!(point_format, 6..=8) {
            return Err(Error::UnsupportedPointFormat(point_format));
        }
        Ok(Header {
            version,
            header_len: u16_at(b, 94),
            offset_to_point_data: u32_at(b, 96),
            vlr_count: u32_at(b, 100),
            point_format,
            point_record_len: u16_at(b, 105),
            point_count: u64_at(b, 247),
            transform: Transform {
                scale: [f64_at(b, 131), f64_at(b, 139), f64_at(b, 147)],
                offset: [f64_at(b, 155), f64_at(b, 163), f64_at(b, 171)],
            },
            bounds: Aabb {
                min: [f64_at(b, 187), f64_at(b, 203), f64_at(b, 219)],
                max: [f64_at(b, 179), f64_at(b, 195), f64_at(b, 211)],
            },
            evlr_offset: u64_at(b, 235),
            evlr_count: u32_at(b, 243),
        })
    }

    /// Where the VLR block lives: immediately after the header, ending at the points.
    pub fn vlr_block(&self) -> Range {
        Range::new(
            self.header_len as u64,
            self.offset_to_point_data as u64 - self.header_len as u64,
        )
    }
}

/// Walk the VLR block.
///
/// A malformed length ends the walk rather than failing the parse: a trailing padding
/// run is common in the wild and is not a defect in the records that did parse.
pub fn parse_vlrs(block: &[u8]) -> Vec<Vlr<'_>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + VLR_HEADER_LEN <= block.len() {
        let mut user_id = [0u8; 16];
        user_id.copy_from_slice(&block[at + 2..at + 18]);
        let record_id = u16_at(block, at + 18);
        let len = u16_at(block, at + 20) as usize;
        let data_at = at + VLR_HEADER_LEN;
        if data_at + len > block.len() {
            break;
        }
        out.push(Vlr {
            user_id,
            record_id,
            data: &block[data_at..data_at + len],
        });
        at = data_at + len;
    }
    out
}
