// Bevy map material: bindings in the material bind group (2) and the fragment entry point.
// The shared map code is appended after this file.

@group(2) @binding(0) var<uniform> params: MapParams;
@group(2) @binding(1) var elev_tex: texture_2d_array<f32>;
@group(2) @binding(2) var prov_tex: texture_2d_array<u32>;
@group(2) @binding(3) var pal_tex: texture_2d<f32>;

@fragment
fn fragment(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = map_color(pos.xy);
    // Bevy's main target is sRGB: hand it linear values so colours match the egui build.
    return vec4<f32>(pow(c.rgb, vec3<f32>(2.2)), c.a);
}
