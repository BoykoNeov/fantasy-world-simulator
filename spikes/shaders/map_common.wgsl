// Shared by both renderer spikes. Mirrors spike_world::cube and spike_world::view exactly.
// The including app must declare these bindings (any group/binding numbers):
//   var<uniform> params: MapParams;
//   var elev_tex: texture_2d_array<f32>;   // R32Float, one layer per cube face, metres
//   var prov_tex: texture_2d_array<u32>;   // R32Uint province id per cell
//   var pal_tex: texture_2d<f32>;          // RGBA8, 256 provinces per row

struct MapParams {
    view: vec4<f32>,    // centre lon, centre lat, scale (px per radian), face size n
    screen: vec4<f32>,  // map width, height, origin x, origin y (physical px)
    layers: vec4<f32>,  // terrain opacity, political opacity, lines on, province-line alpha
};

const PI: f32 = 3.14159265358979;
const FRAC_PI_2: f32 = 1.57079632679490;
const FRAC_PI_4: f32 = 0.78539816339745;
const NO_PROVINCE: u32 = 0xffffffffu;
const PLANET_RADIUS_M: f32 = 6371000.0;

fn wrap_lon(l: f32) -> f32 {
    return l - 2.0 * PI * floor((l + PI) / (2.0 * PI));
}

fn screen_to_lonlat(px: vec2<f32>) -> vec2<f32> {
    let p = px - params.screen.zw;
    let s = params.view.z;
    return vec2<f32>(
        wrap_lon(params.view.x + (p.x - params.screen.x * 0.5) / s),
        params.view.y - (p.y - params.screen.y * 0.5) / s,
    );
}

fn lonlat_to_dir(ll: vec2<f32>) -> vec3<f32> {
    let c = cos(ll.y);
    return vec3<f32>(c * sin(ll.x), sin(ll.y), c * cos(ll.x));
}

// x: face index, yz: continuous texel coordinates (0..n) on that face.
fn dir_to_face_texel(d: vec3<f32>) -> vec3<f32> {
    let a = abs(d);
    var face = 0.0;
    var ab = vec2<f32>(0.0);
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x > 0.0) { face = 0.0; ab = vec2<f32>(-d.z / d.x, d.y / d.x); }
        else { face = 1.0; ab = vec2<f32>(d.z / -d.x, d.y / -d.x); }
    } else if (a.y >= a.z) {
        if (d.y > 0.0) { face = 2.0; ab = vec2<f32>(d.x / d.y, -d.z / d.y); }
        else { face = 3.0; ab = vec2<f32>(d.x / -d.y, d.z / -d.y); }
    } else if (d.z > 0.0) {
        face = 4.0; ab = vec2<f32>(d.x / d.z, d.y / d.z);
    } else {
        face = 5.0; ab = vec2<f32>(d.x / d.z, d.y / -d.z);
    }
    let uv = atan(ab) / FRAC_PI_4;
    return vec3<f32>(face, (uv + 1.0) * 0.5 * params.view.w);
}

fn elev_load(face: i32, t: vec2<i32>) -> f32 {
    let n = i32(params.view.w);
    return textureLoad(elev_tex, clamp(t, vec2<i32>(0), vec2<i32>(n - 1)), face, 0).r;
}

// Manual bilinear (R32Float is not filterable everywhere). Clamped at face edges: small seam
// artefacts are accepted for the spike.
fn elev_at(ll: vec2<f32>) -> f32 {
    let f = dir_to_face_texel(lonlat_to_dir(ll));
    let face = i32(f.x);
    let p = f.yz - 0.5;
    let i = vec2<i32>(floor(p));
    let t = fract(p);
    let e00 = elev_load(face, i);
    let e10 = elev_load(face, i + vec2<i32>(1, 0));
    let e01 = elev_load(face, i + vec2<i32>(0, 1));
    let e11 = elev_load(face, i + vec2<i32>(1, 1));
    return mix(mix(e00, e10, t.x), mix(e01, e11, t.x), t.y);
}

fn province_at(ll: vec2<f32>) -> u32 {
    let f = dir_to_face_texel(lonlat_to_dir(ll));
    let n = i32(params.view.w);
    let t = clamp(vec2<i32>(floor(f.yz)), vec2<i32>(0), vec2<i32>(n - 1));
    return textureLoad(prov_tex, t, i32(f.x), 0).r;
}

fn terrain_color(e: f32, lat: f32) -> vec3<f32> {
    var c: vec3<f32>;
    if (e < 0.0) {
        let t = clamp(-e / 4000.0, 0.0, 1.0);
        c = mix(vec3<f32>(0.55, 0.72, 0.74), vec3<f32>(0.20, 0.36, 0.50), sqrt(t));
    } else {
        let lo = vec3<f32>(0.52, 0.63, 0.38);
        let mid = vec3<f32>(0.76, 0.72, 0.49);
        let hi = vec3<f32>(0.60, 0.50, 0.39);
        let rock = vec3<f32>(0.66, 0.63, 0.60);
        let snow = vec3<f32>(0.96, 0.96, 0.97);
        if (e < 500.0) { c = mix(lo, mid, e / 500.0); }
        else if (e < 1600.0) { c = mix(mid, hi, (e - 500.0) / 1100.0); }
        else if (e < 3000.0) { c = mix(hi, rock, (e - 1600.0) / 1400.0); }
        else { c = mix(rock, snow, clamp((e - 3000.0) / 700.0, 0.0, 1.0)); }
    }
    let ice = smoothstep(1.17, 1.30, abs(lat));
    return mix(c, vec3<f32>(0.92, 0.94, 0.96), ice * select(0.7, 1.0, e >= 0.0));
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

// Full map colour for a framebuffer pixel position.
fn map_color(px: vec2<f32>) -> vec4<f32> {
    let paper = vec3<f32>(0.93, 0.88, 0.76);
    let ll = screen_to_lonlat(px);
    if (abs(ll.y) > FRAC_PI_2) {
        return vec4<f32>(paper * 0.82, 1.0);
    }
    let e = elev_at(ll);
    var col = terrain_color(e, ll.y);

    if (e >= 0.0) {
        // Hillshade from samples one cell or one pixel away, whichever is larger, in true distance.
        let d = max(FRAC_PI_2 / params.view.w * 0.75, 1.0 / params.view.z);
        let ex = elev_at(ll + vec2<f32>(d / max(cos(ll.y), 0.05), 0.0));
        let en = elev_at(ll + vec2<f32>(0.0, d));
        let k = 12.0 / (d * PLANET_RADIUS_M);
        let n = normalize(vec3<f32>(-(ex - e) * k, -(en - e) * k, 1.0));
        let light = normalize(vec3<f32>(-1.0, 1.0, 1.2));
        col = col * (0.55 + 0.55 * max(dot(n, light), 0.0));
    }

    col = mix(paper, col, params.layers.x);

    let prov = province_at(ll);
    if (prov != NO_PROVINCE && params.layers.y > 0.0) {
        let pal = textureLoad(pal_tex, vec2<i32>(i32(prov % 256u), i32(prov / 256u)), 0).rgb;
        col = mix(col, pal, params.layers.y * 0.55);
    }

    // Parchment tint and paper grain.
    col = col * mix(vec3<f32>(1.0), vec3<f32>(1.0, 0.95, 0.84), 0.45);
    col = col * (0.97 + 0.05 * hash2(floor(px)));
    return vec4<f32>(col, 1.0);
}

// ---------------------------------------------------------------------------------------------
// Border lines: each segment is a screen-space quad of 6 vertices, projected here. `wrap_k`
// selects which copy of the east-west wrapping map to draw (-1, 0, +1).

struct LineVert {
    clip: vec4<f32>,
    color: vec4<f32>,
    // Distance from the line centre in px, and the half width, for antialiasing.
    side_px: f32,
    half_width: f32,
};

fn line_vertex(p0: vec2<f32>, p1: vec2<f32>, corner: vec2<f32>, kind: f32, wrap_k: f32) -> LineVert {
    var out: LineVert;
    let s = params.view.z;
    let half = params.screen.xy * 0.5;
    // Same horizontal shift for both ends, chosen from p0, so a segment never tears.
    let d0 = p0.x - params.view.x;
    let shift = wrap_lon(d0) - d0 + wrap_k * 2.0 * PI;
    let a = vec2<f32>((p0.x - params.view.x + shift) * s, (params.view.y - p0.y) * s) + half;
    let b = vec2<f32>((p1.x - params.view.x + shift) * s, (params.view.y - p1.y) * s) + half;

    var hw = 0.7;
    var color = vec4<f32>(0.30, 0.22, 0.15, 0.9);
    if (kind > 1.5) {
        hw = 1.3;
        color = vec4<f32>(0.50, 0.13, 0.10, 0.85);
    } else if (kind > 0.5) {
        hw = 0.45;
        color = vec4<f32>(0.33, 0.27, 0.22, 0.6 * params.layers.w);
    }
    color.a = color.a * params.layers.z;

    let delta = b - a;
    let len = length(delta);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) { dir = delta / len; }
    let nrm = vec2<f32>(-dir.y, dir.x);
    let ext = hw + 1.0;
    // Extend along the segment by half a pixel so joints don't gap.
    let along = mix(-0.5, len + 0.5, corner.x);
    let p = a + dir * along + nrm * corner.y * ext;

    out.clip = vec4<f32>(p.x / params.screen.x * 2.0 - 1.0, 1.0 - p.y / params.screen.y * 2.0, 0.0, 1.0);
    // Hide invisible lines and the long, degenerate segments around the poles.
    if (color.a <= 0.001 || len > 0.25 * s) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.color = color;
    out.side_px = corner.y * ext;
    out.half_width = hw;
    return out;
}

fn line_fragment(color: vec4<f32>, side_px: f32, half_width: f32) -> vec4<f32> {
    let cov = clamp(half_width + 0.5 - abs(side_px), 0.0, 1.0);
    return vec4<f32>(color.rgb, color.a * cov);
}
