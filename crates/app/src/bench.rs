//! A scripted camera path for measuring frame times, and the JSON report it writes. Same phases and
//! frame counts as the renderer trial, minus the brush, so the numbers compare.

use crate::projection::Camera;
use std::f64::consts::PI;
use std::time::Duration;

pub const WARMUP: u32 = 60;
pub const PAN: u32 = 300;
pub const ZOOM: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Warmup,
    Pan,
    Zoom,
    Done,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Warmup => "warmup",
            Phase::Pan => "pan",
            Phase::Zoom => "zoom",
            Phase::Done => "done",
        }
    }
}

/// Camera for a frame: a full turn of the world at fit zoom, then a zoom to 8× onto `target`
/// (longitude, latitude).
pub fn frame(frame: u32, fit: Camera, target: (f64, f64)) -> (Phase, Camera) {
    let mut f = frame;
    if f < WARMUP {
        return (Phase::Warmup, fit);
    }
    f -= WARMUP;
    if f < PAN {
        let t = f as f64 / PAN as f64;
        return (Phase::Pan, Camera { lon0: sim::grid::wrap_lon(t * 2.0 * PI), ..fit });
    }
    f -= PAN;
    let t = (f as f64 / ZOOM as f64).min(1.0);
    let ty = fit.projection.forward(0.0, target.1).1;
    let cam = Camera { lon0: t * target.0, cy: t * ty, scale: fit.scale * 8f64.powf(t), ..fit };
    (if f < ZOOM { Phase::Zoom } else { Phase::Done }, cam)
}

#[derive(Default)]
pub struct Stats {
    pub frames: Vec<(Phase, f64)>,
}

impl Stats {
    pub fn record(&mut self, phase: Phase, dt: Duration) {
        if phase == Phase::Pan || phase == Phase::Zoom {
            self.frames.push((phase, dt.as_secs_f64() * 1000.0));
        }
    }

    pub fn to_json(&self, adapter: &str, cells: usize, segments: usize, size: (u32, u32), notes: &str) -> String {
        let mut s = format!(
            "{{\n  \"app\": \"fantasymap\",\n  \"adapter\": \"{}\",\n  \"cells\": {cells},\n  \"line_segments\": {segments},\n  \"window\": [{}, {}],\n",
            adapter.replace('"', "'"),
            size.0,
            size.1
        );
        for phase in [Phase::Pan, Phase::Zoom] {
            let v: Vec<f64> = self.frames.iter().filter(|(p, _)| *p == phase).map(|(_, t)| *t).collect();
            s += &format!("  \"{}\": {},\n", phase.name(), summary(&v));
        }
        s += &format!("  \"notes\": \"{}\"\n}}\n", notes.replace('"', "'"));
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
        "{{\"n\": {}, \"mean_ms\": {:.3}, \"p50_ms\": {:.3}, \"p95_ms\": {:.3}, \"p99_ms\": {:.3}, \"max_ms\": {:.3}}}",
        s.len(),
        mean,
        pct(0.5),
        pct(0.95),
        pct(0.99),
        s[s.len() - 1]
    )
}
