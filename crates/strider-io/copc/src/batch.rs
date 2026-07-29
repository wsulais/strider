// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One node's compressed chunk to one Arrow record batch.
//!
//! Two obligations shape this:
//!
//! * [[RFC-0002:C-EXEC]] 3 — the representation at an operator boundary is a plain
//!   Arrow record batch with no Strider envelope, so what comes out of here is directly
//!   consumable through the Arrow C data interface, Flight and Parquet.
//! * [[RFC-0005:C-CRS]] 1 and 4 — coordinates in GeoArrow's encoding, with the
//!   coordinate reference system as metadata **on the coordinate field**. Batch or
//!   schema metadata explicitly does not satisfy it, so `position` is one field
//!   carrying its own system.
//!
//! The coordinate layout is GeoArrow's **separated** one: a struct of `x`, `y`, `z`
//! child arrays rather than one interleaved buffer. [[RFC-0005:C-CRS]] 4's second
//! bullet is what requires it — coordinates must remain individually projectable, so
//! that an operator reading one axis does not thereby materialise the others.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, Float64Array, Float64Builder, Int16Array, StructArray, UInt16Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Fields, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use geoarrow_schema::{CoordType, Dimension, PointType};
use laz::record::{LayeredPointRecordDecompressor, RecordDecompressor};
use laz::LazVlr;

use crate::copc::Node;
use crate::error::{Error, Result};
use crate::source::{Decoder, Source};

/// Name of the coordinate field. One field, three children.
pub const POSITION: &str = "position";

/// Byte offsets within a LAS 1.4 extended point record (formats 6, 7 and 8, which are
/// the only ones COPC admits). The first 30 bytes are common to all three.
mod at {
    pub const X: usize = 0;
    pub const Y: usize = 4;
    pub const Z: usize = 8;
    pub const INTENSITY: usize = 12;
    pub const RETURNS: usize = 14;
    pub const CLASSIFICATION: usize = 16;
    pub const SCAN_ANGLE: usize = 18;
    pub const POINT_SOURCE_ID: usize = 20;
    pub const GPS_TIME: usize = 22;
    /// Present in formats 7 and 8.
    pub const RGB: usize = 30;
}

impl Decoder {
    /// The schema every batch from this source carries.
    ///
    /// Derived from the source rather than fixed, because the attributes present depend
    /// on the point record format — a format-6 source has no colour, and inventing a
    /// null colour column would assert an attribute the source does not have.
    pub fn schema(&self) -> SchemaRef {
        let point = PointType::new(Dimension::XYZ, self.geo_metadata())
            .with_coord_type(CoordType::Separated);
        let mut fields = vec![
            point.to_field(POSITION, false),
            Field::new("intensity", DataType::UInt16, false),
            Field::new("return_number", DataType::UInt8, false),
            Field::new("number_of_returns", DataType::UInt8, false),
            Field::new("classification", DataType::UInt8, false),
            // Kept as stored. LAS records scan angle in units of 0.006 degrees, and
            // converting it here would be this crate interpreting an attribute rather
            // than carrying it.
            Field::new("scan_angle", DataType::Int16, false),
            Field::new("point_source_id", DataType::UInt16, false),
            Field::new("gps_time", DataType::Float64, false),
        ];
        if matches!(self.header().point_format, 7 | 8) {
            fields.push(Field::new("red", DataType::UInt16, false));
            fields.push(Field::new("green", DataType::UInt16, false));
            fields.push(Field::new("blue", DataType::UInt16, false));
        }
        Arc::new(Schema::new(fields))
    }

    /// Decode one node's chunk.
    ///
    /// `bytes` is the node's compressed chunk, exactly the range [`Node::chunk`] names.
    /// Nothing is sought and nothing is retrieved: the arithmetic decoder runs over the
    /// buffer it was handed, which is what makes this callable from a host that has no
    /// filesystem ([[RFC-0004:C-HOST]] 1).
    pub fn decode(&self, node: &Node, bytes: &[u8]) -> Result<RecordBatch> {
        let vlr = LazVlr::from_buffer(self.laz_vlr.as_slice())?;
        let record_len = self.header().point_record_len as usize;
        let n = node.point_count as usize;

        // The layered decompressor is the LAZ 1.4 one, and the only one correct for the
        // extended point formats. The crate's `LasZipDecompressor` is deliberately
        // avoided: it reads a chunk table by seeking, and a COPC hierarchy entry already
        // says where the chunk is.
        let mut decompressor = LayeredPointRecordDecompressor::new(std::io::Cursor::new(bytes));
        decompressor.set_fields_from(vlr.items())?;

        let transform = self.header().transform;
        let has_rgb = matches!(self.header().point_format, 7 | 8);

        let mut x = Float64Builder::with_capacity(n);
        let mut y = Float64Builder::with_capacity(n);
        let mut z = Float64Builder::with_capacity(n);
        let mut intensity = Vec::with_capacity(n);
        let mut return_number = Vec::with_capacity(n);
        let mut number_of_returns = Vec::with_capacity(n);
        let mut classification = Vec::with_capacity(n);
        let mut scan_angle = Vec::with_capacity(n);
        let mut point_source_id = Vec::with_capacity(n);
        let mut gps_time = Vec::with_capacity(n);
        let mut red = Vec::with_capacity(if has_rgb { n } else { 0 });
        let mut green = Vec::with_capacity(if has_rgb { n } else { 0 });
        let mut blue = Vec::with_capacity(if has_rgb { n } else { 0 });

        let mut raw = vec![0u8; record_len];
        let mut decoded = 0u32;
        for _ in 0..n {
            if decompressor.decompress_next(&mut raw).is_err() {
                break;
            }
            decoded += 1;
            let p = transform.apply([
                crate::las::i32_at(&raw, at::X),
                crate::las::i32_at(&raw, at::Y),
                crate::las::i32_at(&raw, at::Z),
            ]);
            x.append_value(p[0]);
            y.append_value(p[1]);
            z.append_value(p[2]);
            intensity.push(crate::las::u16_at(&raw, at::INTENSITY));
            let returns = raw[at::RETURNS];
            return_number.push(returns & 0x0f);
            number_of_returns.push(returns >> 4);
            classification.push(raw[at::CLASSIFICATION]);
            scan_angle.push(crate::las::u16_at(&raw, at::SCAN_ANGLE) as i16);
            point_source_id.push(crate::las::u16_at(&raw, at::POINT_SOURCE_ID));
            gps_time.push(crate::las::f64_at(&raw, at::GPS_TIME));
            if has_rgb {
                red.push(crate::las::u16_at(&raw, at::RGB));
                green.push(crate::las::u16_at(&raw, at::RGB + 2));
                blue.push(crate::las::u16_at(&raw, at::RGB + 4));
            }
        }
        if decoded != node.point_count {
            return Err(Error::NodePointCount {
                key: node.key,
                declared: node.point_count,
                decoded,
            });
        }

        // The separated GeoArrow layout: one struct field, three independently
        // projectable children ([[RFC-0005:C-CRS]] 4).
        let coords: Vec<(Arc<Field>, ArrayRef)> = vec![
            (
                Arc::new(Field::new("x", DataType::Float64, false)),
                Arc::new(x.finish()) as ArrayRef,
            ),
            (
                Arc::new(Field::new("y", DataType::Float64, false)),
                Arc::new(y.finish()) as ArrayRef,
            ),
            (
                Arc::new(Field::new("z", DataType::Float64, false)),
                Arc::new(z.finish()) as ArrayRef,
            ),
        ];
        let position = StructArray::from(coords);

        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(position),
            Arc::new(UInt16Array::from(intensity)),
            Arc::new(UInt8Array::from(return_number)),
            Arc::new(UInt8Array::from(number_of_returns)),
            Arc::new(UInt8Array::from(classification)),
            Arc::new(Int16Array::from(scan_angle)),
            Arc::new(UInt16Array::from(point_source_id)),
            Arc::new(Float64Array::from(gps_time)),
        ];
        if has_rgb {
            columns.push(Arc::new(UInt16Array::from(red)));
            columns.push(Arc::new(UInt16Array::from(green)));
            columns.push(Arc::new(UInt16Array::from(blue)));
        }

        Ok(RecordBatch::try_new(self.schema(), columns)?)
    }
}

impl Source {
    /// Delegates to a [`Decoder`], for a caller that has the whole source in hand.
    pub fn schema(&self) -> SchemaRef {
        self.decoder().schema()
    }

    /// Delegates to a [`Decoder`]. A host with worker threads should take one `decoder()` and
    /// share that instead, so the index stays mutable — see [`Source::decoder`].
    pub fn decode(&self, node: &Node, bytes: &[u8]) -> Result<RecordBatch> {
        self.decoder().decode(node, bytes)
    }
}

/// The children of the coordinate field, for a consumer that wants to name one axis.
pub fn coordinate_fields() -> Fields {
    Fields::from(vec![
        Field::new("x", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
        Field::new("z", DataType::Float64, false),
    ])
}
