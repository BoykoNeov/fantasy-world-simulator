// egui + wgpu bindings and entry points; the shared map code is appended after this file.

@group(0) @binding(0) var<uniform> params: MapParams;
@group(0) @binding(1) var elev_tex: texture_2d_array<f32>;
@group(0) @binding(2) var prov_tex: texture_2d_array<u32>;
@group(0) @binding(3) var pal_tex: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_map(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return map_color(pos.xy);
}

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) side_px: f32,
    @location(2) half_width: f32,
};

@vertex
fn vs_line(
    @location(0) p0: vec2<f32>,
    @location(1) p1: vec2<f32>,
    @location(2) corner: vec2<f32>,
    @location(3) kind: f32,
    @builtin(instance_index) inst: u32,
) -> LineOut {
    let v = line_vertex(p0, p1, corner, kind, f32(inst) - 1.0);
    var out: LineOut;
    out.clip = v.clip;
    out.color = v.color;
    out.side_px = v.side_px;
    out.half_width = v.half_width;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return line_fragment(in.color, in.side_px, in.half_width);
}
