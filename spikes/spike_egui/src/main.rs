//! Renderer spike, egui + wgpu flavour: the map is a wgpu paint callback inside an egui
//! central panel; borders are a GPU vertex buffer; labels go through the egui painter.
//!
//! `spike_egui [--bench] [--n 408] [--provinces 5000] [--out DIR] [--tag NAME]`
//! GPU choice: set `WGPU_POWER_PREF=low` (integrated) or `high` (discrete) before running.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu, wgpu::util::DeviceExt as _};
use spike_world::borders::{extract_borders, line_vertices, LineVertex};
use spike_world::view::{bench_frame, BenchStats, Camera, MapUniforms, Options, Phase, BRUSH_AMOUNT, BRUSH_RADIUS};
use spike_world::world::{apply_brush, generate, place_name, GenParams, World};
use std::time::Instant;

fn main() -> eframe::Result {
    let opts = Options::from_args();
    let mut native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_title("spike_egui").with_inner_size([1920.0, 1080.0]).with_resizable(!opts.bench),
        ..Default::default()
    };
    // Vsync off so frame times measure work, not the monitor.
    native.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    eframe::run_native("spike_egui", native, Box::new(move |cc| Ok(Box::new(MapApp::new(cc, opts)))))
}

struct Label {
    text: String,
    lon: f64,
    lat: f64,
}

struct MapApp {
    opts: Options,
    world: World,
    segment_count: usize,
    labels: Vec<Label>,
    cam: Option<Camera>,
    terrain: f32,
    political: f32,
    lines: bool,
    tool: Tool,
    elev_tex: wgpu::Texture,
    queue: wgpu::Queue,
    adapter: String,
    // Benchmark state.
    frame: u32,
    last: Option<Instant>,
    stats: BenchStats,
    screenshot_sent: bool,
    gen_ms: f64,
    last_dt_ms: f64,
    target: (f64, f64),
}

#[derive(PartialEq, Clone, Copy)]
enum Tool {
    Pan,
    Raise,
    Lower,
}

/// GPU objects that live in egui's callback resource map (they must outlive the render pass).
struct MapGpu {
    map_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    lines: wgpu::Buffer,
    line_vertex_count: u32,
}

impl MapApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> Self {
        let t0 = Instant::now();
        let world = generate(&GenParams { n: opts.n, provinces: opts.provinces, ..Default::default() });
        let segments = extract_borders(&world);
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let labels = world
            .label_provinces(14)
            .into_iter()
            .map(|p| {
                let (lon, lat) = spike_world::cube::dir_to_lonlat(world.province_center[p]);
                Label { text: place_name(p, 7), lon, lat }
            })
            .collect();

        let rs = cc.wgpu_render_state.as_ref().expect("wgpu renderer");
        let device = &rs.device;
        let queue = &rs.queue;
        let n = world.grid.n as u32;

        let elev_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("elevation"),
                size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&world.elevation),
        );
        let prov_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("province"),
                size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&world.province),
        );
        let (pw, ph, pal) = world.palette_rgba();
        let pal_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("palette"),
                size: wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pal,
        );
        let array_view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
        };

        let tex_entry = |binding: u32, sample_type: wgpu::TextureSampleType, dim: wgpu::TextureViewDimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: dim, multisampled: false },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex_entry(1, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2Array),
                tex_entry(2, wgpu::TextureSampleType::Uint, wgpu::TextureViewDimension::D2Array),
                tex_entry(3, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2),
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map uniforms"),
            size: std::mem::size_of::<MapUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&array_view(&elev_tex)) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&array_view(&prov_tex)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&pal_tex.create_view(&Default::default())) },
            ],
        });

        let source = format!("{}\n{}", include_str!("map_egui.wgsl"), spike_world::MAP_COMMON_WGSL);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("map"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: rs.target_format, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let map_pipeline = pipeline("map", "vs_fullscreen", "fs_map", &[], None);
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32],
        };
        let line_pipeline = pipeline("lines", "vs_line", "fs_line", &[Some(line_layout)], Some(wgpu::BlendState::ALPHA_BLENDING));

        let verts = line_vertices(&segments);
        let lines = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("borders"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        rs.renderer.write().callback_resources.insert(MapGpu {
            map_pipeline,
            line_pipeline,
            bind_group,
            uniforms,
            lines,
            line_vertex_count: verts.len() as u32,
        });

        let adapter = format!("{} ({:?})", rs.adapter.get_info().name, rs.adapter.get_info().backend);
        let target = world.bench_target();
        MapApp {
            opts,
            segment_count: segments.len(),
            world,
            labels,
            cam: None,
            terrain: 1.0,
            political: 0.6,
            lines: true,
            tool: Tool::Pan,
            elev_tex,
            queue: rs.queue.clone(),
            adapter,
            frame: 0,
            last: None,
            stats: BenchStats::default(),
            screenshot_sent: false,
            gen_ms,
            last_dt_ms: 0.0,
            target,
        }
    }

    /// Brush at a map-relative pixel position; uploads only the touched rectangle of each face.
    fn brush(&mut self, cam: &Camera, x: f64, y: f64, w: f64, h: f64, amount: f32) {
        let t0 = Instant::now();
        let Some(cell) = cam.pick(&self.world.grid, x, y, w, h) else { return };
        let rects = apply_brush(&mut self.world, cell, BRUSH_RADIUS, amount);
        let mut bytes = 0u64;
        for r in &rects {
            let data = self.world.elevation_rect(r);
            let (rw, rh) = ((r.i1 - r.i0 + 1) as u32, (r.j1 - r.j0 + 1) as u32);
            bytes += data.len() as u64 * 4;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.elev_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: r.i0 as u32, y: r.j0 as u32, z: r.face as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(rw * 4), rows_per_image: Some(rh) },
                wgpu::Extent3d { width: rw, height: rh, depth_or_array_layers: 1 },
            );
        }
        self.stats.brush_cpu_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        self.stats.brush_upload_bytes.push(bytes);
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, hover: Option<usize>) {
        ui.heading("Fantasy map spike");
        ui.label("egui + wgpu");
        ui.separator();
        ui.add(egui::Slider::new(&mut self.terrain, 0.0..=1.0).text("terrain"));
        ui.add(egui::Slider::new(&mut self.political, 0.0..=1.0).text("political"));
        ui.checkbox(&mut self.lines, "borders & coasts");
        ui.separator();
        ui.label("Tool");
        ui.radio_value(&mut self.tool, Tool::Pan, "pan (drag) / zoom (wheel)");
        ui.radio_value(&mut self.tool, Tool::Raise, "raise land");
        ui.radio_value(&mut self.tool, Tool::Lower, "lower land");
        ui.separator();
        ui.label(format!("cells: {}", self.world.grid.cells()));
        ui.label(format!("provinces: {}", self.world.province_cells.len()));
        ui.label(format!("border segments: {}", self.segment_count));
        ui.label(format!("world gen: {:.0} ms", self.gen_ms));
        ui.label(format!("frame: {:.2} ms", self.last_dt_ms));
        ui.label(format!("GPU: {}", self.adapter));
        if let Some(c) = hover {
            ui.separator();
            ui.label(format!("cell {c}"));
            ui.label(format!("elevation {:.0} m", self.world.elevation[c]));
            let p = self.world.province[c];
            if p != spike_world::world::NO_PROVINCE {
                ui.label(format!("province {p} ({})", place_name(p as usize, 7)));
                ui.label(format!("faction {}", self.world.province_faction[p as usize]));
            }
        }
    }
}

struct MapCallback {
    uniforms: MapUniforms,
}

impl egui_wgpu::CallbackTrait for MapCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let gpu: &MapGpu = resources.get().expect("map resources");
        queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&self.uniforms));
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let gpu: &MapGpu = resources.get().expect("map resources");
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        pass.set_pipeline(&gpu.map_pipeline);
        pass.draw(0..3, 0..1);
        if self.uniforms.layers[2] > 0.0 {
            pass.set_pipeline(&gpu.line_pipeline);
            pass.set_vertex_buffer(0, gpu.lines.slice(..));
            // Three instances = three copies of the wrapping map (west, centre, east).
            pass.draw(0..gpu.line_vertex_count, 0..3);
        }
    }
}

impl eframe::App for MapApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = self.last.map(|l| now - l).unwrap_or_default();
        self.last = Some(now);
        self.last_dt_ms = dt.as_secs_f64() * 1000.0;
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point() as f64;

        let mut hover = None;
        egui::Panel::left("controls").exact_size(240.0).show(ui, |ui| {
            self.side_panel(ui, None);
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            let (w, h) = ((rect.width() as f64 * ppp).round(), (rect.height() as f64 * ppp).round());
            let origin = ((rect.min.x as f64 * ppp).round(), (rect.min.y as f64 * ppp).round());
            let mut cam = self.cam.unwrap_or_else(|| Camera::fit(h));

            if self.opts.bench {
                let bf = bench_frame(self.frame, w, h, self.target);
                cam = bf.camera;
                self.stats.record(bf.phase, dt);
                if let Some((x, y)) = bf.brush {
                    self.brush(&cam, x, y, w, h, BRUSH_AMOUNT);
                }
                if bf.phase == Phase::Done && !self.screenshot_sent {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                    self.screenshot_sent = true;
                }
                self.frame += 1;
            } else {
                let to_map = |p: egui::Pos2| ((p.x - rect.min.x) as f64 * ppp, (p.y - rect.min.y) as f64 * ppp);
                if let Some(p) = response.hover_pos() {
                    let (x, y) = to_map(p);
                    hover = cam.pick(&self.world.grid, x, y, w, h);
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y) as f64;
                    if scroll != 0.0 {
                        cam.zoom_at((scroll * 0.003).exp(), x, y, w, h);
                    }
                }
                if response.dragged() {
                    match self.tool {
                        Tool::Pan => {
                            let d = response.drag_delta();
                            cam.pan_pixels(d.x as f64 * ppp, d.y as f64 * ppp);
                        }
                        tool => {
                            if let Some(p) = response.interact_pointer_pos() {
                                let (x, y) = to_map(p);
                                let amount = if tool == Tool::Raise { BRUSH_AMOUNT } else { -BRUSH_AMOUNT };
                                self.brush(&cam, x, y, w, h, amount);
                            }
                        }
                    }
                }
            }
            self.cam = Some(cam);

            let uniforms = cam.uniforms(
                self.world.grid.n,
                w,
                h,
                origin,
                [self.terrain, self.political, if self.lines { 1.0 } else { 0.0 }, cam.province_line_alpha(h) as f32],
            );
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, MapCallback { uniforms }));
            self.paint_labels(ui.painter(), &cam, rect, ppp, w, h);
        });

        if hover.is_some() {
            egui::Area::new(egui::Id::new("hover")).fixed_pos(egui::pos2(250.0, 10.0)).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| self.hover_info(ui, hover.unwrap()));
            });
        }

        if self.opts.bench {
            self.check_screenshot(&ctx);
        }
        ctx.request_repaint();
    }
}

impl MapApp {
    fn hover_info(&self, ui: &mut egui::Ui, c: usize) {
        ui.label(format!("elevation {:.0} m", self.world.elevation[c]));
        let p = self.world.province[c];
        if p != spike_world::world::NO_PROVINCE {
            ui.label(format!("{} (faction {})", place_name(p as usize, 7), self.world.province_faction[p as usize]));
        }
    }

    fn paint_labels(&self, painter: &egui::Painter, cam: &Camera, rect: egui::Rect, ppp: f64, w: f64, h: f64) {
        let ink = egui::Color32::from_rgb(60, 40, 25);
        let font = egui::FontId::proportional(15.0);
        for l in &self.labels {
            let (x, y) = cam.lonlat_to_screen(l.lon, l.lat, w, h);
            let pos = rect.min + egui::vec2((x / ppp) as f32, (y / ppp) as f32);
            if rect.contains(pos) {
                painter.text(pos, egui::Align2::CENTER_CENTER, &l.text, font.clone(), ink);
            }
        }
        // One label curved along an arc, glyph by glyph (no native curved text).
        let (tlon, tlat) = self.target;
        let (x, y) = cam.lonlat_to_screen(tlon, tlat + 0.04, w, h);
        let centre = (x / ppp, y / ppp);
        let text = "THE OLD KINGDOM";
        let size = 22.0;
        let glyphs = spike_world::view::arc_layout(text.chars().count(), size * 0.75, centre.0, centre.1, 260.0);
        for (ch, (gx, gy, angle)) in text.chars().zip(glyphs) {
            let galley = painter.layout_no_wrap(ch.to_string(), egui::FontId::proportional(size as f32), egui::Color32::from_rgb(110, 30, 20));
            let pos = rect.min + egui::vec2(gx as f32, gy as f32);
            if !rect.contains(pos) {
                continue;
            }
            let anchor = pos - egui::vec2(galley.size().x * 0.5, galley.size().y * 0.5);
            painter.add(egui::epaint::TextShape::new(anchor, galley, egui::Color32::from_rgb(110, 30, 20)).with_angle_and_anchor(angle as f32, egui::Align2::CENTER_CENTER));
        }
    }

    fn check_screenshot(&mut self, ctx: &egui::Context) {
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None })
        });
        let Some(image) = shot else { return };
        let dir = std::path::Path::new(&self.opts.out_dir);
        std::fs::create_dir_all(dir).expect("create out dir");
        let tag = &self.opts.tag;
        let png_path = dir.join(format!("egui_{tag}.png"));
        let file = std::fs::File::create(&png_path).expect("png file");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.width() as u32, image.height() as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(image.as_raw())).expect("png write");
        let notes = format!("world gen {:.0} ms; partial texture upload per brush stroke; pixels_per_point {}", self.gen_ms, ctx.pixels_per_point());
        let json = self.stats.to_json(
            "egui",
            &self.adapter,
            self.world.grid.cells(),
            self.segment_count,
            (image.width() as u32, image.height() as u32),
            &notes,
        );
        std::fs::write(dir.join(format!("egui_{tag}.json")), json).expect("json write");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
