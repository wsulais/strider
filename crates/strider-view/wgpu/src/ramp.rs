// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Colour ramps, as **data**.
//!
//! A ramp is 256 sRGB texels the host uploads. The shader samples it and knows nothing
//! else — not the ramp's name, not how many stops it had, not what attribute is being
//! ramped. That is the whole design, and it buys three things that a ramp compiled into the
//! shader cannot:
//!
//! * **A ramp becomes a choice rather than a release.** Adding magma, or a diverging ramp
//!   for a signed attribute, or a project's house palette, is data.
//! * **The renderer stops knowing about LAS.** The earlier version had `viridis(intensity)`
//!   in a `switch` over attribute names, which put the source format's vocabulary inside the
//!   device layer. A channel index and a range have no format in them.
//! * **A computed attribute ramps like a read one.** Height above ground from an analytical
//!   pass and intensity from the file arrive as the same thing.
//!
//! Stops are given in sRGB because that is how every published ramp is specified and how
//! anyone reading this file will check it. The conversion to linear happens once, here, on
//! upload — rather than per fragment, and rather than not at all, which is the mistake that
//! made viridis render as orchid.

/// Texels in a ramp. 256 is what a `Rgba8` 1-D texture gives for free and is finer than a
/// display can show.
pub const RAMP_TEXELS: usize = 256;

/// A ramp, ready to upload: 256 RGBA texels, linear, premultiplied by nothing.
#[derive(Clone)]
pub struct Ramp {
    pub name: &'static str,
    texels: Vec<u8>,
}

impl Ramp {
    /// Build from sRGB stops at given positions, linearly interpolated between them.
    ///
    /// Interpolation in sRGB rather than linear space is deliberate and is what every GIS
    /// does: the published stop values for viridis and its relatives are *already* the
    /// perceptually spaced ones, so interpolating them in linear light would undo the
    /// spacing that makes the ramp uniform.
    pub fn from_srgb_stops(name: &'static str, stops: &[(f32, [f32; 3])]) -> Self {
        assert!(!stops.is_empty(), "a ramp needs at least one stop");
        let mut texels = Vec::with_capacity(RAMP_TEXELS * 4);
        for i in 0..RAMP_TEXELS {
            let t = i as f32 / (RAMP_TEXELS - 1) as f32;
            let srgb = sample_stops(stops, t);
            for c in srgb {
                texels.push((srgb_to_linear(c) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            texels.push(255);
        }
        Self { name, texels }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.texels
    }

    /// Viridis, as published stops rather than as a polynomial fit.
    ///
    /// The fit was fine and this is better for one reason: a reader can check these numbers
    /// against the reference ramp, and could not check six vectors of polynomial
    /// coefficients against anything.
    pub fn viridis() -> Self {
        Self::from_srgb_stops(
            "viridis",
            &[
                (0.0, [0.267, 0.005, 0.329]),
                (0.125, [0.283, 0.141, 0.458]),
                (0.25, [0.254, 0.265, 0.530]),
                (0.375, [0.207, 0.372, 0.553]),
                (0.5, [0.164, 0.471, 0.558]),
                (0.625, [0.128, 0.567, 0.551]),
                (0.75, [0.135, 0.659, 0.518]),
                (0.875, [0.267, 0.749, 0.441]),
                (1.0, [0.993, 0.906, 0.144]),
            ],
        )
    }

    /// Magma. Reads better than viridis for intensity, where the interesting values are at
    /// the bright end.
    pub fn magma() -> Self {
        Self::from_srgb_stops(
            "magma",
            &[
                (0.0, [0.001, 0.000, 0.014]),
                (0.25, [0.232, 0.060, 0.437]),
                (0.5, [0.550, 0.161, 0.506]),
                (0.75, [0.882, 0.392, 0.383]),
                (1.0, [0.987, 0.991, 0.750]),
            ],
        )
    }

    /// Blue–white–red, for a signed attribute where zero is meaningful — a residual, or a
    /// height difference between two surveys. A sequential ramp hides the sign.
    pub fn diverging() -> Self {
        Self::from_srgb_stops(
            "blue-white-red",
            &[
                (0.0, [0.192, 0.310, 0.639]),
                (0.5, [0.969, 0.969, 0.969]),
                (1.0, [0.706, 0.016, 0.149]),
            ],
        )
    }

    /// Greyscale, for printing and for hillshade.
    pub fn grey() -> Self {
        Self::from_srgb_stops("grey", &[(0.0, [0.05, 0.05, 0.05]), (1.0, [1.0, 1.0, 1.0])])
    }

    pub fn all() -> Vec<Ramp> {
        vec![
            Self::viridis(),
            Self::magma(),
            Self::diverging(),
            Self::grey(),
        ]
    }
}

fn sample_stops(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    if t <= stops[0].0 {
        return stops[0].1;
    }
    if let Some(last) = stops.last() {
        if t >= last.0 {
            return last.1;
        }
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.0 && t <= b.0 {
            let u = if (b.0 - a.0).abs() < f32::EPSILON {
                0.0
            } else {
                (t - a.0) / (b.0 - a.0)
            };
            return [
                a.1[0] + (b.1[0] - a.1[0]) * u,
                a.1[1] + (b.1[1] - a.1[1]) * u,
                a.1[2] + (b.1[2] - a.1[2]) * u,
            ];
        }
    }
    stops[stops.len() - 1].1
}

/// The sRGB transfer function's inverse. Applied once per texel at build time rather than
/// per fragment, which is the other reason the ramp is a texture.
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// What drives a point's colour.
///
/// Two cases, and no third: the colour the source recorded, or a ramp over one channel. The
/// attribute enum this replaces had six arms naming LAS fields, which is exactly the
/// knowledge the device layer should not hold.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shading {
    /// The source's own colour, untouched.
    SourceRgb,
    /// Ramp channel `channel` between `range`, using the ramp bound at `ramp`.
    Ramped {
        channel: u32,
        /// Supplied by the host, and **not** observed from whatever happens to be resident.
        /// A range derived from the resident set shifts as the camera moves, so identical
        /// points change colour when a neighbour loads — which makes the picture unreadable
        /// and any comparison between two frames meaningless.
        range: (f32, f32),
        ramp: usize,
    },
}

impl Shading {
    pub(crate) fn channel_or_sentinel(&self) -> u32 {
        match self {
            // No channel index can be this, so the shader needs no second flag.
            Shading::SourceRgb => u32::MAX,
            Shading::Ramped { channel, .. } => *channel,
        }
    }

    pub(crate) fn range(&self) -> (f32, f32) {
        match self {
            Shading::SourceRgb => (0.0, 1.0),
            Shading::Ramped { range, .. } => *range,
        }
    }

    pub(crate) fn ramp_index(&self) -> usize {
        match self {
            Shading::SourceRgb => 0,
            Shading::Ramped { ramp, .. } => *ramp,
        }
    }
}
