//! The combination tester: runs mixes of module versions headless on small worlds and several
//! seeds, and scores each run.
//!
//! Every run is strict (a module writing a field it doesn't own stops it) and gets one drought, so
//! the weather slot has work to do. The scorer looks at world totals, not single values, because a
//! single province's food store or a quiet year's missing rain sits at zero as a matter of course:
//!
//! - **red**: a crash, a value clamped back into its bounds, a total that changed by more than its
//!   causes explain, a value held at a finite upper bound for half a year, or a conserved total
//!   whose owner runs staying at zero for three months (everyone gone);
//! - **yellow**: nothing changing for five years although something owns a conserved total, or a
//!   red finding that the known-quirks list explains;
//! - **refused**: the engine refuses the configuration when the world is made, by design.
//!
//! Verdicts depend only on the mix and the seed, so the table written from them is the same every
//! time. Speed is measured but kept out of the table: it depends on how busy the machine is.

use crate::config::{Config, ModuleConfig, CHOICES};
use crate::events::Event;
use crate::fields::{quantity_total, FieldId, Quantity};
use crate::module::{plan, Manifest};
use crate::scenario::{Scenario, ScenarioCommand, ScenarioTimeline, SCENARIO_FORMAT};
use crate::sim::Sim;
use crate::world::WorldParams;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

/// A value held at its upper bound this long is a runaway.
pub const PINNED_DAYS: u64 = 180;
/// A conserved total at zero this long means everyone (or everything) is gone.
pub const GONE_DAYS: u64 = 90;
/// Totals unchanged for this many years: the world has stopped.
pub const STAGNANT_YEARS: u64 = 5;

/// The table the window reads, written by `combos --write`.
const SHIPPED: &str = include_str!("../data/compatibility.toml");

/// One version per slot that has a choice, in [`CHOICES`] order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mix(pub Vec<&'static str>);

/// A slot's name as the tester writes it.
pub fn slot_name(k: usize) -> String {
    format!("{:?}", CHOICES[k].0).to_lowercase()
}

impl Mix {
    /// The default version of every slot.
    pub fn defaults() -> Mix {
        Mix(CHOICES.iter().map(|c| c.1[0]).collect())
    }

    pub fn of(modules: &ModuleConfig) -> Mix {
        Mix(CHOICES.iter().map(|c| modules.choice(c.0).expect("every slot in CHOICES has a choice")).collect())
    }

    /// The defaults, changed by `slot=version` pairs (see [`choose`]).
    pub fn parse(text: &str) -> Result<Mix, String> {
        let mut m = ModuleConfig::default();
        choose(&mut m, text)?;
        Ok(Mix::of(&m))
    }

    /// `terrain=noise climate=latitude ...`
    pub fn label(&self) -> String {
        self.0.iter().enumerate().map(|(k, v)| format!("{}={v}", slot_name(k))).collect::<Vec<_>>().join(" ")
    }

    pub fn modules(&self) -> ModuleConfig {
        let mut m = ModuleConfig::default();
        for (k, v) in self.0.iter().enumerate() {
            m.set_choice(CHOICES[k].0, v).expect("versions come from CHOICES");
        }
        m
    }

    /// Whether the engine accepts this mix; `Err` with its reason if not.
    pub fn check(&self) -> Result<(), String> {
        let modules = self.modules().build();
        let manifests: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
        plan(&manifests).map(|_| ()).map_err(|e| e.0)
    }

    fn pairs(&self) -> impl Iterator<Item = Pair> + '_ {
        let n = self.0.len();
        (0..n).flat_map(move |i| ((i + 1)..n).map(move |j| Pair { a: (i, self.0[i]), b: (j, self.0[j]) }))
    }
}

/// Choose versions by `slot=version` pairs separated by commas or spaces, e.g.
/// `economics=off,population=off`; slots not named keep their version.
pub fn choose(modules: &mut ModuleConfig, text: &str) -> Result<(), String> {
    for part in text.split([',', ' ']).filter(|p| !p.is_empty()) {
        let (slot, version) = part.split_once('=').ok_or_else(|| format!("{part}: expected slot=version"))?;
        let k = (0..CHOICES.len()).find(|&k| slot_name(k) == slot).ok_or_else(|| format!("no slot {slot} with a choice"))?;
        modules.set_choice(CHOICES[k].0, version).map_err(|_| format!("{slot} has no version {version}"))?;
    }
    Ok(())
}

/// Two slots' versions together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pair {
    pub a: (usize, &'static str),
    pub b: (usize, &'static str),
}

impl Pair {
    fn every() -> Vec<Pair> {
        let n = CHOICES.len();
        let mut out = Vec::new();
        for i in 0..n {
            for j in (i + 1)..n {
                for &x in CHOICES[i].1 {
                    for &y in CHOICES[j].1 {
                        out.push(Pair { a: (i, x), b: (j, y) });
                    }
                }
            }
        }
        out
    }

    pub fn label(&self) -> (String, String) {
        (format!("{}={}", slot_name(self.a.0), self.a.1), format!("{}={}", slot_name(self.b.0), self.b.1))
    }
}

/// Every mix there is. Fine while the slots are few; [`all_pairs`] is for when they aren't.
pub fn all_mixes() -> Vec<Mix> {
    let mut out = vec![Vec::new()];
    for c in CHOICES {
        out = out.into_iter().flat_map(|m: Vec<&'static str>| c.1.iter().map(move |v| [m.clone(), vec![*v]].concat())).collect();
    }
    out.into_iter().map(Mix).collect()
}

/// A small set of accepted mixes in which every pair of versions that can run together appears at
/// least once. Greedy: each round builds candidates slot by slot, each slot taking the version that
/// covers the most pairs not yet covered, and keeps the best candidate the engine accepts. A pair no
/// candidate covers is looked for on its own; if no accepted mix with it turns up, it can't run.
/// Deterministic for a given `seed`.
pub fn all_pairs(seed: u64) -> Vec<Mix> {
    let n = CHOICES.len();
    let mut uncovered: BTreeSet<Pair> = Pair::every().into_iter().collect();
    let mut out: Vec<Mix> = Vec::new();
    let mut round = 0u64;
    while !uncovered.is_empty() {
        let mut best: Option<(usize, Mix)> = None;
        for c in 0..20u64 {
            let r = |x: u64| crate::rng::mix(seed ^ crate::rng::mix(round * 1000 + c) ^ crate::rng::mix(x));
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&k| r(k as u64));
            let mut chosen: Vec<Option<&'static str>> = vec![None; n];
            for &k in &order {
                let score = |v: &'static str| {
                    let mut covered = 0;
                    for (j, w) in chosen.iter().enumerate() {
                        if let Some(w) = *w {
                            let p = if j < k { Pair { a: (j, w), b: (k, v) } } else { Pair { a: (k, v), b: (j, w) } };
                            covered += uncovered.contains(&p) as usize;
                        }
                    }
                    covered
                };
                let pick = CHOICES[k].1.iter().copied().max_by_key(|&v| (score(v), r(k as u64 ^ crate::rng::purpose(v)))).expect("a slot has versions");
                chosen[k] = Some(pick);
            }
            let m = Mix(chosen.into_iter().map(|v| v.expect("every slot chosen")).collect());
            let gain = m.pairs().filter(|p| uncovered.contains(p)).count();
            if gain > best.as_ref().map_or(0, |b| b.0) && m.check().is_ok() {
                best = Some((gain, m));
            }
        }
        match best {
            Some((_, m)) => {
                for p in m.pairs() {
                    uncovered.remove(&p);
                }
                out.push(m);
            }
            None => {
                // Look for the first remaining pair on its own.
                let p = *uncovered.iter().next().expect("not empty");
                let found = (0..200u64).find_map(|t| {
                    let mut v: Vec<&'static str> = (0..n).map(|k| CHOICES[k].1[(crate::rng::mix(seed ^ round ^ (t * 977) ^ k as u64) % CHOICES[k].1.len() as u64) as usize]).collect();
                    v[p.a.0] = p.a.1;
                    v[p.b.0] = p.b.1;
                    let m = Mix(v);
                    m.check().is_ok().then_some(m)
                });
                match found {
                    Some(m) => {
                        for q in m.pairs() {
                            uncovered.remove(&q);
                        }
                        out.push(m);
                    }
                    None => {
                        uncovered.remove(&p);
                    }
                }
            }
        }
        round += 1;
    }
    out.sort();
    out.dedup();
    out
}

/// How the tester runs each mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Cube face size of the test worlds.
    pub n: usize,
    pub provinces: usize,
    pub years: i64,
    pub seeds: Vec<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { n: 48, provinces: 200, years: 20, seeds: vec![1, 2, 3] }
    }
}

impl Settings {
    pub fn config(&self, mix: &Mix, seed: u64) -> Config {
        let world = WorldParams { n: self.n, provinces: self.provinces, seed, land_fraction: 0.36, radius_km: 6371.0 };
        Config { modules: mix.modules(), ..Config::new(world) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Green,
    Yellow,
    Red,
    /// The engine refuses the configuration (e.g. people with no food to ask for).
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The run stopped: a panic, including a strict-mode ownership breach.
    Crash,
    ContractViolated,
    ConservationBroken,
    /// A value held at its finite upper bound.
    StuckAtTop,
    /// A conserved total at zero although its owner runs.
    AllGone,
    /// Nothing changed for years.
    Stagnant,
    /// A scenario's expectation didn't hold.
    Expectation,
}

impl Problem {
    pub fn severity(self) -> Verdict {
        match self {
            Problem::Stagnant => Verdict::Yellow,
            _ => Verdict::Red,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub problem: Problem,
    /// The day it started (for a crash, the day that was running).
    pub day: u64,
    pub what: String,
    /// The known quirk that explains it, which makes it yellow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known: Option<String>,
}

impl Finding {
    pub fn verdict(&self) -> Verdict {
        if self.known.is_some() { Verdict::Yellow.min(self.problem.severity()) } else { self.problem.severity() }
    }
}

/// The worst of some findings' verdicts (green if there are none).
pub fn worst(findings: &[Finding]) -> Verdict {
    findings.iter().map(Finding::verdict).max().unwrap_or(Verdict::Green)
}

/// One mix on one seed.
#[derive(Clone, Debug)]
pub struct RunResult {
    pub mix: Mix,
    pub seed: u64,
    /// Why the engine refused the mix, if it did.
    pub refused: Option<String>,
    pub findings: Vec<Finding>,
    pub days: u64,
    pub ms_per_day: f64,
    /// The drought the run was given, and its day.
    pub poke: Option<(u64, Event)>,
}

impl RunResult {
    pub fn verdict(&self) -> Verdict {
        if self.refused.is_some() { Verdict::Refused } else { worst(&self.findings) }
    }

    /// A scenario that makes this run again up to just after its first finding, for replaying in
    /// `simrun --scenario` or the window's `--open`.
    pub fn repro(&self, settings: &Settings) -> Option<Scenario> {
        let first = self.findings.iter().map(|f| f.day).min()?;
        let config = settings.config(&self.mix, self.seed);
        let command = self.poke.iter().filter(|p| p.0 <= first).map(|(day, e)| ScenarioCommand { day: *day, edit: None, poke: Some(e.clone()) }).collect();
        let timeline = vec![ScenarioTimeline { id: 0, parent: None, fork_day: None, day: first + 1, command, expect: Vec::new() }];
        Some(Scenario { format: SCENARIO_FORMAT, active: 0, config, timeline })
    }
}

thread_local! {
    /// Where and why the last panic in a run on this thread happened, when [`quiet_panics`] is on.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Whether this thread is inside a run, whose panics are reported as crashes.
    static IN_RUN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keep panics inside runs off the console; [`run`] reports them as crashes instead. Panics
/// anywhere else are printed as usual.
pub fn quiet_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let usual = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if IN_RUN.with(|r| r.get()) {
                LAST_PANIC.with(|p| *p.borrow_mut() = Some(info.to_string()));
            } else {
                usual(info);
            }
        }));
    });
}

/// `f`, with a panic caught and its hook kept quiet.
fn guarded<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    IN_RUN.with(|r| r.set(true));
    let out = catch_unwind(AssertUnwindSafe(f));
    IN_RUN.with(|r| r.set(false));
    out
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = LAST_PANIC.with(|p| p.borrow_mut().take()) {
        return s.replace('\n', " ");
    }
    payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "a panic".into())
}

/// The drought every run gets: over the province that grows the most food, from the start of the
/// second year, for two years. It takes 70% of the rain, not all of it, so its missing rain isn't held at the
/// field's bound.
pub fn test_drought(sim: &Sim) -> Option<(u64, Event)> {
    let p = &sim.world.provinces;
    let heart = (0..p.count).max_by_key(|&i| (p.food_capacity[i], std::cmp::Reverse(i)))?;
    let (lon, lat) = crate::grid::dir_to_lonlat(p.center[heart]);
    let days = 2 * sim.calendar.days_per_year() as u64;
    let event = Event::Drought { lon: lon.to_degrees(), lat: lat.to_degrees(), radius_km: 2000.0, shortfall: 700, days };
    Some((sim.calendar.year_start(sim.calendar.def.first_year + 1), event))
}

/// What the scorer keeps an eye on while a run goes.
struct Watch {
    /// Conserved quantities whose field has a running owner: total yesterday, and since when it has
    /// been zero.
    totals: Vec<(Quantity, u64, Option<u64>)>,
    /// Running fields with a finite upper bound: since when some value has been at it.
    tops: Vec<(FieldId, Option<u64>)>,
    problems: crate::sim::Problems,
    /// The day a total last changed, and the longest stretch without a change.
    changed: u64,
    still: (u64, u64),
    findings: Vec<Finding>,
    /// Index into `findings` of the first clamp and the first conservation break, and how many.
    clamps: Option<usize>,
    breaks: Option<usize>,
}

impl Watch {
    fn new(sim: &Sim) -> Watch {
        let owned = |id: FieldId| sim.plan.owner[id.0 as usize].is_some();
        let running: Vec<FieldId> = sim.running_fields().filter(|&id| owned(id)).collect();
        // Only goods the world keeps count (a class with no store is always empty).
        let totals = sim
            .world
            .quantities()
            .into_iter()
            .filter(|q| running.contains(&q.field()))
            .map(|q| (q, quantity_total(&sim.world, q), None))
            .collect();
        let tops = running.iter().filter(|id| id.def().max.is_finite()).map(|&id| (id, None)).collect();
        Watch { totals, tops, problems: sim.problems, changed: sim.day, still: (0, sim.day), findings: Vec::new(), clamps: None, breaks: None }
    }

    /// Look at the world after a day ran; `day` is the day that ran.
    fn day(&mut self, sim: &Sim, day: u64) {
        let p = sim.problems;
        if p.contract_violations > self.problems.contract_violations && self.clamps.is_none() {
            let what = sim.pending.iter().find_map(|e| match &e.event {
                Event::ContractViolated { field, entity, value, clamped_to, by } => {
                    Some(format!("{} of {entity} was {value}, clamped to {clamped_to} (by {by:?})", field.def().name))
                }
                _ => None,
            });
            self.clamps = Some(self.findings.len());
            self.findings.push(Finding { problem: Problem::ContractViolated, day, what: what.unwrap_or_default(), known: None });
        }
        if p.conservation_breaks > self.problems.conservation_breaks && self.breaks.is_none() {
            let what = sim.pending.iter().find_map(|e| match &e.event {
                Event::ConservationBroken { quantity, expected, actual, by } => {
                    Some(format!("{} was {actual}, expected {expected} (by {by:?})", quantity.name(&sim.world.goods)))
                }
                _ => None,
            });
            self.breaks = Some(self.findings.len());
            self.findings.push(Finding { problem: Problem::ConservationBroken, day, what: what.unwrap_or_default(), known: None });
        }
        self.problems = p;

        let mut moved = false;
        for (q, last, zero) in &mut self.totals {
            let now = quantity_total(&sim.world, *q);
            moved |= now != *last;
            *last = now;
            match (now, *zero) {
                (0, None) => *zero = Some(day),
                (0, Some(since)) if day - since == GONE_DAYS => {
                    self.findings.push(Finding { problem: Problem::AllGone, day: since, what: format!("{} at zero for {GONE_DAYS} days", q.name(&sim.world.goods)), known: None });
                }
                (0, Some(_)) => {}
                _ => *zero = None,
            }
        }
        if moved {
            self.changed = day;
        } else if day - self.changed > self.still.0 {
            self.still = (day - self.changed, self.changed);
        }
        for (id, since) in &mut self.tops {
            let max = id.def().max;
            let data = sim.world.field(*id);
            let at_top = (0..data.len()).find(|&e| data.value(e) >= max);
            match (at_top, *since) {
                (Some(_), None) => *since = Some(day),
                (Some(e), Some(s)) if day - s == PINNED_DAYS => {
                    self.findings.push(Finding {
                        problem: Problem::StuckAtTop,
                        day: s,
                        what: format!("{} of {e} held at its bound {max} for {PINNED_DAYS} days", id.def().name),
                        known: None,
                    });
                }
                (Some(_), Some(_)) => {}
                (None, _) => *since = None,
            }
        }
    }

    fn finish(mut self, sim: &Sim) -> Vec<Finding> {
        let p = sim.problems;
        if let Some(i) = self.clamps {
            self.findings[i].what += &format!("; {} values clamped in all", p.contract_violations);
        }
        if let Some(i) = self.breaks {
            self.findings[i].what += &format!("; {} breaks in all", p.conservation_breaks);
        }
        let years = self.still.0 / sim.calendar.days_per_year() as u64;
        if !self.totals.is_empty() && years >= STAGNANT_YEARS {
            let what = format!("no conserved total changed for {years} years");
            self.findings.push(Finding { problem: Problem::Stagnant, day: self.still.1, what, known: None });
        }
        self.findings.sort_by_key(|f| (f.day, f.problem));
        self.findings
    }
}

/// Run one mix on one seed for the settings' years, and score it.
pub fn run(mix: &Mix, seed: u64, settings: &Settings) -> RunResult {
    let mut result = RunResult { mix: mix.clone(), seed, refused: None, findings: Vec::new(), days: 0, ms_per_day: 0.0, poke: None };
    let sim = match Sim::new(&settings.config(mix, seed)) {
        Ok(s) => s,
        Err(e) => {
            result.refused = Some(e.0);
            return result;
        }
    };
    play(&mut result, sim, settings.years);
    result
}

/// Give a world just made the test drought, run it strictly for `years`, and score it.
fn play(result: &mut RunResult, mut sim: Sim, years: i64) {
    sim.strict = true;
    result.poke = test_drought(&sim);
    if let Some((day, e)) = &result.poke {
        sim.schedule(*day, e.clone()).expect("the drought's day is still to come");
    }
    let end = sim.calendar.year_start(sim.calendar.def.first_year + years);
    let mut watch = Watch::new(&sim);
    let start = Instant::now();
    let outcome = guarded(|| {
        while sim.day < end {
            let day = sim.day;
            sim.step_day();
            watch.day(&sim, day);
        }
    });
    result.days = sim.day;
    result.ms_per_day = start.elapsed().as_secs_f64() * 1000.0 / sim.day.max(1) as f64;
    match outcome {
        Ok(()) => result.findings = watch.finish(&sim),
        Err(payload) => {
            let day = sim.day;
            result.findings = watch.findings;
            result.findings.push(Finding { problem: Problem::Crash, day, what: panic_text(payload.as_ref()), known: None });
        }
    }
}

/// Known quirks: problems a mix is known to have, written down so they show yellow, not red.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quirks {
    #[serde(default)]
    pub quirk: Vec<Quirk>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quirk {
    /// `slot=version` pairs a mix must have, e.g. `["economics=food", "weather=off"]`.
    pub when: Vec<String>,
    pub problem: Problem,
    /// What is known about it: an issue, an explanation.
    pub note: String,
}

impl Quirks {
    pub fn parse(text: &str) -> Result<Quirks, String> {
        let q: Quirks = toml::from_str(text).map_err(|e| format!("cannot read known quirks: {e}"))?;
        for k in &q.quirk {
            Mix::parse(&k.when.join(","))?;
        }
        Ok(q)
    }

    /// Mark the findings a quirk explains.
    pub fn apply(&self, r: &mut RunResult) {
        let label = r.mix.label();
        let has: BTreeSet<&str> = label.split(' ').collect();
        for f in &mut r.findings {
            if let Some(q) = self.quirk.iter().find(|q| q.problem == f.problem && q.when.iter().all(|w| has.contains(w.as_str()))) {
                f.known = Some(q.note.clone());
            }
        }
    }
}

/// The results, as the table written to `data/compatibility.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub settings: Settings,
    #[serde(default)]
    pub mix: Vec<MixRow>,
    #[serde(default)]
    pub pair: Vec<PairRow>,
    #[serde(default)]
    pub scenario: Vec<ScenarioRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixRow {
    /// `terrain=noise climate=latitude ...`
    pub versions: String,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(default)]
    pub run: Vec<RunRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRow {
    pub seed: u64,
    pub verdict: Verdict,
    #[serde(default)]
    pub finding: Vec<Finding>,
}

/// Two versions together: the worst verdict of the runs that had both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairRow {
    pub a: String,
    pub b: String,
    pub verdict: Verdict,
    /// The mix and seed of a run with that verdict, to reproduce it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioRow {
    /// The scenario's file name.
    pub name: String,
    pub verdict: Verdict,
    #[serde(default)]
    pub finding: Vec<Finding>,
}

impl Table {
    /// The table for some runs (any order) and scenario results.
    pub fn new(settings: &Settings, runs: &[RunResult], scenarios: Vec<ScenarioRow>) -> Table {
        let mut runs: Vec<&RunResult> = runs.iter().collect();
        runs.sort_by(|a, b| (&a.mix, a.seed).cmp(&(&b.mix, b.seed)));
        let mut mix = Vec::new();
        for group in runs.chunk_by(|a, b| a.mix == b.mix) {
            let refused = group[0].refused.clone();
            let run = if refused.is_some() {
                Vec::new()
            } else {
                group.iter().map(|r| RunRow { seed: r.seed, verdict: r.verdict(), finding: r.findings.clone() }).collect()
            };
            let verdict = group.iter().map(|r| r.verdict()).max().expect("a group has runs");
            mix.push(MixRow { versions: group[0].mix.label(), verdict, refused, run });
        }
        let mut pair = Vec::new();
        for p in Pair::every() {
            let with: Vec<&&RunResult> = runs.iter().filter(|r| r.refused.is_none() && r.mix.pairs().any(|q| q == p)).collect();
            let (a, b) = p.label();
            let row = match with.iter().max_by_key(|r| (r.verdict(), std::cmp::Reverse((&r.mix, r.seed)))) {
                Some(r) => {
                    let v = r.verdict();
                    let example = (v != Verdict::Green).then(|| format!("{} seed {}", r.mix.label(), r.seed));
                    PairRow { a, b, verdict: v, example }
                }
                // Tried only in refused mixes, or not at all.
                None if runs.iter().any(|r| r.mix.pairs().any(|q| q == p)) => PairRow { a, b, verdict: Verdict::Refused, example: None },
                None => continue,
            };
            pair.push(row);
        }
        Table { settings: settings.clone(), mix, pair, scenario: scenarios }
    }

    pub fn text(&self) -> String {
        let header = "# Which module versions work together: written by `combos --write`, read by the window.\n\
                      # Verdicts depend only on the versions and the seed; see crates/sim/src/combos.rs.\n\n";
        header.to_string() + &toml::to_string(self).expect("the table serialises")
    }

    pub fn parse(text: &str) -> Result<Table, String> {
        toml::from_str(text).map_err(|e| format!("cannot read the compatibility table: {e}"))
    }

    /// The table that ships with the game.
    pub fn shipped() -> Result<Table, String> {
        Table::parse(SHIPPED)
    }

    /// What the table says about a set of versions: the verdict of that exact mix if it was run,
    /// otherwise the worst verdict among its pairs. With notes for the window: each finding of the
    /// mix, or each pair that isn't green.
    pub fn verdict_for(&self, modules: &ModuleConfig) -> Option<(Verdict, Vec<String>)> {
        let mix = Mix::of(modules);
        let label = mix.label();
        if let Some(row) = self.mix.iter().find(|m| m.versions == label) {
            let mut notes: Vec<String> = row.refused.iter().cloned().collect();
            for r in &row.run {
                for f in &r.finding {
                    let known = f.known.as_ref().map_or(String::new(), |k| format!(" (known: {k})"));
                    notes.push(format!("seed {}, day {}: {:?}: {}{known}", r.seed, f.day, f.problem, f.what));
                }
            }
            return Some((row.verdict, notes));
        }
        let mut worst: Option<Verdict> = None;
        let mut notes = Vec::new();
        for p in mix.pairs() {
            let (a, b) = p.label();
            if let Some(row) = self.pair.iter().find(|r| r.a == a && r.b == b) {
                worst = worst.max(Some(row.verdict));
                if row.verdict != Verdict::Green {
                    notes.push(format!("{a} with {b}: {:?}{}", row.verdict, row.example.as_ref().map_or(String::new(), |e| format!(", e.g. {e}"))));
                }
            }
        }
        worst.map(|v| (v, notes))
    }
}

/// Open a scenario file's text and check its expectations; a crash is a finding too.
pub fn check_scenario(name: &str, text: &str) -> ScenarioRow {
    let outcome = guarded(|| -> Result<Vec<Finding>, String> {
        let mut t = crate::branch::Timelines::from_scenario_text(text)?;
        let failed = t.check_expectations();
        let mut findings: Vec<Finding> = failed
            .into_iter()
            .map(|(id, why)| Finding { problem: Problem::Expectation, day: 0, what: format!("timeline {id}: {why}"), known: None })
            .collect();
        for tl in &t.list {
            let p = tl.sim.problems;
            if p.contract_violations > 0 {
                findings.push(Finding { problem: Problem::ContractViolated, day: 0, what: format!("timeline {}: {} values clamped", tl.id, p.contract_violations), known: None });
            }
            if p.conservation_breaks > 0 {
                findings.push(Finding { problem: Problem::ConservationBroken, day: 0, what: format!("timeline {}: {} breaks", tl.id, p.conservation_breaks), known: None });
            }
        }
        Ok(findings)
    });
    let finding = match outcome {
        Ok(Ok(f)) => f,
        Ok(Err(why)) => vec![Finding { problem: Problem::Crash, day: 0, what: why, known: None }],
        Err(payload) => vec![Finding { problem: Problem::Crash, day: 0, what: panic_text(payload.as_ref()), known: None }],
    };
    ScenarioRow { name: name.into(), verdict: worst(&finding), finding }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_name_and_parse_their_versions() {
        let d = Mix::defaults();
        assert_eq!(d.label(), "terrain=noise climate=latitude economics=food population=basic weather=droughts");
        assert_eq!(Mix::of(&ModuleConfig::default()), d);
        let m = Mix::parse("economics=off, population=off").unwrap();
        assert_eq!(Mix::of(&m.modules()), m);
        assert!(m.modules().economics.is_none());
        assert!(Mix::parse("economics=gold").unwrap_err().contains("no version gold"));
        assert!(Mix::parse("magic=on").unwrap_err().contains("no slot magic"));
        // People need food to ask for.
        assert!(Mix::parse("economics=off").unwrap().check().unwrap_err().contains("province.stock"));
        assert_eq!(all_mixes().len(), 32);
    }

    #[test]
    fn all_pairs_covers_every_pair_some_accepted_mix_has() {
        let accepted: Vec<Mix> = all_mixes().into_iter().filter(|m| m.check().is_ok()).collect();
        let possible: BTreeSet<Pair> = accepted.iter().flat_map(|m| m.pairs().collect::<Vec<_>>()).collect();
        for seed in [0, 1, 99] {
            let picked = all_pairs(seed);
            assert!(picked.iter().all(|m| m.check().is_ok()));
            let covered: BTreeSet<Pair> = picked.iter().flat_map(|m| m.pairs().collect::<Vec<_>>()).collect();
            assert_eq!(covered, possible, "seed {seed}");
            assert!(picked.len() < accepted.len(), "{} of {}", picked.len(), accepted.len());
        }
    }

    use crate::fields::Quantity;
    use crate::module::{Ctx, Module, Slot};
    use crate::requests::Cause;
    use crate::world::World;

    /// Takes a slot's place, with its manifest, and misbehaves in a chosen way on its cadence.
    struct Rogue {
        manifest: &'static Manifest,
        bad: fn(&mut World, &mut Ctx),
    }

    impl Module for Rogue {
        fn manifest(&self) -> &'static Manifest {
            self.manifest
        }
        fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
            (self.bad)(w, cx);
        }
    }

    /// Score a small world of `mix` whose `slot` module is replaced by a rogue, for `years`.
    fn rogue_run(mix: &str, slot: Slot, years: i64, bad: fn(&mut World, &mut Ctx)) -> Vec<Finding> {
        let mix = Mix::parse(mix).unwrap();
        let settings = Settings { n: 24, provinces: 40, years, seeds: vec![11] };
        let mut sim = Sim::new(&settings.config(&mix, 11)).unwrap();
        let i = sim.modules.iter().position(|m| m.manifest().slot == slot).unwrap();
        let manifest = sim.modules[i].manifest();
        sim.modules[i] = Box::new(Rogue { manifest, bad });
        let mut r = RunResult { mix, seed: 11, refused: None, findings: Vec::new(), days: 0, ms_per_day: 0.0, poke: None };
        play(&mut r, sim, years);
        r.findings
    }

    fn problems(f: &[Finding]) -> Vec<Problem> {
        let mut p: Vec<Problem> = f.iter().map(|f| f.problem).collect();
        p.dedup();
        p
    }

    #[test]
    fn the_scorer_catches_planted_faults() {
        quiet_panics();
        // Food from nowhere, on the first day (a month start): blamed on the economy.
        let f = rogue_run("population=off", Slot::Economics, 1, |w, _| w.stock[0] += 1);
        assert_eq!(problems(&f), vec![Problem::ConservationBroken]);
        assert_eq!(f[0].day, 0);
        assert!(f[0].what.contains("food was") && f[0].what.contains("Economics"), "{}", f[0].what);

        // Missing rain past its bound: clamped back, and so held at the top.
        let f = rogue_run("", Slot::Weather, 2, |w, _| w.rain_shortfall[3] = 5000);
        assert_eq!(problems(&f), vec![Problem::ContractViolated, Problem::StuckAtTop], "{f:?}");
        assert!(f[0].what.contains("province.rain_shortfall of 3 was 5000"), "{}", f[0].what);

        // Writing a value the module doesn't own stops a strict run, and the run is reported.
        let f = rogue_run("", Slot::Economics, 1, |w, _| w.pops.size[0] += 1);
        assert_eq!(problems(&f), vec![Problem::Crash]);
        assert!(f[0].what.contains("which it does not own"), "{}", f[0].what);

        // All the food thrown away every month, declared: nothing is broken, but nothing is left.
        let f = rogue_run("population=off", Slot::Economics, 1, |w, cx| {
            let all: u64 = w.stock.iter().sum();
            w.stock.iter_mut().for_each(|s| *s = 0);
            cx.flow(Quantity::Goods(0), Cause::Spoilage, -(all as i64));
        });
        assert_eq!(problems(&f), vec![Problem::AllGone], "{f:?}");

        // An economy that does nothing: the food store never changes.
        let f = rogue_run("population=off", Slot::Economics, 7, |_, _| {});
        assert_eq!(problems(&f), vec![Problem::Stagnant], "{f:?}");
        assert_eq!(worst(&f), Verdict::Yellow);

        // The same world with its real modules is clean.
        let r = run(&Mix::defaults(), 11, &Settings { n: 24, provinces: 40, years: 7, seeds: vec![11] });
        assert_eq!(r.findings, Vec::new());
    }

    #[test]
    fn the_table_answers_for_a_mix_or_else_for_its_pairs() {
        let settings = Settings { n: 24, provinces: 40, years: 1, seeds: vec![1] };
        let result = |m: &str, findings: Vec<Finding>| RunResult {
            mix: Mix::parse(m).unwrap(),
            seed: 1,
            refused: None,
            findings,
            days: 0,
            ms_per_day: 0.0,
            poke: None,
        };
        let clamp = Finding { problem: Problem::ContractViolated, day: 40, what: "pop.size of 2".into(), known: None };
        let runs = vec![result("", vec![]), result("weather=off", vec![clamp])];
        let table = Table::parse(&Table::new(&settings, &runs, Vec::new()).text()).unwrap();

        let (v, notes) = table.verdict_for(&Mix::parse("weather=off").unwrap().modules()).unwrap();
        assert_eq!(v, Verdict::Red);
        assert_eq!(notes, vec!["seed 1, day 40: ContractViolated: pop.size of 2".to_string()]);
        assert_eq!(table.verdict_for(&ModuleConfig::default()), Some((Verdict::Green, Vec::new())));
        // Not run: the worst of its pairs. Economics off with weather off was never tried.
        let (v, notes) = table.verdict_for(&Mix::parse("terrain=null,weather=off").unwrap().modules()).unwrap();
        assert_eq!(v, Verdict::Red);
        assert!(notes.iter().any(|n| n.starts_with("climate=latitude with weather=off: Red, e.g. ")), "{notes:?}");
        // The shipped table reads, and has every mix.
        assert_eq!(Table::shipped().unwrap().mix.len(), all_mixes().len());
    }

    #[test]
    fn known_quirks_turn_red_yellow_only_where_they_match() {
        let q = Quirks::parse("[[quirk]]\nwhen = [\"weather=off\"]\nproblem = \"all_gone\"\nnote = \"issue 1\"\n").unwrap();
        let finding = |p| Finding { problem: p, day: 3, what: String::new(), known: None };
        let mut r = RunResult {
            mix: Mix::parse("weather=off").unwrap(),
            seed: 1,
            refused: None,
            findings: vec![finding(Problem::AllGone), finding(Problem::Crash)],
            days: 0,
            ms_per_day: 0.0,
            poke: None,
        };
        q.apply(&mut r);
        assert_eq!(r.findings[0].verdict(), Verdict::Yellow);
        assert_eq!(r.verdict(), Verdict::Red, "the crash isn't explained");
        r.findings.pop();
        assert_eq!(r.verdict(), Verdict::Yellow);
        let mut other = RunResult { mix: Mix::defaults(), findings: vec![finding(Problem::AllGone)], ..r.clone() };
        q.apply(&mut other);
        assert_eq!(other.verdict(), Verdict::Red);
        assert!(Quirks::parse("[[quirk]]\nwhen = [\"weather=rain\"]\nproblem = \"crash\"\nnote = \"\"\n").is_err());
    }
}
