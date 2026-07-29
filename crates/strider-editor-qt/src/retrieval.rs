// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Real retrieval: real file reads and real LASzip decompression, on real threads.
//!
//! This module is where the prototype earns its evidence. An earlier draft simulated
//! retrieval with a latency counter, and the objection to that was correct — a
//! simulated latency cannot show whether the renderer's cancellation rule is worth
//! anything, because the thing being thrown away is exactly the cost the simulation
//! invents.
//!
//! Here a cancelled request has already cost a `pread` against an 11 GB file and an
//! arithmetic decode of a LAZ chunk, and the prototype reports how many milliseconds of
//! that were discarded. That is the number [[RFC-0006:C-RENDER]] 2 is an argument
//! about.
//!
//! The host does the one thing a compliant host is *allowed* to do and which is worst
//! for the renderer: when the renderer cancels, the host **lets the read finish**.
//! C-RENDER 2 requires cancellation to take effect "without waiting for
//! already-dispatched work on that request to run to completion", so modelling the
//! worst legal case is the only honest test.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use strider_view::{Lod, PartitionId, RequestId, Vertex};
use strider_io::copc::{Decoder, Node};

/// A finished read, on its way back to the frame loop.
pub struct Completed {
    pub req: RequestId,
    pub id: PartitionId,
    pub lod: Lod,
    pub verts: Vec<Vertex>,
    /// Wall-clock cost of the read and decode. Real milliseconds of real work.
    pub cost_us: u64,
    /// The renderer had asked for this to be cancelled before it finished.
    pub was_cancelled: bool,
    pub bytes_read: u64,
}

/// Worker-thread retrieval against one open file.
pub struct Retrieval {
    path: String,
    tx: Sender<Completed>,
    rx: Receiver<Completed>,
    /// Requests the renderer cancelled. Consulted only for *reporting*: the read is
    /// not stopped, because the point is that it need not be.
    cancelled: Arc<Mutexish>,
    pub dispatched: u64,
    pub inflight: usize,
    pub bytes_read: u64,
    pub decode_us: u64,
    pub wasted_us: u64,
    pub wasted_reads: u64,
    pub wasted_bytes: u64,
}

/// A set of cancelled request ids. `Mutex<Vec<..>>` under a name that says what it is
/// for, since a prototype reading this should not have to wonder whether the lock is
/// load-bearing. It is not: nothing waits on it.
pub struct Mutexish {
    inner: std::sync::Mutex<Vec<u64>>,
    pub marks: AtomicU64,
}

impl Mutexish {
    fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
            marks: AtomicU64::new(0),
        }
    }

    fn mark(&self, req: RequestId) {
        self.marks.fetch_add(1, Ordering::Relaxed);
        self.inner.lock().unwrap().push(req.0);
    }

    fn is_marked(&self, req: RequestId) -> bool {
        self.inner.lock().unwrap().contains(&req.0)
    }
}

impl Retrieval {
    pub fn new(path: &str) -> Self {
        let (tx, rx) = channel();
        Self {
            path: path.to_string(),
            tx,
            rx,
            cancelled: Arc::new(Mutexish::new()),
            dispatched: 0,
            inflight: 0,
            bytes_read: 0,
            decode_us: 0,
            wasted_us: 0,
            wasted_reads: 0,
            wasted_bytes: 0,
        }
    }

    /// Read one arbitrary range. Used by the open sequence and by hierarchy paging,
    /// both of which happen before there is a frame loop to be asynchronous about.
    pub fn read_blocking(path: &str, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Dispatch a node read. Returns immediately; the frame loop never waits.
    pub fn dispatch(
        &mut self,
        req: RequestId,
        id: PartitionId,
        lod: Lod,
        node: Node,
        // A `Decoder`, not the whole `Source`: holding the source here is what starved
        // hierarchy paging, because the host could then never borrow it mutably to fold in a
        // newly read page. See `Source::decoder`.
        decoder: Decoder,
        origin: [f64; 3],
    ) {
        let tx = self.tx.clone();
        let path = self.path.clone();
        let cancelled = Arc::clone(&self.cancelled);
        self.dispatched += 1;
        self.inflight += 1;
        // The host spawns threads because the host is allowed to: [[RFC-0004:C-HOST]] 3
        // withholds that from *library* crates, and supplying it is this crate's job.
        std::thread::spawn(move || {
            let started = Instant::now();
            // Always sends, including on failure. A retrieval that silently never
            // returns would leave the request in flight for ever, and the renderer has
            // no timeout — by design, since it has no clock.
            let verts = Self::read_blocking(&path, node.chunk.offset, node.chunk.len)
                .ok()
                .and_then(|bytes| decoder.decode(&node, &bytes).ok())
                .map(|batch| strider_doc::doc::to_vertices(&batch, origin))
                .unwrap_or_default();
            let _ = tx.send(Completed {
                req,
                id,
                lod,
                verts,
                cost_us: started.elapsed().as_micros() as u64,
                was_cancelled: cancelled.is_marked(req),
                bytes_read: node.chunk.len,
            });
        });
    }

    /// Note a cancellation, and do not act on it.
    pub fn note_cancel(&mut self, req: RequestId) {
        self.cancelled.mark(req);
    }

    /// Everything that finished since the last call. Never blocks — `try_recv` is the
    /// whole point.
    pub fn drain(&mut self) -> Vec<Completed> {
        let mut out = Vec::new();
        while let Ok(c) = self.rx.try_recv() {
            self.inflight = self.inflight.saturating_sub(1);
            self.bytes_read += c.bytes_read;
            self.decode_us += c.cost_us;
            if c.was_cancelled {
                self.wasted_us += c.cost_us;
                self.wasted_reads += 1;
                self.wasted_bytes += c.bytes_read;
            }
            out.push(c);
        }
        out
    }
}
