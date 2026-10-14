//! Pops v0: people grouped per province, who grow when fed and starve when not.
//!
//! People and food are whole numbers, so every month balances exactly: the ledger accounts for
//! every person born or dead and every ration grown, eaten or spoiled. One ration feeds one person
//! for one day; rates per year are scaled by each month's length, so short months count for less.
//!
//! Once a month each province harvests its share of the year's food (seasonal away from the
//! equator, better or worse by region each year), some stored food spoils, and people eat. If there
//! isn't enough, fewer children are born and some of the unfed die.

use crate::calendar::Calendar;
use crate::rng::{draw, purpose, round_stochastic, Module};
use crate::terrain::fbm;
use crate::world::World;
use std::f64::consts::TAU;

/// Species is a pop attribute from day one, though only humans exist yet.
pub const HUMAN: u16 = 0;

#[derive(Clone, Debug)]
pub struct PopParams {
    pub births_per_year: f64,
    pub deaths_per_year: f64,
    /// Share of the people left unfed who die, per 30 days of hunger.
    pub starvation_deaths: f64,
    /// Share of stored food lost over a year (applied month by month, pro rata).
    pub spoilage_per_year: f64,
    /// Starting population as a share of what an average harvest can feed.
    pub initial_fill: f64,
    /// Starting food stock, in days of eating.
    pub initial_stock_days: f64,
    /// How much a region's harvest swings from year to year (0 = never).
    pub harvest_variability: f64,
    /// How strongly the harvest gathers in late summer at high latitudes (0 = evenly every month).
    pub seasonality: f64,
    /// Fraction of the year at which the northern harvest peaks; the south peaks half a year later.
    pub harvest_peak: f64,
}

impl Default for PopParams {
    fn default() -> Self {
        PopParams {
            births_per_year: 0.036,
            deaths_per_year: 0.028,
            starvation_deaths: 0.3,
            spoilage_per_year: 0.35,
            initial_fill: 0.7,
            initial_stock_days: 120.0,
            harvest_variability: 0.6,
            seasonality: 0.9,
            harvest_peak: 0.62,
        }
    }
}

/// One entry per pop, as parallel arrays. A pop is keyed by province, species, culture, religion
/// and class; all but province are a single value for now.
#[derive(Clone, Debug, Default)]
pub struct Pops {
    pub province: Vec<u32>,
    pub species: Vec<u16>,
    pub culture: Vec<u16>,
    pub religion: Vec<u16>,
    pub class: Vec<u8>,
    pub size: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct PopState {
    pub pops: Pops,
    /// Stored food per province, rations.
    pub food_stock: Vec<u64>,
    /// People who starved in each province since the last year start, for the map and summaries.
    pub starved_this_year: Vec<u64>,
}

/// One month's totals across the planet. Each line balances exactly:
/// stock after = stock before + produced − spoiled − eaten; people after = before + births − deaths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    pub produced: u64,
    pub spoiled: u64,
    pub eaten: u64,
    pub births: u64,
    /// All deaths, starvation included.
    pub deaths: u64,
    pub starved: u64,
}

impl Ledger {
    pub fn add(&mut self, o: &Ledger) {
        self.produced += o.produced;
        self.spoiled += o.spoiled;
        self.eaten += o.eaten;
        self.births += o.births;
        self.deaths += o.deaths;
        self.starved += o.starved;
    }
}

impl PopState {
    /// One human pop in every province that can grow food.
    pub fn initial(world: &World, cal: &Calendar, params: &PopParams) -> Self {
        let p = &world.provinces;
        let mut pops = Pops::default();
        let mut food_stock = vec![0u64; p.count];
        for id in 0..p.count {
            let daily = p.food_capacity[id] as f64 / cal.days_per_year() as f64;
            let size = (daily * params.initial_fill).round() as u64;
            if size == 0 {
                continue;
            }
            pops.province.push(id as u32);
            pops.species.push(HUMAN);
            pops.culture.push(0);
            pops.religion.push(0);
            pops.class.push(0);
            pops.size.push(size);
            food_stock[id] = (size as f64 * params.initial_stock_days).round() as u64;
        }
        PopState { pops, food_stock, starved_this_year: vec![0; p.count] }
    }

    pub fn total_population(&self) -> u64 {
        self.pops.size.iter().sum()
    }

    pub fn province_population(&self, provinces: usize) -> Vec<u64> {
        let mut out = vec![0u64; provinces];
        for (k, &p) in self.pops.province.iter().enumerate() {
            out[p as usize] += self.pops.size[k];
        }
        out
    }

    /// The monthly pass, run on the first day of each month.
    pub fn month(&mut self, world: &World, cal: &Calendar, params: &PopParams, day: u64) -> Ledger {
        let seed = world.params.seed;
        let prov = &world.provinces;
        let date = cal.date(day);
        let year_day = cal.year_start(date.year);
        let harvest_key = draw(seed, Module::Pops, year_day, 0, purpose("harvest year"));
        let days = cal.def.months[date.month].days as u64;
        let this_month = |rate_per_year: f64| rate_per_year * days as f64 / cal.days_per_year() as f64;

        let need: Vec<u64> = self.province_population(prov.count).iter().map(|&p| p * days).collect();
        let mut fed = vec![1.0f64; prov.count];
        let mut ledger = Ledger::default();

        for id in 0..prov.count {
            let e = id as u64;
            let c = prov.center[id];
            let swing = 2.0 * params.harvest_variability * fbm([c[0] * 2.5, c[1] * 2.5, c[2] * 2.5], 3, harvest_key);
            let year_factor = (1.0 + swing).max(0.0);
            let share = month_share(cal, params, date.month, prov.lat[id] as f64);
            let expected = prov.food_capacity[id] as f64 * year_factor * share;
            let produced = round_stochastic(expected, seed, Module::Pops, day, e, purpose("harvest"));

            let mut stock = self.food_stock[id] + produced;
            let spoiled = round_stochastic(stock as f64 * this_month(params.spoilage_per_year), seed, Module::Pops, day, e, purpose("spoil")).min(stock);
            stock -= spoiled;
            let eaten = stock.min(need[id]);
            stock -= eaten;
            if need[id] > 0 {
                fed[id] = eaten as f64 / need[id] as f64;
            }
            self.food_stock[id] = stock;
            ledger.produced += produced;
            ledger.spoiled += spoiled;
            ledger.eaten += eaten;
        }

        for k in 0..self.pops.size.len() {
            let size = self.pops.size[k];
            let id = self.pops.province[k] as usize;
            let f = fed[id];
            let e = k as u64;
            let births = round_stochastic(size as f64 * this_month(params.births_per_year) * f, seed, Module::Pops, day, e, purpose("births"));
            let natural = round_stochastic(size as f64 * this_month(params.deaths_per_year), seed, Module::Pops, day, e, purpose("deaths"));
            let starved =
                round_stochastic(size as f64 * (1.0 - f) * params.starvation_deaths * days as f64 / 30.0, seed, Module::Pops, day, e, purpose("starved"));
            let starved = starved.min(size);
            let deaths = (natural + starved).min(size);
            self.pops.size[k] = size + births - deaths;
            self.starved_this_year[id] += starved;
            ledger.births += births;
            ledger.deaths += deaths;
            ledger.starved += starved;
        }
        ledger
    }
}

/// Share of the year's harvest that arrives in `month`, at latitude `lat` (radians). Shares over a
/// year sum to 1; the swing grows with distance from the equator.
pub fn month_share(cal: &Calendar, params: &PopParams, month: usize, lat: f64) -> f64 {
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
    use crate::world::WorldParams;

    fn small_world() -> World {
        World::generate(&WorldParams { n: 24, provinces: 40, seed: 11, land_fraction: 0.4, radius_km: 6371.0 })
    }

    #[test]
    fn harvest_shares_sum_to_one_and_follow_hemispheres() {
        let cal = Calendar::standard();
        let p = PopParams::default();
        for lat in [-1.2, -0.3, 0.0, 0.5, 1.4] {
            let total: f64 = (0..cal.months_per_year()).map(|m| month_share(&cal, &p, m, lat)).sum();
            assert!((total - 1.0).abs() < 1e-12, "{total}");
        }
        // Northern harvest month (index 7) beats northern midwinter (index 0); the south is reversed.
        assert!(month_share(&cal, &p, 7, 0.9) > 3.0 * month_share(&cal, &p, 0, 0.9));
        assert!(month_share(&cal, &p, 0, -0.9) > 3.0 * month_share(&cal, &p, 7, -0.9));
    }

    #[test]
    fn every_month_balances_exactly() {
        let world = small_world();
        let cal = Calendar::standard();
        let params = PopParams { harvest_variability: 1.2, ..PopParams::default() };
        let mut state = PopState::initial(&world, &cal, &params);
        let mut starved = 0;
        let mut day = 0;
        for _ in 0..30 * cal.months_per_year() {
            let (pop0, stock0): (u64, u64) = (state.total_population(), state.food_stock.iter().sum());
            let l = state.month(&world, &cal, &params, day);
            let (pop1, stock1): (u64, u64) = (state.total_population(), state.food_stock.iter().sum());
            assert_eq!(stock1 + l.spoiled + l.eaten, stock0 + l.produced, "food does not balance on day {day}");
            assert_eq!(pop1 + l.deaths, pop0 + l.births, "people do not balance on day {day}");
            starved += l.starved;
            day += cal.def.months[cal.date(day).month].days as u64;
            assert!(cal.is_month_start(day));
        }
        assert!(state.total_population() > 0);
        assert!(starved > 0, "30 years of harsh harvests and nobody starved");
    }
}
