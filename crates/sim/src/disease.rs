//! Disease "plagues" (build step 10): plagues break out, spread along the ways between provinces
//! (faster by sea and between big towns), kill a share of the sick and leave survivors immune for
//! years.
//!
//! **Within a province** each week some of the people who are neither ill nor immune fall ill:
//! more the more are ill already ([`DiseaseParams::catching`]: how many one sick person gives it
//! to, over their illness, among people none of whom is immune), the more crowded the province
//! against the world's middle one ([`DiseaseParams::crowding`]), where people starved last month
//! ([`DiseaseParams::hunger`]), and the younger the outbreak (it weakens by half every
//! [`DiseaseParams::fade_years`]). A share of the sick come to the end of their illness each week
//! (a week's days over [`DiseaseParams::days_ill`]): [`DiseaseParams::deadliness`] of them die, the
//! rest are immune. The same share sets how many each sick person gives it to in a week, so over an
//! illness one sick person gives it to `catching` others, whatever the length of the illness: the
//! share of a province that is ever ill comes out at the textbook final size for that number.
//! Immunity halves every [`DiseaseParams::immunity_years`] and is thinned by births (newborns are
//! neither ill nor immune). A province with fewer sick people than [`DiseaseParams::ends_below`] is
//! free of it (its last sick recover).
//!
//! **Between provinces** a plague can jump along each way out of a province with sick people, each
//! week with a chance that grows with the share ill there (per 1% ill,
//! [`DiseaseParams::land_reach`]), falls by half for every [`DiseaseParams::half_days`] of travel,
//! is [`DiseaseParams::sea_factor`] times as high along a sea way, and is higher between big towns
//! ([`DiseaseParams::towns`]): the people at both ends against the middle province's, standing in
//! for trade, the same in both economies. The chances from all the sick provinces next to one
//! province are added up into one draw a week; the province that gave the most is where it came
//! from. Where it arrives, [`DiseaseParams::arrival_share`] of the people (at least
//! [`DiseaseParams::first_cases`]) fall ill, if that many are not immune.
//!
//! **Plagues begin** by the player's poke ([`Event::Plague`]), or, with the part
//! [`StartParams`] on, by chance: each month a chance ([`StartParams::per_year`] a year) that a
//! plague begins somewhere, in a province of at least [`StartParams::smallest`] people, the more
//! likely the more people live there ([`StartParams::city_power`]) and [`StartParams::port_weight`]
//! times as likely in a province with a sea way.
//!
//! Deaths are a request to the pops' owner ([`Cause::Plague`], served before everything else, so
//! before recruits); the dead are counted in `province.plague_dead` (since the year began) and in
//! the outbreak's tally when the grant comes, the next day. Soldiers in armies don't catch it, and
//! it is kept apart from the camp disease of armies on campaign. Each outbreak is kept in the
//! module's saved state: where and when it began, when it ended, its dead and the provinces it
//! reached. Events: [`Event::Outbreak`] when one begins, [`Event::PlagueArrived`] when it reaches a
//! province, [`Event::OutbreakOver`] the week after its last province is free of it (its last dead
//! counted by then).
//!
//! The module runs weekly but skips weeks with nobody ill and nothing else to do: with nobody ill,
//! it runs on the first week of a month only to check for a new plague (if they begin by chance),
//! or to bring immunity up to date where people are immune, and on the first week of a year to
//! start counting the year's plague dead afresh. Shares are millionths, rounded by chance so that
//! nothing is lost on average.

use crate::calendar::Calendar;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, Tier};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{Cause, Priority};
use crate::rng::{purpose, round_stochastic, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};

/// Shares are kept in millionths.
const MILLION: f64 = 1_000_000.0;
/// The most of a province's people that can be immune: always a few who aren't.
pub const IMMUNE_MOST: u64 = 990_000;
/// A province's crowding makes it catch at least a third and at most three times as readily as the
/// world's middle province.
const CROWD_LEAST: f64 = 1.0 / 3.0;
const CROWD_MOST: f64 = 3.0;
/// The share of a province's people starved in a month at which hunger counts in full.
const HUNGER_FULL: f64 = 0.01;
/// The spread's chance is given per this share of a province ill.
const SICK_UNIT: f64 = 0.01;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiseaseParams {
    /// How many people one sick person gives it to over their illness, among people none of whom
    /// is immune (before crowding, hunger and weakening).
    pub catching: f64,
    /// Days a person is ill.
    pub days_ill: f64,
    /// Share of the sick who die.
    pub deadliness: f64,
    /// Power of a province's people per km² against the world's middle province's, for how readily
    /// it catches (0: crowding doesn't matter).
    pub crowding: f64,
    /// How much more readily it catches where 1% or more of the people starved last month (in
    /// proportion for fewer): 0.5 makes it 1.5 times.
    pub hunger: f64,
    /// Immunity halves every this many years.
    pub immunity_years: f64,
    /// An outbreak catches half as readily every this many years of its age.
    pub fade_years: f64,
    /// Chance a week that it jumps along a way of no length from a province with 1% of its people
    /// ill (and more for more), between two middle-sized provinces.
    pub land_reach: f64,
    /// The chance along a way halves for every this many days of travel.
    pub half_days: f64,
    /// How many times as readily it goes along a sea way as along a road of the same days.
    pub sea_factor: f64,
    /// Power of the people at both ends of a way against the middle province's, for how readily it
    /// goes along it (0: town sizes don't matter).
    pub towns: f64,
    /// Share of a province's people who fall ill when it arrives, or when a plague begins there.
    pub arrival_share: f64,
    /// The fewest people who fall ill when it arrives (if that many aren't immune).
    pub first_cases: u64,
    /// A province with fewer sick people than this is free of it.
    pub ends_below: u64,
    /// Plagues beginning by chance; none for no such plagues (only the player's). A missing key in
    /// a file means none (not the default), so switching it off survives a settings change.
    #[serde(default)]
    pub starts: Option<StartParams>,
}

impl Default for DiseaseParams {
    fn default() -> Self {
        DiseaseParams {
            catching: 2.0,
            days_ill: 14.0,
            deadliness: 0.3,
            crowding: 0.25,
            hunger: 0.5,
            immunity_years: 15.0,
            fade_years: 3.0,
            land_reach: 0.2,
            half_days: 4.0,
            sea_factor: 70.0,
            towns: 0.5,
            arrival_share: 0.0005,
            first_cases: 10,
            ends_below: 5,
            starts: Some(StartParams::default()),
        }
    }
}

/// Where and how often plagues begin by chance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartParams {
    /// Chance a year that a plague begins somewhere in the world (checked each month).
    pub per_year: f64,
    /// How many times as likely a province with a sea way is to be where it begins.
    pub port_weight: f64,
    /// Power of a province's people, for how likely it is to be where it begins (0: as likely in a
    /// village as in a city).
    pub city_power: f64,
    /// Plagues begin only in provinces of at least this many people.
    pub smallest: u64,
}

impl Default for StartParams {
    fn default() -> Self {
        StartParams { per_year: 0.05, port_weight: 10.0, city_power: 1.0, smallest: 20_000 }
    }
}

/// One outbreak of plague, going on or over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outbreak {
    /// The province where it began.
    pub first: u32,
    /// The day it began.
    pub start: u64,
    /// The day its last province was free of it, `u64::MAX` while it goes on.
    pub end: u64,
    /// People dead of it (granted).
    pub dead: u64,
    /// Provinces it reached, the first one included.
    pub provinces: u32,
    /// Whether [`Event::OutbreakOver`] has been sent.
    pub told: bool,
}

/// What the module keeps between weeks (saved).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct State {
    outbreaks: Vec<Outbreak>,
    /// Each province's people when the shares were last brought up to date, less the plague dead
    /// asked for then: births since thin the shares.
    people: Vec<u64>,
    /// Each province's cells as last seen, to tell after a map edit where land came from.
    cells: Vec<u32>,
    /// The day immunity was last brought up to date.
    updated: u64,
    /// The month (see [`month_key`]) last checked for a plague beginning, `i64::MIN` for none.
    checked_month: i64,
    /// The year `province.plague_dead` counts.
    dead_year: i64,
    /// People starved in each province in the month `famine_month`, from Famine events.
    starved: Vec<u64>,
    famine_month: i64,
}

pub struct Plagues {
    pub params: DiseaseParams,
    st: State,
}

impl Plagues {
    pub fn new(params: DiseaseParams) -> Plagues {
        Plagues { params, st: State { checked_month: i64::MIN, famine_month: i64::MIN, ..State::default() } }
    }

    fn fit(&mut self, n: usize) {
        self.st.people.resize(n, 0);
        self.st.cells.resize(n, 0);
        self.st.starved.resize(n, 0);
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Disease,
    version: "plagues",
    tier: Tier::Province,
    cadence: Cadence::Weekly,
    reads: &["pop.province", "pop.size", "province.cells", "province.area_km2"],
    reads_earlier: &[],
    owns: &["province.sick", "province.immune", "province.plague_dead", "province.outbreak"],
    // The plague's dead are asked of the pops' owner.
    requests: &["pop.size"],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[EventKind::Outbreak, EventKind::PlagueArrived, EventKind::OutbreakOver, EventKind::Refused],
    handles: &[EventKind::Plague, EventKind::Famine],
    state_schema: "outbreaks-1",
    goods: &[],
};

/// Months since year 0, so months of different years differ.
fn month_key(cal: &Calendar, day: u64) -> i64 {
    let d = cal.date(day);
    d.year * cal.months_per_year() as i64 + d.month as i64
}

/// Shares as fractions, from millionths.
fn share(x: u64) -> f64 {
    x as f64 / MILLION
}

/// A share as millionths, rounded by chance, within `most`.
fn millionths(x: f64, most: u64, seed: u64, day: u64, p: usize, what: &str) -> u64 {
    let x = if x.is_finite() { (x * MILLION).max(0.0) } else { 0.0 };
    round_stochastic(x.min(most as f64), seed, Rng::Disease, day, p as u64, purpose(what)).min(most)
}

/// The middle of the positive values (the upper one of two), 1 if there are none.
fn middle(mut v: Vec<f64>) -> f64 {
    v.retain(|x| x.is_finite() && *x > 0.0);
    if v.is_empty() {
        return 1.0;
    }
    let k = v.len() / 2;
    *v.select_nth_unstable_by(k, f64::total_cmp).1
}

impl Module for Plagues {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        let ok = |x: f64, lo: f64, hi: f64| x.is_finite() && x >= lo && x <= hi;
        if !ok(p.catching, 0.0, 1000.0) {
            return Err("how catching it is goes from 0 to 1000 people".into());
        }
        if !ok(p.days_ill, 1.0, 10_000.0) {
            return Err("people are ill for a day or more".into());
        }
        if !ok(p.deadliness, 0.0, 1.0) {
            return Err("the share of the sick who die goes from 0 to 1".into());
        }
        if !ok(p.crowding, 0.0, 10.0) || !ok(p.towns, 0.0, 10.0) {
            return Err("the powers of crowding and of town sizes go from 0 to 10".into());
        }
        if !ok(p.hunger, 0.0, 1000.0) || !ok(p.sea_factor, 0.0, 10_000.0) {
            return Err("hunger's and ships' factors are 0 or more".into());
        }
        if !(ok(p.immunity_years, 0.0, f64::MAX) && p.immunity_years > 0.0 && ok(p.fade_years, 0.0, f64::MAX) && p.fade_years > 0.0) {
            return Err("immunity and outbreaks weaken by half over more than 0 years".into());
        }
        if !ok(p.land_reach, 0.0, 1000.0) {
            return Err("the spread over land is 0 or more".into());
        }
        if !(ok(p.half_days, 0.0, f64::MAX) && p.half_days > 0.0) {
            return Err("a way's reach halves over more than 0 days".into());
        }
        if !(ok(p.arrival_share, 0.0, 1.0) && p.arrival_share > 0.0) {
            return Err("the share falling ill when it arrives is above 0 and at most 1".into());
        }
        if let Some(s) = &p.starts {
            if !ok(s.per_year, 0.0, 1.0) {
                return Err("plagues beginning: the chance a year goes from 0 to 1".into());
            }
            if !ok(s.port_weight, 0.0, 1e9) || !ok(s.city_power, 0.0, 10.0) {
                return Err("plagues beginning: the ports' weight and the cities' power are 0 or more".into());
            }
        }
        Ok(())
    }

    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        (w.sick, w.immune, w.plague_dead, w.outbreak) = (vec![0; n], vec![0; n], vec![0; n], vec![NONE; n]);
        self.st = State {
            outbreaks: Vec::new(),
            people: w.province_population(),
            cells: w.provinces.cells.clone(),
            updated: cx.day,
            checked_month: i64::MIN,
            dead_year: cx.calendar.date(cx.day).year,
            starved: vec![0; n],
            famine_month: i64::MIN,
        };
    }

    fn idle(&self, w: &World, cal: &Calendar, day: u64) -> bool {
        // An outbreak going on, or over and not yet told.
        if self.st.outbreaks.iter().any(|o| !o.told) || w.sick.iter().any(|&s| s > 0) {
            return false;
        }
        if month_key(cal, day) != self.st.checked_month {
            let starts = self.params.starts.as_ref().is_some_and(|s| s.per_year > 0.0);
            if starts || w.immune.iter().any(|&x| x > 0) {
                return false;
            }
        }
        !(cal.date(day).year != self.st.dead_year && w.plague_dead.iter().any(|&x| x > 0))
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        self.fit(n);
        let people = w.province_population();
        self.catch_up(w, cx, &people);
        // Outbreaks that ended last week: their last dead have been counted since.
        for (o, ob) in self.st.outbreaks.iter_mut().enumerate() {
            if ob.end != u64::MAX && !ob.told {
                ob.told = true;
                cx.emit(Event::OutbreakOver { outbreak: o as u32, dead: ob.dead, provinces: ob.provinces });
            }
        }
        let month = month_key(cx.calendar, cx.day);
        if month != self.st.checked_month {
            self.st.checked_month = month;
            if let Some(s) = self.params.starts.clone() {
                self.natural_start(w, cx, &people, &s);
            }
        }
        if w.sick.iter().any(|&s| s > 0) {
            self.week(w, cx, &people);
        }
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let n = w.provinces.count;
        self.fit(n);
        match *e {
            Event::Plague { province } => {
                let people = w.province_population();
                if (province as usize) >= n || people[province as usize] == 0 {
                    cx.emit(Event::Refused { why: format!("no plague can break out in province {province}: nobody lives there") });
                    return;
                }
                self.catch_up(w, cx, &people);
                let p = province as usize;
                match w.outbreak[p] {
                    NONE => {
                        self.begin(w, cx, p, people[p]);
                    }
                    o => {
                        // A plague is there already: the poke adds its first cases to it.
                        let first = self.first_cases(w, p, people[p]);
                        w.sick[p] = w.sick[p].max(millionths(first / people[p] as f64, 1_000_000 - w.immune[p], cx.seed, cx.day, p, "poked"));
                        if cx.traced(province) {
                            cx.note(format!("plague poked in, where outbreak {o} goes on already: at least {first:.0} people ill"));
                        }
                    }
                }
            }
            Event::Famine { province, starved } => {
                let month = month_key(cx.calendar, cx.day);
                if month != self.st.famine_month {
                    self.st.starved.iter_mut().for_each(|s| *s = 0);
                    self.st.famine_month = month;
                }
                if let Some(s) = self.st.starved.get_mut(province as usize) {
                    *s += starved;
                }
            }
            Event::Granted { request: r, granted } if r.cause == Cause::Plague => {
                let year = cx.calendar.date(r.day).year;
                if year > self.st.dead_year {
                    w.plague_dead.iter_mut().for_each(|d| *d = 0);
                    self.st.dead_year = year;
                }
                if year == self.st.dead_year
                    && let Some(d) = w.plague_dead.get_mut(r.entity as usize)
                {
                    *d += granted;
                }
                if let Some(o) = self.st.outbreaks.get_mut(r.tag as usize) {
                    o.dead += granted;
                }
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.st)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.st = postcard::from_bytes(bytes).map_err(|e| format!("Disease state: {e}"))?;
        Ok(())
    }

    /// Put the shares right after an edit moved land or people: a province split off takes the
    /// shares of the province that gave it the most land, one merged into another or given land
    /// mixes them by people, and a province left with nobody has no plague.
    fn edited(&mut self, w: &mut World, edit: &Edit) {
        let n = w.provinces.count;
        let before = std::mem::take(&mut self.st.cells);
        self.fit(n);
        let people = w.province_population();
        let cells = &w.provinces.cells;
        // The province that lost the most cells (the lower number of equals), not `but`.
        let giver = |but: usize| {
            (0..before.len().min(n))
                .filter(|&p| p != but && before[p] > cells[p])
                .max_by(|&a, &b| (before[a] - cells[a]).cmp(&(before[b] - cells[b])).then(b.cmp(&a)))
        };
        match *edit {
            Edit::Split { .. } if n > before.len() => {
                if let Some(g) = giver(n) {
                    for new in before.len()..n {
                        self.mix(w, new, &[(g, 1.0)]);
                    }
                }
            }
            Edit::Merge { from, into } => {
                let (f, i) = (from as usize, into as usize);
                if f < n && i < n && f != i {
                    let weight = |p: usize| self.st.people.get(p).copied().unwrap_or(0) as f64;
                    let (wf, wi) = if weight(f) + weight(i) > 0.0 { (weight(f), weight(i)) } else { (1.0, 1.0) };
                    self.mix(w, i, &[(i, wi), (f, wf)]);
                    w.plague_dead[i] += std::mem::take(&mut w.plague_dead[f]);
                    (w.sick[f], w.immune[f], w.outbreak[f]) = (0, 0, NONE);
                }
            }
            Edit::MoveCells { to, .. } if (to as usize) < n => {
                let t = to as usize;
                if let Some(g) = giver(t) {
                    let had = self.st.people[t];
                    let came = people[t].saturating_sub(had);
                    if came > 0 {
                        self.mix(w, t, &[(t, had as f64), (g, came as f64)]);
                    }
                }
            }
            _ => {}
        }
        for p in 0..n {
            if people[p] == 0 {
                (w.sick[p], w.immune[p], w.outbreak[p]) = (0, 0, NONE);
            }
        }
        self.st.people = people;
        self.st.cells = w.provinces.cells.clone();
    }
}

impl Plagues {
    /// Province `to` takes the shares of `from`, mixed by the weights (`to` may be among them). An
    /// outbreak going on in one of them goes on in `to` (its own first, else the heaviest's).
    fn mix(&mut self, w: &mut World, to: usize, from: &[(usize, f64)]) {
        let all: f64 = from.iter().map(|x| x.1).sum();
        if all <= 0.0 {
            return;
        }
        let avg = |v: &[u64]| (from.iter().map(|&(p, x)| v[p] as f64 * x).sum::<f64>() / all).round() as u64;
        let (sick, immune) = (avg(&w.sick).min(1_000_000), avg(&w.immune).min(IMMUNE_MOST));
        let own = w.outbreak[to];
        let going = |o: u32| o != NONE && self.st.outbreaks.get(o as usize).is_some_and(|ob| ob.end == u64::MAX);
        let mut heaviest: Vec<&(usize, f64)> = from.iter().collect();
        heaviest.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let outbreak = if going(own) { own } else { heaviest.iter().map(|x| w.outbreak[x.0]).find(|&o| going(o)).unwrap_or(NONE) };
        if outbreak != NONE && outbreak != own {
            self.st.outbreaks[outbreak as usize].provinces += 1;
        }
        w.sick[to] = sick;
        w.immune[to] = immune.min(1_000_000 - sick);
        w.outbreak[to] = if sick > 0 { outbreak } else { NONE };
    }

    /// Bring every province's shares up to date: immunity's halving since the last time, the year's
    /// plague dead counted afresh in a new year, and the shares thinned by the people born since.
    /// (A province that lost people keeps its shares: they died or left alike.)
    fn catch_up(&mut self, w: &mut World, cx: &mut Ctx, people: &[u64]) {
        let (cal, day, seed) = (cx.calendar, cx.day, cx.seed);
        let year = cal.date(day).year;
        if year > self.st.dead_year {
            w.plague_dead.iter_mut().for_each(|d| *d = 0);
            self.st.dead_year = year;
        }
        let years = day.saturating_sub(self.st.updated) as f64 / cal.days_per_year() as f64;
        let decay = (-years / self.params.immunity_years).exp2();
        for p in 0..people.len() {
            let (then, now) = (self.st.people[p], people[p]);
            let thin = if then > 0 && now > then { then as f64 / now as f64 } else { 1.0 };
            if w.immune[p] > 0 {
                w.immune[p] = millionths(share(w.immune[p]) * decay * thin, IMMUNE_MOST, seed, day, p, "immunity");
            }
            if w.sick[p] > 0 && thin < 1.0 {
                w.sick[p] = millionths(share(w.sick[p]) * thin, 1_000_000, seed, day, p, "born");
            }
        }
        self.st.people.copy_from_slice(people);
        self.st.updated = day;
    }

    /// The first cases where a plague arrives or begins: the share of its people (at least the
    /// fewest), no more than those not immune.
    fn first_cases(&self, w: &World, p: usize, people: u64) -> f64 {
        let open = people as f64 * (1.0 - share(w.immune[p] + w.sick[p])).max(0.0);
        (people as f64 * self.params.arrival_share).max(self.params.first_cases as f64).min(open)
    }

    /// A new outbreak in province `p`, with its first cases.
    fn begin(&mut self, w: &mut World, cx: &mut Ctx, p: usize, people: u64) -> u32 {
        let o = self.st.outbreaks.len() as u32;
        self.st.outbreaks.push(Outbreak { first: p as u32, start: cx.day, end: u64::MAX, dead: 0, provinces: 1, told: false });
        let first = self.first_cases(w, p, people);
        let sick = if first >= 1.0 { millionths(first / people as f64, 1_000_000 - w.immune[p], cx.seed, cx.day, p, "first cases").max(1) } else { 0 };
        w.sick[p] = sick;
        // An outbreak where nobody could fall ill is over at the next week.
        w.outbreak[p] = if sick > 0 { o } else { NONE };
        if sick == 0 {
            self.st.outbreaks[o as usize].end = cx.day;
        }
        cx.emit(Event::Outbreak { outbreak: o, province: p as u32 });
        if cx.traced(p as u32) {
            cx.note(format!("outbreak {o} of plague begins here: {first:.0} of {people} people fall ill"));
        }
        o
    }

    /// The month's chance that a plague begins somewhere, and where.
    fn natural_start(&mut self, w: &mut World, cx: &mut Ctx, people: &[u64], s: &StartParams) {
        let months = cx.calendar.months_per_year() as f64;
        let chance = 1.0 - (1.0 - s.per_year).powf(1.0 / months);
        if unit(cx.seed, Rng::Disease, cx.day, 0, purpose("begins")) >= chance {
            return;
        }
        let weight: Vec<f64> = (0..people.len())
            .map(|p| {
                if people[p] == 0 || people[p] < s.smallest || w.outbreak[p] != NONE {
                    return 0.0;
                }
                let port = w.routes.of(p as u32).any(|l| l.sea_days.is_some());
                (people[p] as f64).powf(s.city_power) * if port { s.port_weight } else { 1.0 }
            })
            .collect();
        let all: f64 = weight.iter().sum();
        if !(all > 0.0 && all.is_finite()) {
            return;
        }
        let mut pick = unit(cx.seed, Rng::Disease, cx.day, 1, purpose("where")) * all;
        let at = (0..weight.len()).find(|&p| {
            pick -= weight[p];
            weight[p] > 0.0 && pick < 0.0
        });
        let at = at.unwrap_or_else(|| (0..weight.len()).rev().find(|&p| weight[p] > 0.0).expect("some weight"));
        self.begin(w, cx, at, people[at]);
    }

    /// A week of plague: it jumps along the ways, then the sick fall ill, die and recover in each
    /// province it is in, then it arrives where it jumped.
    fn week(&mut self, w: &mut World, cx: &mut Ctx, people: &[u64]) {
        let n = people.len();
        let pr = self.params.clone();
        let (seed, day) = (cx.seed, cx.day);
        let dpy = cx.calendar.days_per_year() as f64;
        let resolve = (cx.calendar.days_per_week() as f64 / pr.days_ill).min(1.0);
        let area = &w.provinces.area_km2;
        let mid_people = middle(people.iter().map(|&x| x as f64).collect());
        let mid_density = middle((0..n).map(|p| if area[p] > 0.0 { people[p] as f64 / area[p] } else { 0.0 }).collect());
        let sick: Vec<usize> = (0..n).filter(|&p| w.sick[p] > 0).collect();
        let arrivals = self.spread(w, cx, people, &sick, mid_people);

        let hungry = month_key(cx.calendar, day).checked_sub(self.st.famine_month).is_some_and(|m| m <= 1);
        let field = known("pop.size");
        let mut died: Vec<(usize, u64)> = Vec::new();
        for &p in &sick {
            let o = w.outbreak[p];
            let total = people[p];
            if total == 0 || o == NONE {
                continue;
            }
            let (s, r) = (share(w.sick[p]), share(w.immune[p]));
            let open = (1.0 - s - r).max(0.0);
            let age = day.saturating_sub(self.st.outbreaks[o as usize].start) as f64 / dpy;
            let strength = (-age / pr.fade_years).exp2();
            let crowd = if pr.crowding > 0.0 && area[p] > 0.0 { ((total as f64 / area[p]) / mid_density).powf(pr.crowding).clamp(CROWD_LEAST, CROWD_MOST) } else { 1.0 };
            let starved = if hungry { self.st.starved[p] as f64 / (total + self.st.starved[p]) as f64 } else { 0.0 };
            let hunger = 1.0 + pr.hunger * (starved / HUNGER_FULL).min(1.0);
            let force = pr.catching * resolve * s * crowd * hunger * strength;
            let new = open * (1.0 - (-force).exp());
            let leaving = s * resolve;
            let dead = round_stochastic((leaving * pr.deadliness * total as f64).max(0.0), seed, Rng::Disease, day, p as u64, purpose("dead")).min(total);
            let after = total - dead;
            let ill = ((s - leaving + new) * total as f64).max(0.0);
            let immune = (r * total as f64 + leaving * total as f64 - dead as f64).max(0.0);
            if dead > 0 {
                cx.request(field, p as u32, dead, Priority::Critical, Cause::Plague, o as u64);
                died.push((p, dead));
            }
            if cx.traced(p as u32) {
                cx.note(format!(
                    "plague (outbreak {o}): {:.0} of {total} people ill, {:.0} immune; {:.0} fall ill this week (catching × {crowd:.2} for crowding, × {hunger:.2} for hunger, × {strength:.2} as the outbreak ages), {:.0} reach the end of their illness: {dead} die",
                    s * total as f64,
                    r * total as f64,
                    new * total as f64,
                    leaving * total as f64
                ));
            }
            if after == 0 {
                (w.sick[p], w.immune[p]) = (0, 0);
                continue;
            }
            let sick_now = millionths(ill / after as f64, 1_000_000, seed, day, p, "ill");
            let immune_now = millionths(immune / after as f64, IMMUNE_MOST.min(1_000_000 - sick_now), seed, day, p, "immune");
            (w.sick[p], w.immune[p]) = (sick_now, immune_now);
            if ill < pr.ends_below as f64 || sick_now == 0 {
                // Free of it: the last few sick recover.
                w.immune[p] = (w.immune[p] + w.sick[p]).min(IMMUNE_MOST);
                w.sick[p] = 0;
                if cx.traced(p as u32) {
                    cx.note(format!("fewer than {} people ill: free of the plague", pr.ends_below));
                }
            }
        }

        for (q, o, from, by_sea) in arrivals {
            let first = self.first_cases(w, q, people[q]);
            if first < 1.0 {
                continue;
            }
            w.sick[q] = millionths(first / people[q] as f64, 1_000_000 - w.immune[q], seed, day, q, "arrives").max(1);
            w.outbreak[q] = o;
            self.st.outbreaks[o as usize].provinces += 1;
            cx.emit(Event::PlagueArrived { outbreak: o, province: q as u32, from: from as u32, by_sea });
            if cx.traced(q as u32) {
                cx.note(format!("plague arrives from province {from} {}: {first:.0} of {} people fall ill", if by_sea { "by sea" } else { "over land" }, people[q]));
            }
        }

        // Provinces free of it, and outbreaks with no province left.
        let mut left = vec![0u32; self.st.outbreaks.len()];
        for p in 0..n {
            let o = w.outbreak[p];
            if o == NONE {
                // (Never so, but were it: sick people of no outbreak recover.)
                if w.sick[p] > 0 {
                    w.immune[p] = (w.immune[p] + std::mem::take(&mut w.sick[p])).min(IMMUNE_MOST);
                }
                continue;
            }
            if w.sick[p] == 0 || people[p] == 0 {
                w.outbreak[p] = NONE;
            } else if let Some(c) = left.get_mut(o as usize) {
                *c += 1;
            }
        }
        for (o, ob) in self.st.outbreaks.iter_mut().enumerate() {
            if ob.end == u64::MAX && left[o] == 0 {
                ob.end = day;
            }
        }
        // The dead asked for leave the people the shares are of.
        for (p, dead) in died {
            self.st.people[p] = people[p] - dead;
        }
    }

    /// Where it jumps this week, from the provinces with sick people: (province, outbreak, from,
    /// by sea), in province order. One draw per province it may reach.
    fn spread(&self, w: &World, cx: &mut Ctx, people: &[u64], sick: &[usize], mid_people: f64) -> Vec<(usize, u32, usize, bool)> {
        let pr = &self.params;
        let n = people.len();
        // (province, chance summed, the most one way gave, from, by sea, outbreak)
        let mut slot: Vec<u32> = vec![NONE; n];
        let mut found: Vec<(usize, f64, f64, usize, bool, u32)> = Vec::new();
        if pr.land_reach <= 0.0 {
            return Vec::new();
        }
        for &p in sick {
            let o = w.outbreak[p];
            if o == NONE || people[p] == 0 {
                continue;
            }
            let push = pr.land_reach * share(w.sick[p]) / SICK_UNIT;
            for l in w.routes.of(p as u32) {
                let q = l.other(p as u32) as usize;
                if q >= n || people[q] == 0 || w.outbreak[q] != NONE {
                    continue;
                }
                let towns = if pr.towns > 0.0 { ((people[p] as f64 / mid_people) * (people[q] as f64 / mid_people)).powf(pr.towns) } else { 1.0 };
                let land = l.land_days.map_or(0.0, |d| (-(d as f64) / pr.half_days).exp2());
                let sea = l.sea_days.map_or(0.0, |d| pr.sea_factor * (-(d as f64) / pr.half_days).exp2());
                // (Without bounds it is certain to arrive.)
                let h = push * towns * (land + sea);
                if h.is_nan() || h <= 0.0 {
                    continue;
                }
                let k = match slot[q] {
                    NONE => {
                        slot[q] = found.len() as u32;
                        found.push((q, 0.0, -1.0, p, false, o));
                        found.len() - 1
                    }
                    k => k as usize,
                };
                let e = &mut found[k];
                e.1 += h;
                if h > e.2 {
                    (e.2, e.3, e.4, e.5) = (h, p, sea > land, o);
                }
            }
        }
        found.sort_by_key(|e| e.0);
        let mut out = Vec::new();
        for (q, h, _, from, by_sea, o) in found {
            let chance = 1.0 - (-h).exp();
            let hit = unit(cx.seed, Rng::Disease, cx.day, q as u64, purpose("arrive")) < chance;
            if cx.traced(q as u32) {
                cx.note(format!("plague next door: a chance of {:.1}% that it arrives this week, most likely from province {from}", 100.0 * chance));
            }
            if hit {
                out.push((q, o, from, by_sea));
            }
        }
        out
    }
}
