//! Economics v0, "food vs population": each province's food store.
//!
//! The module owns `province.stock` and keeps one good in it: food, as a whole (the goods tree's
//! "food" class). Once a month each province harvests its share of the
//! year's food (seasonal away from the equator, better or worse by region each year) and some of
//! the store spoils. Everyone else takes food by change request: at the end of the day the store is
//! shared out, essential needs first, in proportion within a class when it runs short.

use crate::calendar::Calendar;
use crate::events::EventKind;
use crate::fields::{known, Goods, Quantity};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{ration, Cause, Request};
use crate::rng::{draw, purpose, round_stochastic, Module as Rng};
use crate::terrain::fbm;
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoodParams {
    /// Share of stored food lost over a year (applied month by month, pro rata).
    pub spoilage_per_year: f64,
    /// Starting stock, in days of an average harvest.
    pub initial_stock_days: f64,
    /// How much a region's harvest swings from year to year (0 = never).
    pub harvest_variability: f64,
    /// How strongly the harvest gathers in late summer at high latitudes (0 = evenly every month).
    pub seasonality: f64,
    /// Fraction of the year at which the northern harvest peaks; the south peaks half a year later.
    pub harvest_peak: f64,
}

impl Default for FoodParams {
    fn default() -> Self {
        FoodParams { spoilage_per_year: 0.35, initial_stock_days: 84.0, harvest_variability: 0.6, seasonality: 0.9, harvest_peak: 0.62 }
    }
}

pub struct Food {
    pub params: FoodParams,
}

/// Where the food store is in `province.stock`: (columns per province, the food column), and the
/// quantity food is counted in.
fn food_column(goods: &Goods) -> (usize, usize, Quantity) {
    let c = goods.column("food").expect("Economics \"food\" keeps food");
    (goods.stride(), c, Quantity::Goods(goods.class_of_column(c)))
}

static V0: Manifest = Manifest {
    slot: Slot::Economics,
    version: "food",
    tier: crate::fields::Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["province.food_capacity", "province.center", "province.lat", "province.rain_shortfall"],
    owns: &["province.stock"],
    requests: &[],
    requires: &[],
    emits: &[EventKind::Granted],
    handles: &[],
    state_schema: "none",
    goods: &["food"],
};

impl Module for Food {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }

    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let per_day = |cap: u64| cap as f64 / cx.calendar.days_per_year() as f64;
        let (k, food, _) = food_column(&w.goods);
        w.stock = vec![0; w.provinces.count * k];
        for (id, &c) in w.provinces.food_capacity.iter().enumerate() {
            w.stock[id * k + food] = (per_day(c) * self.params.initial_stock_days).round() as u64;
        }
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let (p, cal, day, seed) = (&self.params, cx.calendar, cx.day, cx.seed);
        let (k, food, counted) = food_column(&w.goods);
        let prov = &w.provinces;
        let date = cal.date(day);
        let harvest_key = draw(seed, Rng::Economics, cal.year_start(date.year), 0, purpose("harvest year"));
        let month_part = cal.def.months[date.month].days as f64 / cal.days_per_year() as f64;
        let (mut produced, mut spoiled) = (0u64, 0u64);
        for id in 0..prov.count {
            let e = id as u64;
            let c = prov.center[id];
            let swing = 2.0 * p.harvest_variability * fbm([c[0] * 2.5, c[1] * 2.5, c[2] * 2.5], 3, harvest_key);
            let year_factor = (1.0 + swing).max(0.0);
            let share = month_share(cal, p, date.month, prov.lat[id] as f64);
            // Crops fail in proportion to the missing rain; exactly 1 in a normal year.
            let rain = 1.0 - w.rain_shortfall[id] as f64 / 1000.0;
            let expected = prov.food_capacity[id] as f64 * year_factor * share * rain;
            let harvest = round_stochastic(expected, seed, Rng::Economics, day, e, purpose("harvest"));
            let at = id * k + food;
            let stock = w.stock[at] + harvest;
            let spoil = round_stochastic(stock as f64 * p.spoilage_per_year * month_part, seed, Rng::Economics, day, e, purpose("spoil")).min(stock);
            w.stock[at] = stock - spoil;
            if cx.traced(id as u32) {
                cx.note(format!(
                    "harvest {harvest} rations = {} a year in an average year × {year_factor:.2} for this year's luck × {:.3} of the year's crop due this month × {rain:.2} for the rain",
                    prov.food_capacity[id], share
                ));
                cx.note(format!("{spoil} of {stock} stored rations spoiled ({:.0}% a year)", 100.0 * p.spoilage_per_year));
            }
            produced += harvest;
            spoiled += spoil;
        }
        cx.flow(counted, Cause::Harvest, produced as i64);
        cx.flow(counted, Cause::Spoilage, -(spoiled as i64));
    }

    fn apply_requests(&mut self, w: &mut World, requests: &[Request], cx: &mut Ctx) {
        let stock = known("province.stock");
        // Requests arrive sorted by field and entity (a province's store of one good): take each
        // store's run in turn.
        for run in requests.chunk_by(|a, b| (a.field, a.entity) == (b.field, b.entity)) {
            debug_assert_eq!(run[0].field, stock);
            let at = run[0].entity as usize;
            let wants: Vec<_> = run.iter().map(|r| (r.priority, r.take, r.tag)).collect();
            let grants = ration(w.stock[at], &wants);
            for (r, g) in run.iter().zip(grants) {
                w.stock[at] -= g;
                cx.grant(r, g);
            }
        }
    }
}

/// Share of the year's harvest that arrives in `month`, at latitude `lat` (radians). Shares over a
/// year sum to 1; the swing grows with distance from the equator.
pub fn month_share(cal: &Calendar, params: &FoodParams, month: usize, lat: f64) -> f64 {
    let strength = params.seasonality * (lat.abs().to_degrees() / 40.0).min(1.0);
    let peak = if lat >= 0.0 { params.harvest_peak } else { (params.harvest_peak + 0.5).fract() };
    let weight = |m: usize| {
        let days = cal.def.months[m].days as f64;
        let x = cal.month_mid_year_fraction(m) - peak;
        days * (1.0 + strength * (TAU * x).cos()).max(0.0)
    };
    let total: f64 = (0..cal.months_per_year()).map(weight).sum();
    weight(month) / total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvest_shares_sum_to_one_and_follow_hemispheres() {
        let cal = Calendar::standard();
        let p = FoodParams::default();
        for lat in [-1.2, -0.3, 0.0, 0.5, 1.4] {
            let total: f64 = (0..cal.months_per_year()).map(|m| month_share(&cal, &p, m, lat)).sum();
            assert!((total - 1.0).abs() < 1e-12, "{total}");
        }
        // Northern harvest month (index 7) beats northern midwinter (index 0); the south is reversed.
        assert!(month_share(&cal, &p, 7, 0.9) > 3.0 * month_share(&cal, &p, 0, 0.9));
        assert!(month_share(&cal, &p, 0, -0.9) > 3.0 * month_share(&cal, &p, 7, -0.9));
    }
}
