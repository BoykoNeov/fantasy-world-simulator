//! Weather v0, "droughts": years when the rain doesn't come.
//!
//! The module owns `province.rain_shortfall`, the share of a province's normal rain that is
//! missing (per mille). A drought starts with a [`Event::Drought`], usually a player's poke: within
//! its radius rain falls short by the drought's strength, full in the inner half and fading to
//! nothing at the edge. Where droughts overlap, the worst one counts. Droughts that have run their
//! course are cleared on the next month start. Readers such as the harvest only ever see the field.

use crate::events::{Event, EventKind};
use crate::grid::{dir_to_lonlat, lonlat_to_dir};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::world::World;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drought {
    pub lon: f64,
    pub lat: f64,
    pub radius_km: f64,
    pub shortfall: u64,
    /// The first day without it.
    pub until: u64,
}

#[derive(Default)]
pub struct Droughts {
    pub active: Vec<Drought>,
}

static V0: Manifest = Manifest {
    slot: Slot::Weather,
    version: "droughts",
    tier: crate::fields::Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["province.center"],
    owns: &["province.rain_shortfall"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &[],
    emits: &[],
    handles: &[EventKind::Drought],
    state_schema: "droughts-1",
    goods: &[],
};

impl Droughts {
    /// Recompute every province's shortfall from the droughts in force, explaining the traced one.
    fn paint(&self, w: &mut World, cx: &mut Ctx) {
        let p = &w.provinces;
        let radius = w.params.radius_km;
        let mut out = vec![0u64; p.count];
        for d in &self.active {
            let c = lonlat_to_dir(d.lon.to_radians(), d.lat.to_radians());
            for id in 0..p.count {
                let x = p.center[id];
                let km = (c[0] * x[0] + c[1] * x[1] + c[2] * x[2]).clamp(-1.0, 1.0).acos() * radius;
                let s = (d.shortfall as f64 * reach(km / d.radius_km)).round() as u64;
                if cx.traced(id as u32) && s > 0 {
                    let (lon, lat) = dir_to_lonlat(c);
                    cx.note(format!(
                        "a drought centred {:.0} km away (at {:.1}°, {:.1}°, {:.0} km across) takes {:.0}% of the rain, until {}",
                        km,
                        lat.to_degrees(),
                        lon.to_degrees(),
                        2.0 * d.radius_km,
                        s as f64 / 10.0,
                        cx.calendar.format(d.until)
                    ));
                }
                out[id] = out[id].max(s);
            }
        }
        w.rain_shortfall = out;
    }
}

/// How much of a drought's strength reaches a point `x` radii from its centre: all of it in the
/// inner half, then a smooth fade to nothing at the edge.
fn reach(x: f64) -> f64 {
    let t = ((1.0 - x) / 0.5).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Module for Droughts {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        self.active.clear();
        w.rain_shortfall = vec![0; w.provinces.count];
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let before = self.active.len();
        self.active.retain(|d| d.until > cx.day);
        // Explain the province's rain on every month start it's traced, not only when it changes.
        if self.active.len() != before || cx.trace.is_some() {
            self.paint(w, cx);
        }
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let Event::Drought { lon, lat, radius_km, shortfall, days } = *e else { return };
        self.active.push(Drought { lon, lat, radius_km, shortfall: shortfall.min(1000), until: cx.day + days });
        self.paint(w, cx);
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.active)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.active = postcard::from_bytes(bytes).map_err(|e| format!("Weather state: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drought_is_full_inside_and_fades_to_nothing_at_its_edge() {
        assert_eq!(reach(0.0), 1.0);
        assert_eq!(reach(0.5), 1.0);
        assert!((reach(0.75) - 0.5).abs() < 1e-12);
        assert_eq!(reach(1.0), 0.0);
        assert_eq!(reach(3.0), 0.0);
    }
}
