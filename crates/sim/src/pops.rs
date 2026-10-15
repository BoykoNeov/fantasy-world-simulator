//! Population v0: people grouped into pops per province, who grow when fed and starve when not.
//!
//! The module owns the pop fields and `province.starved_this_year`. On the first day of each month
//! every pop asks the owner of the province's goods for a month of food (one ration per person per
//! day), naming food by the goods tree's class, not by a store.
//! The grant arrives the next day: fewer children are born when food fell short, and some of the
//! unfed die. People are whole numbers, so every change balances exactly.

use crate::events::{Event, EventKind};
use crate::fields::{known, Quantity};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{Cause, Priority};
use crate::rng::{purpose, round_stochastic, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};

/// Species is a pop attribute from day one, though only humans exist yet.
pub const HUMAN: u16 = 0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopParams {
    pub births_per_year: f64,
    pub deaths_per_year: f64,
    /// Share of the people left unfed who die, per 30 days of hunger.
    pub starvation_deaths: f64,
    /// Starting population as a share of what an average harvest can feed.
    pub initial_fill: f64,
}

impl Default for PopParams {
    fn default() -> Self {
        PopParams { births_per_year: 0.036, deaths_per_year: 0.028, starvation_deaths: 0.3, initial_fill: 0.7 }
    }
}

/// One entry per pop, as parallel arrays. A pop is keyed by province, species, culture, religion
/// and class; all but province are a single value for now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pops {
    pub province: Vec<u32>,
    pub species: Vec<u16>,
    pub culture: Vec<u16>,
    pub religion: Vec<u16>,
    pub class: Vec<u8>,
    pub size: Vec<u64>,
}

impl Pops {
    pub fn push(&mut self, province: u32, size: u64) {
        self.province.push(province);
        self.species.push(HUMAN);
        self.culture.push(0);
        self.religion.push(0);
        self.class.push(0);
        self.size.push(size);
    }
}

pub struct Population {
    pub params: PopParams,
}

static V0: Manifest = Manifest {
    slot: Slot::Population,
    version: "basic",
    tier: crate::fields::Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["province.food_capacity"],
    owns: &["pop.province", "pop.species", "pop.culture", "pop.religion", "pop.class", "pop.size", "province.starved_this_year"],
    requests: &["province.stock"],
    asks: &["food"],
    requires: &[],
    emits: &[EventKind::Famine],
    handles: &[EventKind::Granted],
    state_schema: "none",
    goods: &[],
};

impl Module for Population {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }

    /// One human pop in every province that can grow food.
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let p = &w.provinces;
        w.pops = Pops::default();
        for id in 0..p.count {
            let daily = p.food_capacity[id] as f64 / cx.calendar.days_per_year() as f64;
            let size = (daily * self.params.initial_fill).round() as u64;
            if size > 0 {
                w.pops.push(id as u32, size);
            }
        }
        w.starved_this_year = vec![0; p.count];
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        if cx.calendar.is_year_start(cx.day) {
            w.starved_this_year.iter_mut().for_each(|s| *s = 0);
        }
        let days = cx.calendar.def.months[cx.calendar.date(cx.day).month].days as u64;
        let stock = known("province.stock");
        // Food is asked for by its place in the goods tree: the economy serves it from whatever it
        // keeps under food, whether food as a whole or grain, fish and livestock.
        let food = cx.goods.tree.find("food").expect("checked at configuration: the goods tree has food");
        for k in 0..w.pops.size.len() {
            let size = w.pops.size[k];
            if size > 0 {
                cx.ask_good(stock, w.pops.province[k], food, size * days, Priority::Essential, Cause::Eating, k as u64);
            }
        }
    }

    /// A month's eating was granted (in full or not): settle the month's births and deaths.
    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let Event::Granted { request: r, granted } = e else { return };
        let (p, cal, seed) = (&self.params, cx.calendar, cx.seed);
        let k = r.tag as usize;
        // Rates apply to the pop as it is today; `fed` is the share of what it asked for that came.
        let Some(&size) = w.pops.size.get(k) else { return };
        let days = cal.def.months[cal.date(r.day).month].days as u64;
        let this_month = |rate_per_year: f64| rate_per_year * days as f64 / cal.days_per_year() as f64;
        let fed = if r.take == 0 { 1.0 } else { *granted as f64 / r.take as f64 };
        let draw = |x: f64, what: &str| round_stochastic(x, seed, Rng::Pops, r.day, k as u64, purpose(what));

        let births = draw(size as f64 * this_month(p.births_per_year) * fed, "births");
        let natural = draw(size as f64 * this_month(p.deaths_per_year), "deaths").min(size);
        let starved = draw(size as f64 * (1.0 - fed) * p.starvation_deaths * days as f64 / 30.0, "starved").min(size - natural);
        w.pops.size[k] = size + births - natural - starved;
        let province = w.pops.province[k];
        w.starved_this_year[province as usize] += starved;
        if cx.traced(province) {
            cx.note(format!(
                "pop {k} ({size} people) got {:.0}% of the month's food it asked for: {births} born, {natural} died of old age and illness, {starved} starved",
                100.0 * fed
            ));
        }
        cx.flow(Quantity::People, Cause::Births, births as i64);
        cx.flow(Quantity::People, Cause::Deaths, -(natural as i64));
        cx.flow(Quantity::People, Cause::Starvation, -(starved as i64));
        if starved > 0 {
            cx.emit(Event::Famine { province, starved });
        }
    }
}
