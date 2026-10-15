// The map: every screen pixel goes back through the projection to a point on the sphere, then to a
// cube face and a cell. Mirrors `projection.rs` and the sim's grid (`grid.rs`) exactly; the GPU
// picking test compares them.

struct MapParams {
    view: vec4<f32>,     // central meridian, projected y at the view centre, px per projected unit, face size n
    screen: vec4<f32>,   // map width, height, origin x, origin y (physical px)
    proj: vec4<f32>,     // projection (0 Equal Earth, 1 rectangle), first world copy for lines, px per point, unused
    layers0: vec4<f32>,  // opacity: terrain, temperature, rainfall, population
    layers1: vec4<f32>,  // opacity: famine, provinces; lines: coasts, rivers
    layers2: vec4<f32>,  // lines: province borders, graticule; opacity: drought, prices
    ids: vec4<u32>,      // hovered province, selected province (or NO_PROVINCE), unused x2
};

@group(0) @binding(0) var<uniform> params: MapParams;
@group(0) @binding(1) var terrain_tex: texture_2d_array<f32>;  // RGBA8: land colour, alpha 1 on land
@group(0) @binding(2) var fields_tex: texture_2d_array<f32>;   // RGBA8: sea depth, temperature, rainfall
@group(0) @binding(3) var prov_tex: texture_2d_array<u32>;     // R32Uint: province per cell
@group(0) @binding(4) var tables_tex: texture_2d_array<f32>;   // RGBA8: per-province colours
@group(0) @binding(5) var smooth_samp: sampler;
@group(0) @binding(6) var warp_tex: texture_2d_array<f32>;     // RG32Float: border wobble, in texels

const PI: f32 = 3.14159265358979;
const FRAC_PI_2: f32 = 1.57079632679490;
const FRAC_PI_4: f32 = 0.78539816339745;
const NO_PROVINCE: u32 = 0xffffffffu;
const TABLE_WIDTH: u32 = 256u;

// Equal Earth.
const A1: f32 = 1.340264;
const A2: f32 = -0.081106;
const A3: f32 = 0.000893;
const A4: f32 = 0.003796;
const M: f32 = 0.8660254037844386;
const EE_HALF_HEIGHT: f32 = 1.3173627591574;

const PAPER: vec3<f32> = vec3<f32>(0.93, 0.88, 0.76);
const INK: vec3<f32> = vec3<f32>(0.27, 0.19, 0.13);

fn is_rectangle() -> bool {
    return params.proj.x > 0.5;
}

fn wrap_lon(l: f32) -> f32 {
    return l - 2.0 * PI * floor((l + PI) / (2.0 * PI));
}

fn ee_y(t: f32) -> f32 {
    let t2 = t * t;
    return t * (A1 + A2 * t2 + t2 * t2 * t2 * (A3 + A4 * t2));
}

fn ee_dy(t: f32) -> f32 {
    let t2 = t * t;
    return A1 + 3.0 * A2 * t2 + t2 * t2 * t2 * (7.0 * A3 + 9.0 * A4 * t2);
}

fn half_height() -> f32 {
    return select(EE_HALF_HEIGHT, FRAC_PI_2, is_rectangle());
}

// Projected (x, y) of a longitude relative to the central meridian, and a latitude.
fn project(dlon: f32, lat: f32) -> vec2<f32> {
    if (is_rectangle()) {
        return vec2<f32>(dlon, lat);
    }
    let t = asin(M * sin(lat));
    return vec2<f32>(dlon * cos(t) / (M * ee_dy(t)), ee_y(t));
}

// Parametric latitude for a projected y, clamped to the map. Newton's method from a start within
// 0.07 of the answer; four steps reach float precision.
fn ee_theta(y: f32) -> f32 {
    let yc = clamp(y, -EE_HALF_HEIGHT, EE_HALF_HEIGHT);
    var t = yc / A1;
    for (var k = 0; k < 4; k++) {
        t = t - (ee_y(t) - yc) / ee_dy(t);
    }
    return t;
}

// (longitude relative to the central meridian, latitude, 1 if on the map else 0, projected x per
// radian of longitude at this y).
fn unproject(p: vec2<f32>) -> vec4<f32> {
    if (is_rectangle()) {
        let inside = abs(p.y) <= FRAC_PI_2;
        return vec4<f32>(p.x, clamp(p.y, -FRAC_PI_2, FRAC_PI_2), select(0.0, 1.0, inside), 1.0);
    }
    let t = ee_theta(p.y);
    let lat = asin(clamp(sin(t) / M, -1.0, 1.0));
    let xpl = cos(t) / (M * ee_dy(t));
    let dlon = p.x / xpl;
    let inside = abs(p.y) <= EE_HALF_HEIGHT && abs(dlon) <= PI;
    return vec4<f32>(dlon, lat, select(0.0, 1.0, inside), xpl);
}

// Map-relative pixel -> projected coordinates.
fn screen_to_projected(local: vec2<f32>) -> vec2<f32> {
    let s = params.view.z;
    return vec2<f32>((local.x - params.screen.x * 0.5) / s, params.view.y - (local.y - params.screen.y * 0.5) / s);
}

fn lonlat_to_dir(lon: f32, lat: f32) -> vec3<f32> {
    let c = cos(lat);
    return vec3<f32>(c * sin(lon), sin(lat), c * cos(lon));
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

// The cell under a map-relative pixel, or NO_PROVINCE off the map.
fn cell_at(local: vec2<f32>) -> u32 {
    let u = unproject(screen_to_projected(local));
    if (u.z < 0.5) {
        return NO_PROVINCE;
    }
    let f = dir_to_face_texel(lonlat_to_dir(wrap_lon(params.view.x + u.x), u.y));
    let n = u32(params.view.w);
    let t = clamp(vec2<u32>(floor(f.yz)), vec2<u32>(0u), vec2<u32>(n - 1u));
    return u32(f.x) * n * n + t.y * n + t.x;
}

fn ramp4(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>, t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    if (x < 1.0) { return mix(a, b, x); }
    if (x < 2.0) { return mix(b, c, x - 1.0); }
    return mix(c, d, x - 2.0);
}

fn temperature_color(t: f32) -> vec3<f32> {
    if (t < 0.5) {
        return ramp4(vec3<f32>(0.16, 0.24, 0.63), vec3<f32>(0.33, 0.56, 0.82), vec3<f32>(0.62, 0.80, 0.92), vec3<f32>(0.96, 0.96, 0.92), t * 2.0);
    }
    return ramp4(vec3<f32>(0.96, 0.96, 0.92), vec3<f32>(0.95, 0.80, 0.45), vec3<f32>(0.90, 0.55, 0.25), vec3<f32>(0.75, 0.16, 0.12), t * 2.0 - 1.0);
}

fn rainfall_color(t: f32) -> vec3<f32> {
    return ramp4(vec3<f32>(0.86, 0.76, 0.55), vec3<f32>(0.59, 0.75, 0.35), vec3<f32>(0.20, 0.55, 0.31), vec3<f32>(0.12, 0.31, 0.59), t);
}

fn table(prov: u32, layer: i32) -> vec4<f32> {
    return textureLoad(tables_tex, vec2<u32>(prov % TABLE_WIDTH, prov / TABLE_WIDTH), layer, 0);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Face texel `f` moved by the border wobble (`wobble.rs`): the bilinear blend of the four texels
// around it, in the same order as `wobble::warp_at`.
fn warped(f: vec3<f32>) -> vec3<f32> {
    let face = i32(f.x);
    let p = f.yz - 0.5;
    let b = vec2<i32>(floor(p)) + vec2<i32>(1);
    let t = fract(p);
    let top = mix(textureLoad(warp_tex, b, face, 0).rg, textureLoad(warp_tex, b + vec2<i32>(1, 0), face, 0).rg, t.x);
    let bottom = mix(textureLoad(warp_tex, b + vec2<i32>(0, 1), face, 0).rg, textureLoad(warp_tex, b + vec2<i32>(1, 1), face, 0).rg, t.x);
    return vec3<f32>(f.x, f.yz + mix(top, bottom, t.y));
}

// The province drawn at face texel `f`. Of the four cells around the point, the province holding
// the largest share of the bilinear weights wins; sea cells don't count. Its edge then runs smoothly
// through the middle of each cell side, as the border lines do, instead of stepping along cell
// edges. With `need_land`, points that are mostly sea have no province (for picking). Ties go to
// the first cell in the order below. `wobble::province_at` must give the same answer.
fn province_at(f: vec3<f32>, need_land: bool) -> u32 {
    // Written out for the four cells, without arrays or loops: much faster on integrated GPUs.
    let face = i32(f.x);
    let p = f.yz - 0.5;
    let b = vec2<i32>(floor(p)) + vec2<i32>(1);
    let t = fract(p);
    let i0 = textureLoad(prov_tex, b, face, 0).r;
    let i1 = textureLoad(prov_tex, b + vec2<i32>(1, 0), face, 0).r;
    let i2 = textureLoad(prov_tex, b + vec2<i32>(0, 1), face, 0).r;
    let i3 = textureLoad(prov_tex, b + vec2<i32>(1, 1), face, 0).r;
    if (i0 == i1 && i0 == i2 && i0 == i3) {
        return i0;
    }
    let w0 = (1.0 - t.x) * (1.0 - t.y);
    let w1 = t.x * (1.0 - t.y);
    let w2 = (1.0 - t.x) * t.y;
    let w3 = t.x * t.y;
    let l0 = select(w0, 0.0, i0 == NO_PROVINCE);
    let l1 = select(w1, 0.0, i1 == NO_PROVINCE);
    let l2 = select(w2, 0.0, i2 == NO_PROVINCE);
    let l3 = select(w3, 0.0, i3 == NO_PROVINCE);
    if (need_land && l0 + l1 + l2 + l3 < 0.5) {
        return NO_PROVINCE;
    }
    // Each cell's province's share of the weights (zero for sea cells).
    let s0 = select(0.0, l0 + select(0.0, l1, i1 == i0) + select(0.0, l2, i2 == i0) + select(0.0, l3, i3 == i0), l0 > 0.0);
    let s1 = select(0.0, l1 + select(0.0, l0, i0 == i1) + select(0.0, l2, i2 == i1) + select(0.0, l3, i3 == i1), l1 > 0.0);
    let s2 = select(0.0, l2 + select(0.0, l0, i0 == i2) + select(0.0, l1, i1 == i2) + select(0.0, l3, i3 == i2), l2 > 0.0);
    let s3 = select(0.0, l3 + select(0.0, l0, i0 == i3) + select(0.0, l1, i1 == i3) + select(0.0, l2, i2 == i3), l3 > 0.0);
    // Largest share; ties go to the earlier cell.
    var best = NO_PROVINCE;
    var best_s = 0.0;
    if (s0 > best_s) { best = i0; best_s = s0; }
    if (s1 > best_s) { best = i1; best_s = s1; }
    if (s2 > best_s) { best = i2; best_s = s2; }
    if (s3 > best_s) { best = i3; }
    return best;
}

// The planet's colour at a point on the map.
fn world_color(lon: f32, lat: f32) -> vec3<f32> {
    let f = dir_to_face_texel(lonlat_to_dir(lon, lat));
    let face = i32(f.x);
    let n = params.view.w;
    let uv = (f.yz + 1.0) / (n + 2.0);
    let ter = textureSampleLevel(terrain_tex, smooth_samp, uv, face, 0.0);
    let fld = textureSampleLevel(fields_tex, smooth_samp, uv, face, 0.0);
    let land = ter.a >= 0.5;
    let temp_c = fld.g * 65.0 - 30.0;

    var base: vec3<f32>;
    if (land) {
        base = ter.rgb;
    } else {
        base = mix(vec3<f32>(0.66, 0.78, 0.76), vec3<f32>(0.30, 0.45, 0.56), fld.r);
        base = mix(base, vec3<f32>(0.90, 0.92, 0.93), smoothstep(-8.0, -16.0, temp_c));
    }
    var col = mix(PAPER, base, params.layers0.x);
    col = mix(col, temperature_color(fld.g), params.layers0.y);
    if (!land) {
        return col;
    }
    col = mix(col, rainfall_color(fld.b), params.layers0.z);

    // Only look the province up when something on screen uses it.
    let wanted = params.layers0.w > 0.0 || params.layers1.x > 0.0 || params.layers1.y > 0.0 || params.layers2.z > 0.0 || params.layers2.w > 0.0
        || params.ids.x != NO_PROVINCE || params.ids.y != NO_PROVINCE;
    if (!wanted) {
        return col;
    }
    let prov = province_at(warped(f), false);
    if (prov == NO_PROVINCE) {
        return col;
    }
    let pop = table(prov, 1);
    col = mix(col, pop.rgb, params.layers0.w * pop.a);
    let dry = table(prov, 3);
    col = mix(col, dry.rgb, params.layers2.z * dry.a);
    let price = table(prov, 4);
    col = mix(col, price.rgb, params.layers2.w * price.a);
    let fam = table(prov, 2);
    col = mix(col, fam.rgb, params.layers1.x * fam.a);
    col = mix(col, table(prov, 0).rgb, params.layers1.y * 0.55);
    if (prov == params.ids.y) {
        col = mix(col, vec3<f32>(1.0, 0.92, 0.55), 0.38);
    } else if (prov == params.ids.x) {
        col = mix(col, vec3<f32>(1.0, 0.97, 0.86), 0.28);
    }
    return col;
}

// The page around the map, with a double ink rule along the map's edge.
// `xpl`: projected x per radian of longitude at this row.
fn page_color(p: vec2<f32>, xpl: f32) -> vec3<f32> {
    let s = params.view.z;
    let hh = half_height();
    var outside = (abs(p.y) - hh) * s;
    if (!is_rectangle()) {
        let edge_x = PI * xpl;
        outside = max(outside, (abs(p.x) - edge_x) * s);
    }
    let ppp = params.proj.z;
    var col = PAPER * 0.93;
    let inner = 1.0 - smoothstep(1.2 * ppp, 2.2 * ppp, outside);
    let outer = smoothstep(4.5 * ppp, 5.2 * ppp, outside) * (1.0 - smoothstep(5.8 * ppp, 6.5 * ppp, outside));
    col = mix(col, INK, max(inner, outer * 0.8) * 0.85);
    return col;
}

// Parchment tint, paper grain, faint blotches and a vignette, all fixed to the screen.
fn paper_finish(col: vec3<f32>, local: vec2<f32>) -> vec3<f32> {
    var c = col * mix(vec3<f32>(1.0), vec3<f32>(1.0, 0.95, 0.84), 0.40);
    c = c * (0.975 + 0.04 * hash2(floor(local)));
    // Two noise scales at different angles, so neither one's square grid shows.
    let r1 = mat2x2<f32>(0.80, -0.60, 0.60, 0.80) * local;
    let r2 = mat2x2<f32>(0.28, 0.96, -0.96, 0.28) * local;
    let blotch = value_noise(r1 / 210.0) * 0.6 + value_noise(r2 / 67.0) * 0.4;
    c = c * (0.975 + 0.035 * blotch);
    let q = local / params.screen.xy - 0.5;
    c = c * (1.0 - 0.28 * dot(q, q));
    return c;
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_map(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let local = pos.xy - params.screen.zw;
    let p = screen_to_projected(local);
    let u = unproject(p);
    var col: vec3<f32>;
    if (u.z > 0.5) {
        col = world_color(wrap_lon(params.view.x + u.x), u.y);
    } else {
        col = page_color(p, u.w);
    }
    return vec4<f32>(paper_finish(col, local), 1.0);
}

// Cell id under each pixel, for the picking test.
@fragment
fn fs_pick(@builtin(position) pos: vec4<f32>) -> @location(0) u32 {
    return cell_at(pos.xy - params.screen.zw);
}

// Province drawn at each pixel (NO_PROVINCE off the map or at sea), for the picking test.
@fragment
fn fs_pick_province(@builtin(position) pos: vec4<f32>) -> @location(0) u32 {
    let u = unproject(screen_to_projected(pos.xy - params.screen.zw));
    if (u.z < 0.5) {
        return NO_PROVINCE;
    }
    return province_at(warped(dir_to_face_texel(lonlat_to_dir(wrap_lon(params.view.x + u.x), u.y))), true);
}

// ---------------------------------------------------------------------------------------------
// Lines: each segment is an instanced screen-space quad of 6 vertices. On the repeating rectangle
// each segment is drawn once per visible copy of the world: vertex 6k.. is copy (first + k).

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Distance from the line centre in px, the half width, distance along the line in px, and the
    // longitude relative to the central meridian (to clip at the oval's edge).
    @location(1) side_px: f32,
    @location(2) half_width: f32,
    @location(3) along_px: f32,
    @location(4) dlon: f32,
    @location(5) @interpolate(flat) kind: u32,
};

@vertex
fn vs_line(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) kind: u32,
    @location(3) width: f32,
    @location(4) dist: f32,
) -> LineOut {
    var out: LineOut;
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi % 6u];
    let copy = f32(i32(vi / 6u) + i32(params.proj.y));
    let s = params.view.z;
    let half = params.screen.xy * 0.5;
    let ppp = params.proj.z;

    // Both ends shifted together, from the first end, so a segment never tears across the seam.
    let dla = wrap_lon(a.x - params.view.x) + copy * 2.0 * PI;
    let dlb = dla + wrap_lon(b.x - a.x);
    let pa = project(dla, a.y);
    let pb = project(dlb, b.y);
    let sa = vec2<f32>(pa.x * s, (params.view.y - pa.y) * s) + half;
    let sb = vec2<f32>(pb.x * s, (params.view.y - pb.y) * s) + half;

    var color: vec4<f32>;
    var hw = width * 0.5 * ppp;
    switch kind {
        case 0u: { color = vec4<f32>(INK, 0.95 * params.layers1.z); }
        case 1u: { color = vec4<f32>(0.42, 0.24, 0.18, 0.75 * params.layers2.x); }
        case 2u: { color = vec4<f32>(0.22, 0.40, 0.62, 0.9 * params.layers1.w); }
        default: { color = vec4<f32>(0.40, 0.30, 0.20, 0.35 * params.layers2.y); }
    }
    // Rivers thin out when zoomed far out.
    if (kind == 2u) {
        hw = hw * clamp(s / (params.screen.y * 0.6), 0.6, 1.0);
    }

    let delta = sb - sa;
    let len = length(delta);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) { dir = delta / len; }
    let nrm = vec2<f32>(-dir.y, dir.x);
    let ext = hw + 1.0;
    let along = mix(-0.5, len + 0.5, corner.x);
    let p = sa + dir * along + nrm * corner.y * ext;

    out.clip = vec4<f32>(p.x / params.screen.x * 2.0 - 1.0, 1.0 - p.y / params.screen.y * 2.0, 0.0, 1.0);
    // Hide invisible lines, anything absurdly long, and everything before the view is set up.
    if (color.a <= 0.001 || len > 0.5 * s || s <= 0.0 || params.screen.x < 1.0 || params.screen.y < 1.0) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.color = color;
    out.side_px = corner.y * ext;
    out.half_width = hw;
    out.along_px = dist * s + along;
    out.dlon = mix(dla, dlb, corner.x);
    out.kind = kind;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    if (!is_rectangle() && abs(in.dlon) > PI) {
        discard;
    }
    var a = in.color.a * clamp(in.half_width + 0.5 - abs(in.side_px), 0.0, 1.0);
    // Province borders and the graticule are dotted.
    if (in.kind == 1u || in.kind == 3u) {
        let period = 7.0 * params.proj.z;
        let on = 1.0 - smoothstep(0.55, 0.7, fract(in.along_px / period));
        a = a * on;
    }
    return vec4<f32>(in.color.rgb, a);
}
