// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Opening a COPC source, and answering a spatial request against its index.
//!
//! Both are resumable state machines returning the byte ranges they want, because
//! [[RFC-0004:C-HOST]] 2 and 4 leave no other shape available: explicit offsets and
//! lengths, batched, and nothing to await.

use geoarrow_schema::{Crs, Metadata};
use std::sync::Arc;

use crate::copc::{Entry, Hierarchy, Info, Node, VoxelKey};
use crate::error::{Error, Result};
use crate::las::{parse_vlrs, Header, HEADER_LEN};
use strider_core::retrieval::find;
use strider_core::{Aabb, Delivered, Need, Range, Step};

/// LAS 1.4's WKT record, where a source states its coordinate reference system.
const WKT_USER_ID: &str = "LASF_Projection";
const WKT_RECORD_ID: u16 = 2112;
const LASZIP_USER_ID: &str = "laszip encoded";
const LASZIP_RECORD_ID: u16 = 22204;

/// Opening a source. Three rounds of retrieval, each a batch.
///
/// Round 1 is a fixed range, so it can be issued knowing nothing. Round 2's range comes
/// from the header. Round 3's comes from the COPC info record, and reads the **root
/// hierarchy page only** — deeper pages are read when a request needs them, which is
/// what keeps opening a 13 GB source independent of its size.
#[derive(Debug)]
pub struct Open {
    stage: Stage,
}

#[derive(Debug)]
enum Stage {
    Header,
    Vlrs(Header),
    RootPage(Header, Info, Vec<u8>, Crs),
    Done,
}

impl Default for Open {
    fn default() -> Self {
        Self::new()
    }
}

impl Open {
    pub fn new() -> Self {
        Self {
            stage: Stage::Header,
        }
    }

    /// The ranges wanted right now, before any bytes have been delivered.
    pub fn first(&self) -> Need {
        Need::one(Range::new(0, HEADER_LEN))
    }

    /// Advance. Call with the bytes for every range the previous `Step::Need` asked
    /// for; call with an empty slice to obtain the first need.
    pub fn step(&mut self, delivered: &[Delivered<'_>]) -> Result<Step<Source>> {
        match core::mem::replace(&mut self.stage, Stage::Done) {
            Stage::Header if delivered.is_empty() => {
                self.stage = Stage::Header;
                Ok(Step::Need(self.first()))
            }
            Stage::Header => {
                let bytes = find(delivered, Range::new(0, HEADER_LEN))?;
                let header = Header::parse(bytes)?;
                let need = Need::one(header.vlr_block());
                self.stage = Stage::Vlrs(header);
                Ok(Step::Need(need))
            }
            Stage::Vlrs(header) => {
                let bytes = find(delivered, header.vlr_block())?;
                let vlrs = parse_vlrs(bytes);

                let info = vlrs
                    .iter()
                    .find(|v| v.is(crate::copc::USER_ID, crate::copc::INFO_RECORD_ID))
                    .ok_or(Error::NoCopcInfo)
                    .and_then(Info::parse)?;

                let laz = vlrs
                    .iter()
                    .find(|v| v.is(LASZIP_USER_ID, LASZIP_RECORD_ID))
                    .ok_or(Error::NoLaszipVlr)?
                    .data
                    .to_vec();

                // [[RFC-0005:C-CRS]] 3: absence is represented distinctly and is not
                // defaulted. `Crs::default()` carries no system and no `crs_type`, so a
                // source that declares none produces a field that says so.
                //
                // [[RFC-0005:C-CRS]] 2 forbids parsing or inferring, and that decides
                // the constructor. LAS declares record 2112 to be "OGC WKT" without
                // saying which WKT, and GeoArrow's `crs_type` vocabulary has no value
                // meaning "WKT of unstated version" — only `wkt2:2019`. Choosing that
                // would be inferring; so the system is carried with `crs_type` absent,
                // which GeoArrow admits. See NOTES: this is the one place C-CRS 4's
                // "form disambiguated by `crs_type`" cannot be satisfied for a LAS
                // source without breaching C-CRS 2.
                let crs = vlrs
                    .iter()
                    .find(|v| v.is(WKT_USER_ID, WKT_RECORD_ID))
                    .map(|v| {
                        let end = v.data.iter().position(|c| *c == 0).unwrap_or(v.data.len());
                        Crs::from_unknown_crs_type(
                            String::from_utf8_lossy(&v.data[..end]).into_owned(),
                        )
                    })
                    .unwrap_or_default();

                let need = Need::one(info.root_page());
                self.stage = Stage::RootPage(header, info, laz, crs);
                Ok(Step::Need(need))
            }
            Stage::RootPage(header, info, laz, crs) => {
                let bytes = find(delivered, info.root_page())?;
                let page = crate::copc::parse_page(bytes)?;
                let root_bounds = info.root_bounds();
                let mut hierarchy = Hierarchy::default();
                hierarchy.absorb(&page, &root_bounds);
                Ok(Step::Ready(Source {
                    header,
                    info,
                    laz_vlr: laz,
                    crs,
                    hierarchy,
                    root_bounds,
                }))
            }
            Stage::Done => Err(Error::SteppedAfterReady),
        }
    }
}

/// Everything needed to turn a node's bytes into a batch, and nothing else.
///
/// `Clone` is cheap: the LASzip record is shared, and the rest is a few hundred bytes. A host
/// hands one of these to each worker and keeps the [`Source`] — and therefore the index —
/// to itself.
#[derive(Clone, Debug)]
pub struct Decoder {
    pub(crate) header: Header,
    pub(crate) laz_vlr: std::sync::Arc<Vec<u8>>,
    pub(crate) crs: Crs,
}

impl Decoder {
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Opaque, as [[RFC-0005:C-CRS]] 2 requires.
    pub fn crs(&self) -> &Crs {
        &self.crs
    }

    /// Field metadata for the coordinate field: the system, and no edge interpretation.
    ///
    /// [[RFC-0005:C-CRS]] 4 names `edges` as one of the three things to carry, and the
    /// conformant value here is its absence. GeoArrow's `edges` says how to interpolate
    /// *between* two vertices, and points have no edges — every variant the vocabulary offers
    /// (spherical, Karney, Andoyer, Thomas) is a path formula. Absent is the statement, not
    /// an omission.
    pub(crate) fn geo_metadata(&self) -> Arc<Metadata> {
        Arc::new(Metadata::new(self.crs.clone(), None))
    }
}

fn alloc_shared(bytes: &[u8]) -> std::sync::Arc<Vec<u8>> {
    std::sync::Arc::new(bytes.to_vec())
}

/// An open COPC source: its header, its index, and its coordinate reference system.
///
/// Holds no points and no handle to anything. Every method either answers from the
/// index or says which bytes would answer it.
#[derive(Debug)]
pub struct Source {
    header: Header,
    info: Info,
    /// The LASzip record's bytes, kept as bytes. Parsed on decode rather than held
    /// parsed so this type stays `Send` and cheap to hold across a frame.
    laz_vlr: Vec<u8>,
    crs: Crs,
    hierarchy: Hierarchy,
    root_bounds: Aabb,
}

impl Source {
    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Opaque, as [[RFC-0005:C-CRS]] 2 requires. Comparable for identity, nothing else.
    pub fn crs(&self) -> &Crs {
        &self.crs
    }

    pub fn root_bounds(&self) -> &Aabb {
        &self.root_bounds
    }

    pub fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }

    /// The coarsest level whose point spacing is at least as fine as `target`.
    ///
    /// Answered from the info record alone, so a level-of-detail choice costs no
    /// retrieval — which is what lets it happen inside a frame.
    pub fn level_for_spacing(&self, target: f64) -> u8 {
        let mut level = 0u8;
        while level < 20 && self.info.spacing_at(level) > target {
            level += 1;
        }
        level
    }

    /// Resolve a spatial request against the index.
    ///
    /// `Step::Need` means the index itself is incomplete for this query — deeper
    /// hierarchy pages must be read first. `Step::Ready` gives the nodes whose cubes
    /// meet the query at or above the level asked for; retrieving them is the caller's,
    /// via [`Node::chunk`].
    pub fn resolve(&self, query: &Aabb, max_level: u8) -> Step<Vec<Node>> {
        let pages = self
            .hierarchy
            .pages_meeting(&self.root_bounds, query, max_level);
        if !pages.is_empty() {
            return Step::Need(Need {
                ranges: pages.iter().map(Entry::range).collect(),
            });
        }
        let mut nodes: Vec<Node> = self
            .hierarchy
            .nodes()
            .iter()
            .copied()
            .filter(|n| n.level() <= max_level && n.bounds.intersects_xy(query))
            .collect();
        // Coarse first: a consumer drawing as batches arrive gets a complete if blurry
        // picture early, rather than a sharp corner and a hole.
        nodes.sort_by_key(|n| (n.key.level, n.chunk.offset));
        Step::Ready(nodes)
    }

    /// Fold delivered hierarchy pages into the index.
    pub fn absorb_pages(&mut self, delivered: &[Delivered<'_>]) -> Result<usize> {
        let mut absorbed = 0;
        for d in delivered {
            let page = crate::copc::parse_page(d.bytes)?;
            let key = self
                .hierarchy
                .pages_meeting(&self.root_bounds, &self.root_bounds, u8::MAX)
                .into_iter()
                .find(|p| p.offset == d.range.offset)
                .map(|p| p.key);
            self.hierarchy.absorb(&page, &self.root_bounds);
            if let Some(k) = key {
                self.hierarchy.forget_page(k);
            }
            absorbed += 1;
        }
        Ok(absorbed)
    }

    /// A node the index knows about, by key.
    pub fn node(&self, key: VoxelKey) -> Option<&Node> {
        self.hierarchy.nodes().iter().find(|n| n.key == key)
    }

    /// A cheap, shareable snapshot of everything decoding needs.
    ///
    /// This split exists because of a bug that was invisible until a viewport had holes in
    /// it. Handing worker threads the whole `Source` meant the host could not take a
    /// mutable borrow of it to fold in newly read hierarchy pages, so paging starved
    /// whenever a read was in flight — which, in a frame loop, is almost always. Deeper
    /// nodes were therefore never *known to exist*, and the renderer cannot request a
    /// partition the index has never heard of.
    ///
    /// The two halves have genuinely different lifetimes: the index grows as pages arrive,
    /// while what decoding needs — the header, the LASzip record, the coordinate reference
    /// system — is fixed the moment the source is open.
    pub fn decoder(&self) -> Decoder {
        Decoder {
            header: self.header,
            laz_vlr: alloc_shared(&self.laz_vlr),
            crs: self.crs.clone(),
        }
    }

}
