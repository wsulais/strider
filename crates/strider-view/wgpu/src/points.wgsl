// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// PROTOTYPE / THROWAWAY. Points, and depth-tested anchors.

struct Camera {
    view_proj: mat4x4<f32>,
    // z range of the cloud, for the height ramp.
    z_lo: f32,
    z_hi: f32,
    point_size: f32,
    // Which channel to ramp, or a sentinel: 0xffffffff for the source colour, 0xfffffffe
    // for classification. See `Shading` on the Rust side.
    ramp_channel: u32,
    // The channel's range, SUPPLIED BY THE HOST. Not observed from the resident set: a range
    // derived from what happens to be loaded shifts as the camera moves, so the same point
    // changes colour when a neighbour arrives.
    ramp_lo: f32,
    ramp_hi: f32,
    // Target size in pixels. Supplied rather than assumed: these were hardcoded, so a
    // target of any other size got the wrong point size.
    viewport: vec2<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;

struct PointIn {
    @location(0) pos: vec3<f32>,
    // Classification *after* the effective edit set has been applied. The mask lives in
    // `render-core` and the host uploads its result, so the shader never sees an edit —
    // which is what keeps the gesture model out of the device layer.
    // Not `class`: WGSL reserves it.
    @location(1) classification: u32,
    // Colour as the source recorded it, normalised by the host.
    @location(2) rgb: vec3<f32>,
    // Rampable channels whose MEANING THIS SHADER DOES NOT KNOW. The host decides what each
    // carries — an attribute read from the file, or one an analytical pass computed — and
    // says which index to ramp. Naming them here would put LAS inside the renderer.
    @location(3) channels: vec4<f32>,
    // The fifth channel, alone because Vulkan has no five-component vertex format. Its meaning is
    // as unknown here as the other four's.
    @location(4) channel4: f32,
};

struct PointOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec3<f32>,
};

// The ramp, as a texture the host uploaded. The shader knows nothing about which ramp it
// is, nor what attribute is being ramped — only how to sample it.
@group(0) @binding(1) var ramp_tex: texture_1d<f32>;
@group(0) @binding(2) var ramp_smp: sampler;

fn ramp_t(value: f32) -> f32 {
    return clamp((value - cam.ramp_lo) / max(cam.ramp_hi - cam.ramp_lo, 1e-6), 0.0, 1.0);
}

// Class palette. The one place a LAS vocabulary survives, and it survives because an edit
// carries an attribute predicate over classification, so the renderer already interprets
// that one field. Everything else goes through a channel and a ramp.
fn class_colour(c: u32) -> vec3<f32> {
    switch c {
        case 2u:      { return vec3<f32>(0.77, 0.66, 0.47); }
        case 3u, 4u:  { return vec3<f32>(0.38, 0.63, 0.33); }
        case 5u:      { return vec3<f32>(0.27, 0.78, 0.35); }
        case 6u:      { return vec3<f32>(0.84, 0.38, 0.36); }
        case 9u:      { return vec3<f32>(0.29, 0.60, 0.84); }
        default:      { return vec3<f32>(0.62, 0.62, 0.67); }
    }
}

// sRGB to linear, for the colours still computed in display space here: the class palette,
// and a LAS RGB triple that came from a camera. The ramp texture is already linear, having
// been converted once per texel at upload.
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// What a point is coloured by. Two cases and no third.
fn shade(in: PointIn) -> vec3<f32> {
    // The sentinel the host writes when it wants the source colour. No channel index can
    // collide with it, so no second flag is needed.
    if cam.ramp_channel == 0xffffffffu {
        return to_linear(clamp(in.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    if cam.ramp_channel == 0xfffffffeu {
        // Classification, which the renderer interprets for the reason `class_colour` gives.
        return to_linear(class_colour(in.classification));
    }
    var value = in.channels[0];
    if cam.ramp_channel == 1u { value = in.channels[1]; }
    else if cam.ramp_channel == 2u { value = in.channels[2]; }
    else if cam.ramp_channel == 3u { value = in.channels[3]; }
    else if cam.ramp_channel == 4u { value = in.channel4; }
    return textureSampleLevel(ramp_tex, ramp_smp, ramp_t(value), 0.0).rgb;
}

@vertex
fn vs_points(@builtin(vertex_index) vi: u32, in: PointIn) -> PointOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vi];
    var clip = cam.view_proj * vec4<f32>(in.pos, 1.0);
    clip = vec4<f32>(
        clip.x + corner.x * cam.point_size * clip.w / cam.viewport.x,
        clip.y + corner.y * cam.point_size * clip.w / cam.viewport.y,
        clip.z,
        clip.w,
    );
    var out: PointOut;
    out.clip = clip;
    // `shade` returns linear already: the ramp texture is linear and the two display-space
    // paths convert themselves.
    out.colour = shade(in);
    return out;
}


@fragment
fn fs_points(in: PointOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.colour, 1.0);
}

// ---------------------------------------------------------------------------- anchors
//
// [[RFC-0006:C-OVERLAY]] 1 requires depth-dependent content to be drawn by the renderer.
// Here it is: a billboard at a world position, rasterised with the same depth buffer the
// cloud wrote. Occlusion is therefore a real depth comparison rather than a decision
// anybody made — a marker behind a canopy is discarded by the depth test, which is what a
// toolkit compositing the same label cannot do at any cost.

struct AnchorIn {
    @location(0) world: vec3<f32>,
    @location(1) kind: u32,
};

struct AnchorOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) kind: u32,
};

@vertex
fn vs_anchors(
    @builtin(vertex_index) vi: u32,
    in: AnchorIn,
) -> AnchorOut {
    // A screen-aligned quad, expanded in clip space so it keeps its pixel size.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vi];
    var clip = cam.view_proj * vec4<f32>(in.world, 1.0);
    let scale = 14.0;
    clip = vec4<f32>(
        clip.x + corner.x * scale * clip.w / cam.viewport.x,
        clip.y + corner.y * scale * clip.w / cam.viewport.y,
        clip.z,
        clip.w,
    );
    var out: AnchorOut;
    out.clip = clip;
    out.uv = corner;
    out.kind = in.kind;
    return out;
}

@fragment
fn fs_anchors(in: AnchorOut) -> @location(0) vec4<f32> {
    let r = length(in.uv);
    if r > 1.0 { discard; }
    let ring = smoothstep(0.55, 0.72, r) * (1.0 - smoothstep(0.92, 1.0, r));
    if ring < 0.04 { discard; }
    return vec4<f32>(to_linear(vec3<f32>(0.47, 0.90, 0.94)), ring);
}
