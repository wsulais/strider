// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Device memory, as the host owns it.
//!
//! In a real build these are wgpu buffers. Here they are `Vec<Vertex>`, which changes
//! nothing about the boundary being tested: the renderer holds a `UploadToken` and can
//! read through `Uploads`, and cannot allocate, free, or fill one.

use std::collections::BTreeMap;

use strider_view::{UploadToken, Uploads, Vertex};

#[derive(Default)]
pub struct Store {
    buffers: BTreeMap<UploadToken, Vec<Vertex>>,
    next: u64,
    pub uploaded_points: u64,
    pub freed_points: u64,
}

impl Store {
    /// Upload a decoded node. Returns the handle the renderer will hold.
    pub fn upload(&mut self, verts: Vec<Vertex>) -> UploadToken {
        self.next += 1;
        let token = UploadToken(self.next);
        self.uploaded_points += verts.len() as u64;
        self.buffers.insert(token, verts);
        token
    }

    /// Perform an `Effect::Evict`. The renderer decided; the host acts.
    pub fn free(&mut self, token: UploadToken) {
        if let Some(v) = self.buffers.remove(&token) {
            self.freed_points += v.len() as u64;
        }
    }

    pub fn resident_points(&self) -> u64 {
        self.buffers.values().map(|v| v.len() as u64).sum()
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

static EMPTY: &[Vertex] = &[];

impl Uploads for Store {
    fn vertices(&self, token: UploadToken) -> &[Vertex] {
        self.buffers.get(&token).map(|v| v.as_slice()).unwrap_or(EMPTY)
    }
}
