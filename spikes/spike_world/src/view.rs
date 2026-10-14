//! Map camera (equirectangular, wrapping east-west), picking, the shared benchmark script and
//! frame statistics. The projection maths is mirrored in `shaders/map_common.wgsl`.

use crate::cube::{lonlat_to_dir, wrap_lon, Grid};
use std::f64::consts::{FRAC_PI_2, PI};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Longitude / latitude at the centre of the view, radians.
    pub lon: f64,
    pub lat: f64,
    /// Pixels per radian.
    pub scale: f64,
}

impl Camera {
    /// Whole planet height fits the view.
    pub fn fit(h: f64) -> Self {
        Camera { lon: 0.0, lat: 0.0, scale: h / PI }
    }

    pub fn screen_to_lonlat(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        (wrap_lon(self.lon + (x - w * 0.5) / self.scale), self.lat - (y - h * 0.5) / self.scale)
    }

    /// Screen position of a point, using the copy of the wrapping map nearest the view centre.
    pub fn lonlat_to_screen(&self, lon: f64, lat: f64, w: f64, h: f64) -> (f64, f64) {
        (wrap_lon(lon - self.lon) * self.scale + w * 0.5, (self.lat - lat) * self.scale + h * 0.5)
    }

    pub fn pick(&self, grid: &Grid, x: f64, y: f64, w: f64, h: f64) -> Option<usize> {
        let (lon, lat) = self.screen_to_lonlat(x, y, w, h);
        (lat.abs() <= FRAC_PI_2).then(|| grid.dir_to_cell(lonlat_to_dir(lon, lat)))
    }

    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        self.lon = wrap_lon(self.lon - dx / self.scale);
        self.lat = (self.lat + dy / self.scale).clamp(-FRAC_PI_2, FRAC_PI_2);
    }

    /// Zoom keeping the point under the cursor fixed.
    pub fn zoom_at(&mut self, factor: f64, x: f64, y: f64, w: f64, h: f64) {
        let (lon, lat) = self.screen_to_lonlat(x, y, w, h);
        self.scale = (self.scale * factor).clamp(h / PI * 0.5, h / PI * 200.0);
        self.lon = wrap_lon(lon - (x - w * 0.5) / self.scale);
        self.lat = (lat + (y - h * 0.5) / self.scale).clamp(-FRAC_PI_2, FRAC_PI_2);
    }

    /// Province borders fade in as you zoom in.
    pub fn province_line_alpha(&self, h: f64) -> f64 {
        ((self.scale / (h / PI) - 1.5) / 2.5).clamp(0.0, 1.0)
    }

    /// Uniform block shared by both renderers' shaders.
    pub fn uniforms(&self, n: usize, w: f64, h: f64, origin: (f64, f64), layers: [f32; 4]) -> MapUniforms {
        MapUniforms {
            view: [self.lon as f32, self.lat as f32, self.scale as f32, n as f32],
            screen: [w as f32, h as f32, origin.0 as f32, origin.1 as f32],
            layers,
        }
    }
}

/// Matches `MapParams` in map_common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MapUniforms {
    pub view: [f32; 4],
    pub screen: [f32; 4],
    /// terrain opacity, political opacity, lines on (0/1), province-line alpha
    pub layers: [f32; 4],
}

// ---------------------------------------------------------------------------------------------
// Benchmark script: identical camera path and brush strokes for both apps.

pub const BENCH_WARMUP: u32 = 60;
pub const BENCH_PAN: u32 = 300;
pub const BENCH_ZOOM: u32 = 300;
pub const BENCH_BRUSH: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Warmup,
    Pan,
    Zoom,
    Brush,
    Done,
}

pub const PHASES: [Phase; 3] = [Phase::Pan, Phase::Zoom, Phase::Brush];

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Warmup => "warmup",
            Phase::Pan => "pan",
            Phase::Zoom => "zoom",
            Phase::Brush => "brush",
            Phase::Done => "done",
        }
    }
}

pub struct BenchFrame {
    pub phase: Phase,
    pub camera: Camera,
    /// Brush position in screen pixels, applied this frame.
    pub brush: Option<(f64, f64)>,
}

/// `target`: (lon, lat) to zoom into and brush around; both apps pass the largest province.
pub fn bench_frame(frame: u32, w: f64, h: f64, target: (f64, f64)) -> BenchFrame {
    let fit = Camera::fit(h);
    let zoom_target = Camera { lon: target.0, lat: target.1, scale: fit.scale * 8.0 };
    let mut f = frame;
    if f < BENCH_WARMUP {
        return BenchFrame { phase: Phase::Warmup, camera: fit, brush: None };
    }
    f -= BENCH_WARMUP;
    if f < BENCH_PAN {
        let t = f as f64 / BENCH_PAN as f64;
        return BenchFrame { phase: Phase::Pan, camera: Camera { lon: wrap_lon(t * 2.0 * PI), ..fit }, brush: None };
    }
    f -= BENCH_PAN;
    if f < BENCH_ZOOM {
        let t = f as f64 / BENCH_ZOOM as f64;
        let cam = Camera {
            lon: t * zoom_target.lon,
            lat: t * zoom_target.lat,
            scale: fit.scale * 8f64.powf(t),
        };
        return BenchFrame { phase: Phase::Zoom, camera: cam, brush: None };
    }
    f -= BENCH_ZOOM;
    if f < BENCH_BRUSH {
        // 50 strokes of 6 frames, walking around an ellipse.
        let theta = f as f64 * 0.021;
        let pos = (w * 0.5 + 0.12 * w * theta.cos(), h * 0.5 + 0.12 * h * theta.sin());
        return BenchFrame { phase: Phase::Brush, camera: zoom_target, brush: Some(pos) };
    }
    BenchFrame { phase: Phase::Done, camera: zoom_target, brush: None }
}

pub const BRUSH_RADIUS: usize = 10;
pub const BRUSH_AMOUNT: f32 = 60.0;

/// Collects per-phase frame times and per-stroke brush costs, and writes a small JSON report.
#[derive(Default)]
pub struct BenchStats {
    pub frames: Vec<(Phase, f64)>,
    /// CPU time of applying a brush stroke and issuing its texture upload.
    pub brush_cpu_ms: Vec<f64>,
    pub brush_upload_bytes: Vec<u64>,
}

impl BenchStats {
    pub fn record(&mut self, phase: Phase, dt: Duration) {
        if phase != Phase::Warmup && phase != Phase::Done {
            self.frames.push((phase, dt.as_secs_f64() * 1000.0));
        }
    }

    pub fn to_json(&self, app: &str, adapter: &str, cells: usize, segments: usize, size: (u32, u32), extra: &str) -> String {
        let mut s = format!(
            "{{\n  \"app\": \"{app}\",\n  \"adapter\": \"{}\",\n  \"cells\": {cells},\n  \"border_segments\": {segments},\n  \"window\": [{}, {}],\n",
            adapter.replace('"', "'"),
            size.0,
            size.1
        );
        for phase in PHASES {
            let v: Vec<f64> = self.frames.iter().filter(|(p, _)| *p == phase).map(|(_, t)| *t).collect();
            s += &format!("  \"{}\": {},\n", phase.name(), summary(&v));
        }
        s += &format!("  \"brush_cpu_ms\": {},\n", summary(&self.brush_cpu_ms));
        let bytes = self.brush_upload_bytes.iter().sum::<u64>() as f64 / self.brush_upload_bytes.len().max(1) as f64;
        s += &format!("  \"brush_upload_bytes_avg\": {bytes:.0},\n");
        s += &format!("  \"notes\": \"{}\"\n}}\n", extra.replace('"', "'"));
        s
    }
}

fn summary(v: &[f64]) -> String {
    if v.is_empty() {
        return "null".into();
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    format!(
        "{{\"n\": {}, \"mean_ms\": {:.3}, \"p50_ms\": {:.3}, \"p95_ms\": {:.3}, \"p99_ms\": {:.3}, \"max_ms\": {:.3}, \"fps_mean\": {:.0}}}",
        s.len(),
        mean,
        pct(0.5),
        pct(0.95),
        pct(0.99),
        s[s.len() - 1],
        1000.0 / mean
    )
}

/// Command-line options both apps accept.
#[derive(Clone, Debug)]
pub struct Options {
    pub bench: bool,
    pub n: usize,
    pub provinces: usize,
    pub out_dir: String,
    pub tag: String,
}

impl Options {
    pub fn from_args() -> Self {
        let mut o = Options {
            bench: false,
            n: 408,
            provinces: 5000,
            out_dir: "W:/temp/claude/fantasymap-spike/out".into(),
            tag: "default".into(),
        };
        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--bench" => o.bench = true,
                "--n" => {
                    i += 1;
                    o.n = args[i].parse().expect("--n <face size>");
                }
                "--provinces" => {
                    i += 1;
                    o.provinces = args[i].parse().expect("--provinces <count>");
                }
                "--out" => {
                    i += 1;
                    o.out_dir = args[i].clone();
                }
                "--tag" => {
                    i += 1;
                    o.tag = args[i].clone();
                }
                other => panic!("unknown argument {other}"),
            }
            i += 1;
        }
        o
    }
}

/// Per-glyph placement for a label curved along a circular arc (neither renderer does curved text
/// natively). Returns (x, y, rotation in radians) per character, centred on `(cx, cy)`.
pub fn arc_layout(chars: usize, advance: f64, cx: f64, cy: f64, radius: f64) -> Vec<(f64, f64, f64)> {
    let total = advance * chars as f64;
    let span = total / radius; // radians of arc covered
    (0..chars)
        .map(|k| {
            let t = -span * 0.5 + (k as f64 + 0.5) * advance / radius;
            // Arc bulges upward: centre of curvature below the label.
            let x = cx + radius * t.sin();
            let y = cy + radius - radius * t.cos();
            (x, y, t)
        })
        .collect()
}
