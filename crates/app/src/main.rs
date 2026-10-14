//! The map view: generate a world, run it forward some years, and show it.
//!
//! fantasymap [--size small|medium|large|huge] [--seed S] [--n FACE] [--provinces N] [--years Y]
//!            [--projection oval|rect] [--view LON,LAT,ZOOM] [--layer NAME=VALUE]...
//!            [--screenshot FILE.png] [--bench --out DIR --tag NAME]
//!
//! Layer names: terrain, temperature, rainfall, population, famine, provinces (0..1), and coasts,
//! rivers, borders, graticule (0 or 1). `--view` takes degrees and a zoom (1 = planet height fits).
//! GPU choice: set `WGPU_POWER_PREF=low` (integrated) or `high` (discrete) before running.

use app::bench;
use app::gpu::{self, LayerSettings, MapData, MapGpu, MapUniforms};
use app::layers;
use app::projection::{Camera, Projection};
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use sim::config::Config;
use sim::grid::NONE;
use sim::sim::Sim;
use sim::world::{Size, WorldParams, RIVER_MIN_FLOW};
use std::time::Instant;

struct Options {
    params: WorldParams,
    config: Config,
    size: Size,
    years: i64,
    projection: Projection,
    view: Option<(f64, f64, f64)>,
    layers: LayerSettings,
    screenshot: Option<String>,
    bench: bool,
    out_dir: String,
    tag: String,
}

impl Options {
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut size = Size::Large;
        let mut seed = 7;
        let (mut n, mut provinces) = (None, None);
        let mut o = Options {
            params: WorldParams::preset(Size::Large, 7),
            config: Config::new(WorldParams::preset(Size::Large, 7)),
            size,
            years: 20,
            projection: Projection::EqualEarth,
            view: None,
            layers: LayerSettings::default(),
            screenshot: None,
            bench: false,
            out_dir: "W:/temp/claude/fantasymap-mapview/bench".into(),
            tag: "default".into(),
        };
        let mut i = 0;
        let value = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| panic!("{} needs a value", args[*i - 1]))
        };
        while i < args.len() {
            match args[i].as_str() {
                "--size" => {
                    size = match value(&mut i).as_str() {
                        "small" => Size::Small,
                        "medium" => Size::Medium,
                        "large" => Size::Large,
                        "huge" => Size::Huge,
                        other => panic!("unknown size {other}"),
                    }
                }
                "--seed" => seed = value(&mut i).parse().expect("--seed"),
                "--n" => n = Some(value(&mut i).parse().expect("--n")),
                "--provinces" => provinces = Some(value(&mut i).parse().expect("--provinces")),
                "--years" => o.years = value(&mut i).parse().expect("--years"),
                "--terrain" => o.config.modules.terrain = value(&mut i).parse().unwrap_or_else(|e| panic!("{e}")),
                "--climate" => o.config.modules.climate = value(&mut i).parse().unwrap_or_else(|e| panic!("{e}")),
                "--projection" => {
                    o.projection = match value(&mut i).as_str() {
                        "oval" => Projection::EqualEarth,
                        "rect" => Projection::Rectangle,
                        other => panic!("unknown projection {other} (oval or rect)"),
                    }
                }
                "--view" => {
                    let v: Vec<f64> = value(&mut i).split(',').map(|x| x.trim().parse().expect("--view LON,LAT,ZOOM")).collect();
                    assert_eq!(v.len(), 3, "--view LON,LAT,ZOOM");
                    o.view = Some((v[0], v[1], v[2]));
                }
                "--layer" => {
                    let v = value(&mut i);
                    let (name, x) = v.split_once('=').expect("--layer NAME=VALUE");
                    let x: f32 = x.parse().expect("--layer value");
                    let l = &mut o.layers;
                    match name {
                        "terrain" => l.terrain = x,
                        "temperature" => l.temperature = x,
                        "rainfall" => l.rainfall = x,
                        "population" => l.population = x,
                        "famine" => l.famine = x,
                        "provinces" => l.provinces = x,
                        "coasts" => l.coasts = x > 0.5,
                        "rivers" => l.rivers = x > 0.5,
                        "borders" => l.borders = x > 0.5,
                        "graticule" => l.graticule = x > 0.5,
                        other => panic!("unknown layer {other}"),
                    }
                }
                "--screenshot" => o.screenshot = Some(value(&mut i)),
                "--bench" => o.bench = true,
                "--out" => o.out_dir = value(&mut i),
                "--tag" => o.tag = value(&mut i),
                other => panic!("unknown argument {other}"),
            }
            i += 1;
        }
        o.size = size;
        o.params = WorldParams::preset(size, seed);
        if let Some(n) = n {
            o.params.n = n;
        }
        if let Some(p) = provinces {
            o.params.provinces = p;
        }
        o.config.world = o.params.clone();
        o
    }
}

fn main() -> eframe::Result {
    let opts = Options::from_args();
    let scripted = opts.bench || opts.screenshot.is_some();
    let mut native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_title("Fantasy Planet").with_inner_size([1920.0, 1080.0]).with_resizable(!scripted),
        ..Default::default()
    };
    if opts.bench {
        // Vsync off so frame times measure work, not the monitor.
        native.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native("fantasymap", native, Box::new(move |cc| Ok(Box::new(MapApp::new(cc, opts)))))
}

struct MapApp {
    opts: Options,
    sim: Sim,
    /// Province tables waiting for the next paint callback to upload them.
    pending_tables: Option<Vec<layers::Rgba>>,
    adapter: String,
    line_segments: usize,
    gen_s: f64,
    cam: Option<Camera>,
    projection: Projection,
    layers: LayerSettings,
    hover: Option<usize>,
    // Scripted runs.
    frame: u32,
    last: Option<Instant>,
    last_dt_ms: f64,
    stats: bench::Stats,
    screenshot_sent: bool,
    bench_target: (f64, f64),
}

impl MapApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> Self {
        let t0 = Instant::now();
        let mut sim = Sim::new(&opts.config).unwrap_or_else(|e| panic!("{e}"));
        let first = sim.calendar.def.first_year;
        sim.run_to_year(first + opts.years);
        let gen_s = t0.elapsed().as_secs_f64();
        let data = MapData::build(&sim);

        let rs = cc.wgpu_render_state.as_ref().expect("wgpu renderer");
        let gpu = MapGpu::new(&rs.device, &rs.queue, rs.target_format, &data);
        rs.renderer.write().callback_resources.insert(gpu);
        let info = rs.adapter.get_info();

        // The most populous province, for the benchmark zoom.
        let people = sim.world.province_population();
        let top = (0..people.len()).max_by_key(|&p| people[p]).unwrap_or(0);
        let bench_target = sim.world.provinces.center.get(top).map_or((0.0, 0.0), |&d| sim::grid::dir_to_lonlat(d));

        MapApp {
            projection: opts.projection,
            layers: opts.layers,
            opts,
            sim,
            pending_tables: None,
            adapter: format!("{} ({:?})", info.name, info.backend),
            line_segments: data.lines.len(),
            gen_s,
            cam: None,
            hover: None,
            frame: 0,
            last: None,
            last_dt_ms: 0.0,
            stats: bench::Stats::default(),
            screenshot_sent: false,
            bench_target,
        }
    }

    fn advance(&mut self, years: i64) {
        let year = self.sim.calendar.date(self.sim.day).year;
        self.sim.run_to_year(year + years);
        // Only the per-province tables change; the per-cell layers stay as they are.
        self.pending_tables = Some(layers::province_tables(&self.sim));
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let s = &self.sim;
        ui.heading("Fantasy Planet");
        ui.label(format!("{:?} world, seed {}", self.opts.size, s.world.params.seed));
        ui.label(s.calendar.format(s.day));
        ui.label(format!("{} people", group(sim::fields::quantity_total(&s.world, sim::fields::Quantity::People))));
        let mut years = 0;
        ui.horizontal(|ui| {
            if ui.button("+1 year").clicked() {
                years = 1;
            }
            if ui.button("+10 years").clicked() {
                years = 10;
            }
        });
        ui.separator();

        ui.label("Map shape");
        ui.radio_value(&mut self.projection, Projection::EqualEarth, "Oval world map (true areas)");
        ui.radio_value(&mut self.projection, Projection::Rectangle, "Rectangle (repeats sideways)");
        ui.separator();

        ui.label("Layers");
        let l = &mut self.layers;
        ui.add(egui::Slider::new(&mut l.terrain, 0.0..=1.0).text("terrain"));
        ui.add(egui::Slider::new(&mut l.temperature, 0.0..=1.0).text("temperature"));
        ui.add(egui::Slider::new(&mut l.rainfall, 0.0..=1.0).text("rainfall"));
        ui.add(egui::Slider::new(&mut l.population, 0.0..=1.0).text("population"));
        ui.add(egui::Slider::new(&mut l.famine, 0.0..=1.0).text("famine last year"));
        ui.add(egui::Slider::new(&mut l.provinces, 0.0..=1.0).text("province colours"));
        ui.checkbox(&mut l.coasts, "coastlines");
        ui.checkbox(&mut l.rivers, "rivers");
        ui.checkbox(&mut l.borders, "province borders (zoom in)");
        ui.checkbox(&mut l.graticule, "latitude and longitude lines");
        ui.separator();

        if let Some(c) = self.hover {
            self.cell_info(ui, c);
            ui.separator();
        }
        ui.small(format!("{} cells, {} provinces", s.world.grid.cells(), s.world.provinces.count));
        ui.small(format!("{} line segments", self.line_segments));
        ui.small(format!("world built and run in {:.1} s", self.gen_s));
        ui.small(format!("frame {:.1} ms", self.last_dt_ms));
        ui.small(format!("GPU: {}", self.adapter));
        if years > 0 {
            self.advance(years);
        }
    }

    fn cell_info(&self, ui: &mut egui::Ui, c: usize) {
        let w = &self.sim.world;
        let (lon, lat) = (w.grid.lon[c].to_degrees(), w.grid.lat[c].to_degrees());
        ui.label(format!("{:.1}°{} {:.1}°{}", lat.abs(), if lat >= 0.0 { "N" } else { "S" }, lon.abs(), if lon >= 0.0 { "E" } else { "W" }));
        let e = w.elevation[c];
        ui.label(if e < 0.0 { format!("sea, {:.0} m deep", -e) } else { format!("{}, {:.0} m", w.biome[c].name(), e) });
        ui.label(format!("{:.0} °C, {:.0} mm rain a year", w.temperature[c], w.rainfall[c]));
        if e >= 0.0 && w.river_flow[c] >= RIVER_MIN_FLOW {
            ui.label(format!("river, {:.0} m³/s", w.river_flow[c] as f64 * 1000.0 / (365.25 * 86400.0)));
        }
        let p = w.provinces.of_cell[c];
        if p != NONE {
            let p = p as usize;
            let people = w.province_population()[p];
            ui.label(format!("province {p}: {} people, {:.1} per km²", group(people), people as f64 / w.provinces.area_km2[p]));
            ui.label(format!("food stock {} rations", group(w.food_stock[p])));
            // The view always stops on the first day of a year, before the yearly reset, so the
            // sim's "this year" count is the year just ended.
            let starved = w.starved_this_year[p];
            if starved > 0 {
                ui.label(format!("{} starved last year", group(starved)));
            }
        }
    }

    fn check_screenshot(&mut self, ctx: &egui::Context) {
        let shot = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
        let Some(image) = shot else { return };
        let path = match &self.opts.screenshot {
            Some(p) => std::path::PathBuf::from(p),
            None => std::path::Path::new(&self.opts.out_dir).join(format!("fantasymap_{}.png", self.opts.tag)),
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create output folder");
        }
        let file = std::fs::File::create(&path).expect("png file");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.width() as u32, image.height() as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(image.as_raw())).expect("png write");
        if self.opts.bench {
            let notes = format!("projection {:?}; pixels_per_point {}", self.projection, ctx.pixels_per_point());
            let json = self.stats.to_json(&self.adapter, self.sim.world.grid.cells(), self.line_segments, (image.width() as u32, image.height() as u32), &notes);
            std::fs::write(path.with_extension("json"), json).expect("json write");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

struct MapCallback {
    uniforms: MapUniforms,
    copies: u32,
    tables: Option<Vec<layers::Rgba>>,
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
        if let Some(t) = &self.tables {
            gpu.upload_tables(queue, t);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let gpu: &MapGpu = resources.get().expect("map resources");
        gpu.paint(pass, &self.uniforms, self.copies);
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

        egui::Panel::left("controls").exact_size(270.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            let (w, h) = ((rect.width() as f64 * ppp).round(), (rect.height() as f64 * ppp).round());
            let origin = ((rect.min.x as f64 * ppp).round(), (rect.min.y as f64 * ppp).round());
            let mut cam = self.cam.unwrap_or_else(|| {
                let mut c = Camera::fit(self.projection, w, h);
                if let Some((lon, lat, zoom)) = self.opts.view {
                    c.lon0 = lon.to_radians();
                    c.cy = self.projection.forward(0.0, lat.to_radians()).1;
                    c.scale = c.reference_scale(h) * zoom;
                }
                c
            });
            if cam.projection != self.projection {
                cam = cam.with_projection(self.projection, h);
            }

            self.hover = None;
            if self.opts.bench {
                let fit = Camera::fit(self.projection, w, h);
                let (phase, c) = bench::frame(self.frame, fit, self.bench_target);
                cam = c;
                self.stats.record(phase, dt);
                if phase == bench::Phase::Done && !self.screenshot_sent {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                    self.screenshot_sent = true;
                }
            } else {
                let to_map = |p: egui::Pos2| ((p.x - rect.min.x) as f64 * ppp, (p.y - rect.min.y) as f64 * ppp);
                if let Some(p) = response.hover_pos() {
                    let (x, y) = to_map(p);
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y) as f64;
                    if scroll != 0.0 {
                        cam.zoom_at((scroll * 0.003).exp(), x, y, w, h);
                    }
                    self.hover = cam.pick(&self.sim.world.grid, x, y, w, h);
                }
                if response.dragged()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    let d = response.drag_delta();
                    let (_, y) = to_map(p);
                    cam.pan(d.x as f64 * ppp, d.y as f64 * ppp, y - d.y as f64 * ppp, h);
                }
                if self.opts.screenshot.is_some() && self.frame == 10 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                }
            }
            self.frame += 1;
            self.cam = Some(cam);

            let hover_province = self.hover.map_or(NONE, |c| self.sim.world.provinces.of_cell[c]);
            let uniforms = gpu::uniforms(&cam, self.sim.world.grid.n, w, h, origin, ppp, &self.layers, hover_province);
            let (first, last) = cam.visible_copies(w);
            let tables = self.pending_tables.take();
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, MapCallback { uniforms, copies: (last - first + 1) as u32, tables }));
        });

        if self.opts.bench || self.opts.screenshot.is_some() {
            self.check_screenshot(&ctx);
            ctx.request_repaint();
        }
    }
}

/// 1234567 -> "1,234,567".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
