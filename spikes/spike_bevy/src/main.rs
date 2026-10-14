//! Renderer spike, Bevy flavour: the map is a `Material2d` on a big quad, borders are a custom-
//! attribute mesh with a line material (one entity per wrap copy), labels are `Text2d` entities,
//! the side panel is bevy_egui.
//!
//! `spike_bevy [--bench] [--n 408] [--provinces 5000] [--out DIR] [--tag NAME]`
//! GPU choice: `WGPU_POWER_PREF=low|high`. Brush upload: `BEVY_PARTIAL=1` uses a hand-written
//! render-world system that uploads only the touched rectangle; otherwise the idiomatic path
//! (mutate the `Image` asset; Bevy re-uploads the whole texture).

use bevy::{
    asset::{uuid_handle, RenderAssetUsages},
    camera::{visibility::{NoFrustumCulling, RenderLayers}, CameraOutputMode, Viewport},
    core_pipeline::tonemapping::Tonemapping,
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology},
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            AsBindGroup, BlendState, Extent3d, Origin3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
            TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDimension, TextureFormat, TextureViewDescriptor,
            TextureViewDimension, VertexFormat,
        },
        renderer::RenderQueue,
        texture::GpuImage,
        view::{screenshot::{save_to_disk, Screenshot, ScreenshotCaptured}, Msaa},
        Render, RenderApp, RenderSystems,
    },
    shader::ShaderRef,
    sprite_render::{AlphaMode2d, Material2d, Material2dKey, Material2dPipeline, Material2dPlugin},
    window::{PresentMode, PrimaryWindow, WindowResolution},
};
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext};
use spike_world::borders::{extract_borders, line_vertices};
use spike_world::view::{bench_frame, BenchStats, Camera, Options, Phase, BRUSH_AMOUNT, BRUSH_RADIUS};
use spike_world::world::{apply_brush, generate, place_name, DirtyRect, GenParams, World as Planet, NO_PROVINCE};
use std::time::Instant;

const MAP_SHADER: Handle<Shader> = uuid_handle!("6f1e8a52-0c3b-4d6e-9a41-6b9c2d7e1f01");
const LINE_SHADER: Handle<Shader> = uuid_handle!("6f1e8a52-0c3b-4d6e-9a41-6b9c2d7e1f02");

const ATTR_P0: MeshVertexAttribute = MeshVertexAttribute::new("LineP0", 0x5EED_0001, VertexFormat::Float32x2);
const ATTR_P1: MeshVertexAttribute = MeshVertexAttribute::new("LineP1", 0x5EED_0002, VertexFormat::Float32x2);
const ATTR_CORNER: MeshVertexAttribute = MeshVertexAttribute::new("LineCorner", 0x5EED_0003, VertexFormat::Float32x2);
const ATTR_KIND: MeshVertexAttribute = MeshVertexAttribute::new("LineKind", 0x5EED_0004, VertexFormat::Float32);

const PANEL_WIDTH: f32 = 240.0;

/// Matches `MapParams` in map_common.wgsl.
#[derive(ShaderType, Clone, Copy, Default, Debug)]
struct MapParams {
    view: Vec4,
    screen: Vec4,
    layers: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
struct MapMaterial {
    #[uniform(0)]
    params: MapParams,
    #[texture(1, dimension = "2d_array", sample_type = "float", filterable = false)]
    elevation: Handle<Image>,
    #[texture(2, dimension = "2d_array", sample_type = "u_int")]
    province: Handle<Image>,
    #[texture(3)]
    palette: Handle<Image>,
}

impl Material2d for MapMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(MAP_SHADER)
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
struct LineMaterial {
    #[uniform(0)]
    params: MapParams,
    #[uniform(1)]
    wrap: Vec4,
}

impl Material2d for LineMaterial {
    fn vertex_shader() -> ShaderRef {
        ShaderRef::Handle(LINE_SHADER)
    }
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(LINE_SHADER)
    }
    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
    fn specialize(
        _pipeline: &Material2dPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            ATTR_P0.at_shader_location(0),
            ATTR_P1.at_shader_location(1),
            ATTR_CORNER.at_shader_location(2),
            ATTR_KIND.at_shader_location(3),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        if let Some(fragment) = descriptor.fragment.as_mut() {
            for target in fragment.targets.iter_mut().flatten() {
                target.blend = Some(BlendState::ALPHA_BLENDING);
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tool {
    Pan,
    Raise,
    Lower,
}

#[derive(Resource)]
struct MapState {
    opts: Options,
    planet: Planet,
    segment_count: usize,
    cam: Option<Camera>,
    terrain: f32,
    political: f32,
    lines: bool,
    tool: Tool,
    partial_upload: bool,
    elevation: Handle<Image>,
    map_material: Handle<MapMaterial>,
    line_materials: Vec<Handle<LineMaterial>>,
    target: (f64, f64),
    gen_ms: f64,
    // Benchmark state.
    frame: u32,
    last: Option<Instant>,
    last_dt_ms: f64,
    stats: BenchStats,
    screenshot_sent: bool,
    exit_in: Option<u32>,
    adapter: String,
    /// Map rectangle in physical pixels: (origin x, origin y, width, height).
    map_rect: (f64, f64, f64, f64),
    hover: Option<usize>,
}

/// Rectangles touched by the brush this frame, for the partial-upload path.
#[derive(Resource, Clone, Default, ExtractResource)]
struct PendingUploads {
    image: Option<AssetId<Image>>,
    rects: Vec<(DirtyRect, Vec<f32>)>,
}

#[derive(Component)]
struct MapCamera;

#[derive(Component)]
struct PlaceLabel {
    lon: f64,
    lat: f64,
}

#[derive(Component)]
struct ArcGlyph(usize);

const ARC_TEXT: &str = "THE OLD KINGDOM";
const ARC_SIZE: f64 = 22.0;

fn main() {
    let opts = Options::from_args();
    let partial = std::env::var("BEVY_PARTIAL").is_ok_and(|v| v == "1");
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "spike_bevy".into(),
                resolution: WindowResolution::new(1920, 1080),
                present_mode: PresentMode::AutoNoVsync,
                resizable: !opts.bench,
                ..default()
            }),
            ..default()
        }),
    )
    .add_plugins((
        EguiPlugin::default(),
        Material2dPlugin::<MapMaterial>::default(),
        Material2dPlugin::<LineMaterial>::default(),
        ExtractResourcePlugin::<PendingUploads>::default(),
    ))
    .insert_resource(ClearColor(Color::srgb(0.1, 0.1, 0.1)))
    .init_resource::<PendingUploads>()
    .insert_resource(StartupOptions { opts, partial })
    .add_systems(Startup, setup)
    .add_systems(First, clear_uploads)
    .add_systems(Update, (drive_camera, update_materials, update_labels).chain())
    .add_systems(EguiPrimaryContextPass, side_panel)
    .add_systems(Last, finish_bench);
    app.sub_app_mut(RenderApp).add_systems(Render, upload_partial.in_set(RenderSystems::PrepareResources));
    app.run();
}

#[derive(Resource)]
struct StartupOptions {
    opts: Options,
    partial: bool,
}

fn array_image(n: u32, format: TextureFormat, bytes: Vec<u8>, keep_cpu_copy: bool) -> Image {
    let usage = if keep_cpu_copy { RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD } else { RenderAssetUsages::RENDER_WORLD };
    let mut img = Image::new(Extent3d { width: n, height: n, depth_or_array_layers: 6 }, TextureDimension::D2, bytes, format, usage);
    img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
    img
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    startup: Res<StartupOptions>,
    mut shaders: ResMut<Assets<Shader>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut map_materials: ResMut<Assets<MapMaterial>>,
    mut line_materials: ResMut<Assets<LineMaterial>>,
    mut egui_settings: ResMut<EguiGlobalSettings>,
    adapter: Res<bevy::render::renderer::RenderAdapterInfo>,
) {
    let opts = startup.opts.clone();
    let t0 = Instant::now();
    let planet = generate(&GenParams { n: opts.n, provinces: opts.provinces, ..default() });
    let segments = extract_borders(&planet);
    let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let common = spike_world::MAP_COMMON_WGSL;
    let _ = shaders.insert(MAP_SHADER.id(), Shader::from_wgsl(format!("{}\n{}", include_str!("map_bevy.wgsl"), common), "map_bevy.wgsl"));
    let _ = shaders.insert(LINE_SHADER.id(), Shader::from_wgsl(format!("{}\n{}", include_str!("lines_bevy.wgsl"), common), "lines_bevy.wgsl"));

    let n = planet.grid.n as u32;
    let elevation = images.add(array_image(n, TextureFormat::R32Float, bytemuck::cast_slice(&planet.elevation).to_vec(), !startup.partial));
    let province = images.add(array_image(n, TextureFormat::R32Uint, bytemuck::cast_slice(&planet.province).to_vec(), false));
    let (pw, ph, pal) = planet.palette_rgba();
    let palette = images.add(Image::new(
        Extent3d { width: pw, height: ph, depth_or_array_layers: 1 },
        TextureDimension::D2,
        pal,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ));

    // Map camera (viewport set every frame to the area right of the panel) and the egui camera.
    egui_settings.auto_create_primary_context = false;
    commands.spawn((Camera2d, MapCamera, Msaa::Off, Tonemapping::None));
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        RenderLayers::none(),
        Msaa::Off,
        Camera {
            order: 1,
            output_mode: CameraOutputMode::Write { blend_state: Some(BlendState::ALPHA_BLENDING), clear_color: ClearColorConfig::None },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));

    let map_material = map_materials.add(MapMaterial { params: MapParams::default(), elevation: elevation.clone(), province, palette });
    commands.spawn((Mesh2d(meshes.add(Rectangle::new(100_000.0, 100_000.0))), MeshMaterial2d(map_material.clone()), Transform::default()));

    let verts = line_vertices(&segments);
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32, 0.0, 0.0]; verts.len()]);
    mesh.insert_attribute(ATTR_P0, verts.iter().map(|v| v.p0).collect::<Vec<_>>());
    mesh.insert_attribute(ATTR_P1, verts.iter().map(|v| v.p1).collect::<Vec<_>>());
    mesh.insert_attribute(ATTR_CORNER, verts.iter().map(|v| v.corner).collect::<Vec<_>>());
    mesh.insert_attribute(ATTR_KIND, verts.iter().map(|v| v.kind).collect::<Vec<_>>());
    let line_mesh = meshes.add(mesh);
    let mut line_handles = Vec::new();
    for k in [-1.0f32, 0.0, 1.0] {
        let h = line_materials.add(LineMaterial { params: MapParams::default(), wrap: Vec4::new(k, 0.0, 0.0, 0.0) });
        commands.spawn((Mesh2d(line_mesh.clone()), MeshMaterial2d(h.clone()), Transform::from_xyz(0.0, 0.0, 1.0), NoFrustumCulling));
        line_handles.push(h);
    }

    let ink = Color::srgb_u8(60, 40, 25);
    for p in planet.label_provinces(14) {
        let (lon, lat) = spike_world::cube::dir_to_lonlat(planet.province_center[p]);
        commands.spawn((
            Text2d::new(place_name(p, 7)),
            TextFont { font_size: FontSize::Px(15.0), ..default() },
            TextColor(ink),
            Transform::from_xyz(0.0, 0.0, 2.0),
            PlaceLabel { lon, lat },
        ));
    }
    for (k, ch) in ARC_TEXT.chars().enumerate() {
        commands.spawn((
            Text2d::new(ch.to_string()),
            TextFont { font_size: FontSize::Px(ARC_SIZE as f32), ..default() },
            TextColor(Color::srgb_u8(110, 30, 20)),
            Transform::from_xyz(0.0, 0.0, 2.0),
            ArcGlyph(k),
        ));
    }

    let target = planet.bench_target();
    commands.insert_resource(MapState {
        opts,
        segment_count: segments.len(),
        planet,
        cam: None,
        terrain: 1.0,
        political: 0.6,
        lines: true,
        tool: Tool::Pan,
        partial_upload: startup.partial,
        elevation,
        map_material,
        line_materials: line_handles,
        target,
        gen_ms,
        frame: 0,
        last: None,
        last_dt_ms: 0.0,
        stats: BenchStats::default(),
        screenshot_sent: false,
        exit_in: None,
        adapter: format!("{} ({:?})", adapter.name, adapter.backend),
        map_rect: (0.0, 0.0, 1.0, 1.0),
        hover: None,
    });
}

fn clear_uploads(mut pending: ResMut<PendingUploads>) {
    if !pending.rects.is_empty() {
        pending.rects.clear();
    }
}

fn side_panel(
    mut contexts: EguiContexts,
    state: Option<ResMut<MapState>>,
    mut camera: Single<&mut bevy::camera::Camera, With<MapCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
) -> Result {
    let Some(mut state) = state else { return Ok(()) };
    let ctx = contexts.ctx_mut()?;
    let hover = state.hover;
    egui::Panel::left("controls").exact_size(PANEL_WIDTH).show(ctx, |ui| {
        ui.heading("Fantasy map spike");
        ui.label("Bevy + bevy_egui");
        ui.separator();
        ui.add(egui::Slider::new(&mut state.terrain, 0.0..=1.0).text("terrain"));
        ui.add(egui::Slider::new(&mut state.political, 0.0..=1.0).text("political"));
        ui.checkbox(&mut state.lines, "borders & coasts");
        ui.separator();
        ui.label("Tool");
        ui.radio_value(&mut state.tool, Tool::Pan, "pan (drag) / zoom (wheel)");
        ui.radio_value(&mut state.tool, Tool::Raise, "raise land");
        ui.radio_value(&mut state.tool, Tool::Lower, "lower land");
        ui.separator();
        ui.label(format!("cells: {}", state.planet.grid.cells()));
        ui.label(format!("provinces: {}", state.planet.province_cells.len()));
        ui.label(format!("border segments: {}", state.segment_count));
        ui.label(format!("world gen: {:.0} ms", state.gen_ms));
        ui.label(format!("frame: {:.2} ms", state.last_dt_ms));
        ui.label(format!("GPU: {}", state.adapter));
        ui.label(format!("brush upload: {}", if state.partial_upload { "partial (custom)" } else { "whole texture (idiomatic)" }));
        if let Some(c) = hover {
            ui.separator();
            ui.label(format!("elevation {:.0} m", state.planet.elevation[c]));
            let p = state.planet.province[c];
            if p != NO_PROVINCE {
                ui.label(format!("{} (faction {})", place_name(p as usize, 7), state.planet.province_faction[p as usize]));
            }
        }
    });

    let sf = window.scale_factor() as f64;
    let left = (PANEL_WIDTH as f64 * sf).round();
    let (w, h) = (window.physical_width() as f64 - left, window.physical_height() as f64);
    state.map_rect = (left, 0.0, w.max(1.0), h.max(1.0));
    camera.viewport = Some(Viewport {
        physical_position: UVec2::new(left as u32, 0),
        physical_size: UVec2::new(w.max(1.0) as u32, h.max(1.0) as u32),
        ..default()
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn drive_camera(
    state: Option<ResMut<MapState>>,
    mut images: ResMut<Assets<Image>>,
    mut pending: ResMut<PendingUploads>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let Some(mut state) = state else { return };
    let now = Instant::now();
    let dt = state.last.map(|l| now - l).unwrap_or_default();
    state.last = Some(now);
    state.last_dt_ms = dt.as_secs_f64() * 1000.0;
    let (ox, oy, w, h) = state.map_rect;
    let mut cam = state.cam.unwrap_or_else(|| Camera::fit(h));
    let sf = window.scale_factor() as f64;

    let mut strokes: Vec<((f64, f64), f32)> = Vec::new();
    if state.opts.bench {
        let bf = bench_frame(state.frame, w, h, state.target);
        cam = bf.camera;
        state.stats.record(bf.phase, dt);
        if let Some(p) = bf.brush {
            strokes.push((p, BRUSH_AMOUNT));
        }
        state.frame += 1;
    } else if let Some(cursor) = window.cursor_position() {
        let (x, y) = (cursor.x as f64 * sf - ox, cursor.y as f64 * sf - oy);
        if x >= 0.0 {
            state.hover = cam.pick(&state.planet.grid, x, y, w, h);
            if scroll.delta.y != 0.0 {
                cam.zoom_at((scroll.delta.y as f64 * 0.15).exp(), x, y, w, h);
            }
            if mouse.pressed(MouseButton::Left) {
                match state.tool {
                    Tool::Pan => cam.pan_pixels(motion.delta.x as f64 * sf, motion.delta.y as f64 * sf),
                    tool => strokes.push(((x, y), if tool == Tool::Raise { BRUSH_AMOUNT } else { -BRUSH_AMOUNT })),
                }
            }
        }
    }
    state.cam = Some(cam);

    for ((x, y), amount) in strokes {
        let t0 = Instant::now();
        let Some(cell) = cam.pick(&state.planet.grid, x, y, w, h) else { continue };
        let rects = apply_brush(&mut state.planet, cell, BRUSH_RADIUS, amount);
        let mut bytes = 0u64;
        if state.partial_upload {
            pending.image = Some(state.elevation.id());
            for r in rects {
                let data = state.planet.elevation_rect(&r);
                bytes += data.len() as u64 * 4;
                pending.rects.push((r, data));
            }
        } else {
            // Idiomatic: write into the Image asset; Bevy then re-extracts and re-uploads all of it.
            let n = state.planet.grid.n;
            let image = images.get_mut(&state.elevation).expect("elevation image");
            let data = image.data.as_mut().expect("cpu copy");
            for r in rects {
                for j in r.j0..=r.j1 {
                    for i in r.i0..=r.i1 {
                        let idx = (r.face * n + j) * n + i;
                        data[idx * 4..idx * 4 + 4].copy_from_slice(&state.planet.elevation[idx].to_le_bytes());
                    }
                }
            }
            bytes = data.len() as u64;
        }
        state.stats.brush_cpu_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        state.stats.brush_upload_bytes.push(bytes);
    }
}

fn update_materials(state: Option<Res<MapState>>, mut maps: ResMut<Assets<MapMaterial>>, mut lines: ResMut<Assets<LineMaterial>>) {
    let Some(state) = state else { return };
    let Some(cam) = state.cam else { return };
    let (ox, oy, w, h) = state.map_rect;
    let u = cam.uniforms(
        state.planet.grid.n,
        w,
        h,
        (ox, oy),
        [state.terrain, state.political, if state.lines { 1.0 } else { 0.0 }, cam.province_line_alpha(h) as f32],
    );
    let params = MapParams { view: Vec4::from_array(u.view), screen: Vec4::from_array(u.screen), layers: Vec4::from_array(u.layers) };
    if let Some(mut m) = maps.get_mut(&state.map_material) {
        m.params = params;
    }
    for h in &state.line_materials {
        if let Some(mut m) = lines.get_mut(h) {
            m.params = params;
        }
    }
}

fn update_labels(
    state: Option<Res<MapState>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut labels: Query<(&PlaceLabel, &mut Transform, &mut Visibility), Without<ArcGlyph>>,
    mut glyphs: Query<(&ArcGlyph, &mut Transform, &mut Visibility), Without<PlaceLabel>>,
) {
    let Some(state) = state else { return };
    let Some(cam) = state.cam else { return };
    let (_, _, w, h) = state.map_rect;
    let sf = window.scale_factor() as f64;
    // Camera2d world units are logical pixels, origin at the viewport centre, y up.
    let to_world = |x: f64, y: f64| Vec2::new(((x - w * 0.5) / sf) as f32, ((h * 0.5 - y) / sf) as f32);
    let inside = |x: f64, y: f64| x >= 0.0 && y >= 0.0 && x <= w && y <= h;
    for (l, mut t, mut vis) in &mut labels {
        let (x, y) = cam.lonlat_to_screen(l.lon, l.lat, w, h);
        t.translation = to_world(x, y).extend(2.0);
        *vis = if inside(x, y) { Visibility::Inherited } else { Visibility::Hidden };
    }
    let (x, y) = cam.lonlat_to_screen(state.target.0, state.target.1 + 0.04, w, h);
    let layout = spike_world::view::arc_layout(ARC_TEXT.chars().count(), ARC_SIZE * 0.75 * sf, x, y, 260.0 * sf);
    for (g, mut t, mut vis) in &mut glyphs {
        let (gx, gy, angle) = layout[g.0];
        t.translation = to_world(gx, gy).extend(2.0);
        t.rotation = Quat::from_rotation_z(-angle as f32);
        *vis = if inside(gx, gy) { Visibility::Inherited } else { Visibility::Hidden };
    }
}

fn finish_bench(mut commands: Commands, state: Option<ResMut<MapState>>, mut exit: MessageWriter<AppExit>, window: Single<&Window, With<PrimaryWindow>>) {
    let Some(mut state) = state else { return };
    if !state.opts.bench {
        return;
    }
    let (_, _, w, h) = state.map_rect;
    let phase = bench_frame(state.frame, w, h, state.target).phase;
    if phase == Phase::Done && !state.screenshot_sent {
        state.screenshot_sent = true;
        let dir = std::path::PathBuf::from(&state.opts.out_dir);
        std::fs::create_dir_all(&dir).expect("out dir");
        let png = dir.join(format!("bevy_{}.png", state.opts.tag));
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(png)).observe(|_: On<ScreenshotCaptured>, mut s: ResMut<MapState>| {
            s.exit_in = Some(5);
        });
    }
    if let Some(k) = state.exit_in {
        if k == 0 {
            let notes = format!(
                "world gen {:.0} ms; brush upload {}; scale factor {}",
                state.gen_ms,
                if state.partial_upload { "partial via custom render-world system" } else { "idiomatic whole-image re-upload" },
                window.scale_factor()
            );
            let json = state.stats.to_json(
                "bevy",
                &state.adapter,
                state.planet.grid.cells(),
                state.segment_count,
                (window.physical_width(), window.physical_height()),
                &notes,
            );
            std::fs::write(std::path::Path::new(&state.opts.out_dir).join(format!("bevy_{}.json", state.opts.tag)), json).expect("json");
            exit.write(AppExit::Success);
        } else {
            state.exit_in = Some(k - 1);
        }
    }
}

/// Render world: upload just the brushed rectangles into the existing GPU texture.
fn upload_partial(pending: Res<PendingUploads>, gpu_images: Res<RenderAssets<GpuImage>>, queue: Res<RenderQueue>) {
    let Some(id) = pending.image else { return };
    let Some(gpu) = gpu_images.get(id) else { return };
    for (r, data) in &pending.rects {
        let (rw, rh) = ((r.i1 - r.i0 + 1) as u32, (r.j1 - r.j0 + 1) as u32);
        queue.write_texture(
            TexelCopyTextureInfo { texture: &gpu.texture, mip_level: 0, origin: Origin3d { x: r.i0 as u32, y: r.j0 as u32, z: r.face as u32 }, aspect: TextureAspect::All },
            bytemuck::cast_slice(data),
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(rw * 4), rows_per_image: Some(rh) },
            Extent3d { width: rw, height: rh, depth_or_array_layers: 1 },
        );
    }
}
