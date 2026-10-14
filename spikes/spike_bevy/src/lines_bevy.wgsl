// Bevy border-line material. Wrap copy (-1, 0, +1) comes from the material, one entity per copy,
// because Bevy uses the instance index for its own per-mesh data.
// The shared map code is appended after this file; its texture globals are declared but unused.

@group(2) @binding(0) var<uniform> params: MapParams;
@group(2) @binding(1) var<uniform> wrap: vec4<f32>;
@group(2) @binding(10) var elev_tex: texture_2d_array<f32>;
@group(2) @binding(11) var prov_tex: texture_2d_array<u32>;
@group(2) @binding(12) var pal_tex: texture_2d<f32>;

struct LineIn {
    @location(0) p0: vec2<f32>,
    @location(1) p1: vec2<f32>,
    @location(2) corner: vec2<f32>,
    @location(3) kind: f32,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) side_px: f32,
    @location(2) half_width: f32,
};

@vertex
fn vertex(in: LineIn) -> LineOut {
    let v = line_vertex(in.p0, in.p1, in.corner, in.kind, wrap.x);
    var out: LineOut;
    out.clip = v.clip;
    out.color = vec4<f32>(pow(v.color.rgb, vec3<f32>(2.2)), v.color.a);
    out.side_px = v.side_px;
    out.half_width = v.half_width;
    return out;
}

@fragment
fn fragment(in: LineOut) -> @location(0) vec4<f32> {
    return line_fragment(in.color, in.side_px, in.half_width);
}
