//! The speed table: how long a simulated day takes for each mix of module versions, and where the
//! time goes, module by module. Written by `speed --write` into `crates/sim/data/speed.toml`, read
//! by the window beside the stability verdict when versions are picked.
//!
//! Each mix runs on one world twice over: young (after `young_year` years) and busy (after
//! `busy_year`, when realms have grown and wars are going on), `days` days each time, the cheapest
//! of `rounds` tries from the same world (the base mix, being cheap, a whole year at least). Times
//! are the clock's, on one worker thread: measure on a quiet machine. Each module's part is from
//! the engine's clock around its work and the checks around it ([`crate::timing`]).
//!
//! Times depend on the machine, so each mix is also given as a multiple of a base mix measured in
//! the same run (everything that needs realms off: people, food and droughts). Multiples can be
//! compared between machines and runs; milliseconds only within one machine and the same settings.

use crate::combos::{all_mixes, Mix};
use crate::config::CHOICES;
use crate::config::Config;
use crate::module::Slot;
use crate::sim::Sim;
use crate::timing::Timing;
use crate::world::WorldParams;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Cube face size and provinces of the world measured on.
    pub n: usize,
    pub provinces: usize,
    pub seed: u64,
    /// Years run before the young and the busy measures.
    pub young_year: u64,
    pub busy_year: u64,
    /// Days measured each time, and tries (the cheapest counts).
    pub days: u64,
    pub rounds: usize,
}

impl Default for Settings {
    /// A medium world (256 × 256 cells a face, 2,000 provinces).
    fn default() -> Self {
        Settings { n: 256, provinces: 2_000, seed: 7, young_year: 2, busy_year: 30, days: 91, rounds: 3 }
    }
}

impl Settings {
    pub fn config(&self, mix: &Mix) -> Config {
        let world = WorldParams { n: self.n, provinces: self.provinces, seed: self.seed, land_fraction: 0.36, radius_km: 6371.0 };
        Config { modules: mix.modules(), ..Config::new(world) }
    }
}

/// Where and how the table was measured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct About {
    pub machine: String,
    /// Worker threads the simulation ran with (1 is the fair measure).
    pub threads: usize,
    pub commit: String,
    pub date: String,
}

/// One module's part of a day: its own work, and the engine's checks around it, ms a day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleTime {
    pub module: String,
    pub work_ms: f64,
    pub checks_ms: f64,
}

/// A mix's day at one moment of its world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    /// Time a day, ms.
    pub ms: f64,
    /// `ms` as a multiple of the base mix's at the same moment, in the same run.
    pub times_base: f64,
    /// The engine's own time between modules (copies, sorting events, checkpoints), ms a day.
    pub engine_ms: f64,
    /// The modules, most costly first.
    pub modules: Vec<ModuleTime>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// `terrain=noise climate=latitude ...`, as the combination tester writes mixes.
    pub versions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub young: Option<Moment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<Moment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub about: About,
    pub settings: Settings,
    /// The mix the others are measured against.
    pub base: String,
    #[serde(default)]
    pub mix: Vec<Row>,
}

const SHIPPED: &str = include_str!("../data/speed.toml");

/// The base mix: people, food and droughts, nothing that needs realms.
pub fn base_mix() -> Mix {
    let mut m = Mix::defaults();
    for (k, c) in CHOICES.iter().enumerate() {
        if matches!(c.0, Slot::Realms | Slot::Characters | Slot::Diplomacy | Slot::Warfare | Slot::Research | Slot::Culture) {
            m.0[k] = "off";
        }
    }
    m
}

/// The mixes to measure: every accepted mix, the map's versions (terrain, climate) at their
/// defaults unless `map` (they shape the world once, not its days). The base mix first.
pub fn mixes(map: bool) -> Vec<Mix> {
    let defaults = Mix::defaults();
    let base = base_mix();
    let mut out: Vec<Mix> = all_mixes()
        .into_iter()
        .filter(|m| map || CHOICES.iter().enumerate().all(|(k, c)| !matches!(c.0, Slot::Terrain | Slot::Climate) || m.0[k] == defaults.0[k]))
        .filter(|m| m.check().is_ok() && *m != base)
        .collect();
    out.insert(0, base);
    out
}

/// Measure one mix: the young world, then the busy one. Times are not yet multiples of the base
/// (see [`Table::new`]).
pub fn measure(mix: &Mix, s: &Settings) -> Row {
    let versions = mix.label();
    if let Err(why) = mix.check() {
        return Row { versions, refused: Some(why), young: None, busy: None };
    }
    let mut sim = match Sim::new(&s.config(mix)) {
        Ok(sim) => sim,
        Err(e) => return Row { versions, refused: Some(e.to_string()), young: None, busy: None },
    };
    let year = sim.calendar.days_per_year() as u64;
    sim.run_days(s.young_year * year);
    // The base mix is cheap: measured over a year at least, so its days add up to a steady number.
    let days = if *mix == base_mix() { s.days.max(year) } else { s.days };
    let young = moment(&sim, s, days);
    sim.run_days(s.busy_year.saturating_sub(s.young_year) * year);
    let busy = moment(&sim, s, days);
    Row { versions, refused: None, young: Some(young), busy: Some(busy) }
}

/// The cheapest of `rounds` tries of `days` days from `sim` as it stands.
fn moment(sim: &Sim, s: &Settings, days: u64) -> Moment {
    let mut best: Option<(f64, Timing)> = None;
    for _ in 0..s.rounds.max(1) {
        let mut run = sim.fork();
        run.timing = Some(Box::default());
        run.run_days(days);
        let t = *run.timing.take().expect("set above");
        let ms = t.total.as_secs_f64() * 1000.0 / days.max(1) as f64;
        if best.as_ref().is_none_or(|b| ms < b.0) {
            best = Some((ms, t));
        }
    }
    let (ms, t) = best.expect("at least one round");
    let per_day = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / t.days.max(1) as f64;
    let mut modules: Vec<ModuleTime> = t.by_module().into_iter().map(|(slot, w, c)| ModuleTime { module: format!("{slot:?}").to_lowercase(), work_ms: per_day(w), checks_ms: per_day(c) }).collect();
    modules.sort_by(|a, b| (b.work_ms + b.checks_ms).total_cmp(&(a.work_ms + a.checks_ms)).then(a.module.cmp(&b.module)));
    Moment { ms, times_base: 0.0, engine_ms: per_day(t.between()), modules }
}

/// One mix that got slower than in an earlier table.
#[derive(Clone, Debug, PartialEq)]
pub struct Slower {
    pub versions: String,
    /// "young" or "busy".
    pub moment: &'static str,
    /// How many times its time grew, against the middle growth of every mix measured in both
    /// tables: 1.5 is half as slow again as the rest of the table moved.
    pub against_rest: f64,
    /// Multiples of the base, before and now (for showing; not what decides).
    pub was: f64,
    pub now: f64,
    /// Milliseconds before and now, when both tables were measured alike (same machine and
    /// settings).
    pub ms: Option<(f64, f64)>,
}

impl Table {
    /// The table for rows measured in one run; the base mix's row must be among them.
    pub fn new(about: About, settings: Settings, mut rows: Vec<Row>) -> Table {
        let base = base_mix().label();
        let of_base = rows.iter().find(|r| r.versions == base).map(|r| (r.young.as_ref().map(|m| m.ms), r.busy.as_ref().map(|m| m.ms)));
        let (young, busy) = of_base.unwrap_or((None, None));
        for r in &mut rows {
            for (m, b) in [(&mut r.young, young), (&mut r.busy, busy)] {
                if let (Some(m), Some(b)) = (m.as_mut(), b) {
                    m.times_base = if b > 0.0 { m.ms / b } else { 0.0 };
                }
            }
        }
        rows.sort_by(|a, b| a.versions.cmp(&b.versions));
        Table { about, settings, base, mix: rows }
    }

    pub fn text(&self) -> String {
        let header = "# How long a simulated day takes for each mix of module versions, and where the time goes:\n\
                      # written by `speed --write`, read by the window. Milliseconds depend on the machine;\n\
                      # `times_base` (a multiple of the base mix, measured in the same run) compares across\n\
                      # machines. See crates/sim/src/speed.rs.\n\n";
        header.to_string() + &toml::to_string(self).expect("the table serialises")
    }

    pub fn parse(text: &str) -> Result<Table, String> {
        toml::from_str(text).map_err(|e| format!("cannot read the speed table: {e}"))
    }

    /// The table that ships with the game.
    pub fn shipped() -> Result<Table, String> {
        Table::parse(SHIPPED)
    }

    /// The row for a mix of versions, if it was measured.
    pub fn row(&self, mix: &Mix) -> Option<&Row> {
        let label = mix.label();
        self.mix.iter().find(|r| r.versions == label && r.refused.is_none())
    }

    /// Mixes whose time grew more than `factor` times the middle growth of every mix measured in
    /// both tables, young and busy apart. A faster or slower machine, a busy one, or the base mix
    /// measured a little off moves every mix alike, and the middle takes that out: only mixes that
    /// slowed more than the rest are named. With fewer than three mixes in common nothing is.
    pub fn slower_than(&self, old: &Table, factor: f64) -> Vec<Slower> {
        let alike = self.about.machine == old.about.machine && self.about.threads == old.about.threads && self.settings == old.settings;
        let mut out = Vec::new();
        for (name, pick) in [("young", (|r: &Row| r.young.clone()) as fn(&Row) -> Option<Moment>), ("busy", |r: &Row| r.busy.clone())] {
            let pairs: Vec<(&Row, Moment, Moment)> = self
                .mix
                .iter()
                .filter_map(|r| {
                    let o = old.mix.iter().find(|o| o.versions == r.versions)?;
                    Some((r, pick(r)?, pick(o)?)).filter(|(_, n, w)| n.ms > 0.0 && w.ms > 0.0)
                })
                .collect();
            if pairs.len() < 3 {
                continue;
            }
            let mut growth: Vec<f64> = pairs.iter().map(|(_, n, w)| n.ms / w.ms).collect();
            growth.sort_by(f64::total_cmp);
            let middle = growth[growth.len() / 2];
            for (r, n, w) in pairs {
                let against_rest = n.ms / w.ms / middle;
                if against_rest > factor {
                    out.push(Slower { versions: r.versions.clone(), moment: name, against_rest, was: w.times_base, now: n.times_base, ms: alike.then_some((w.ms, n.ms)) });
                }
            }
        }
        out
    }
}

/// Today's date, year-month-day, from the system clock (UTC).
pub fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(ms: f64) -> Moment {
        Moment { ms, times_base: 0.0, engine_ms: 0.1, modules: vec![ModuleTime { module: "warfare".into(), work_ms: ms / 2.0, checks_ms: 0.0 }] }
    }

    fn row(m: &Mix, young: f64, busy: f64) -> Row {
        Row { versions: m.label(), refused: None, young: Some(moment(young)), busy: Some(moment(busy)) }
    }

    #[test]
    fn mixes_are_the_accepted_ones_with_the_base_first() {
        let all = mixes(false);
        assert_eq!(all[0], base_mix());
        assert!(all.iter().all(|m| m.check().is_ok()));
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "no mix twice");
        assert!(mixes(true).len() > all.len(), "the map's versions add mixes");
        assert!(all.contains(&Mix::defaults()));
    }

    #[test]
    fn the_table_gives_multiples_of_the_base_reads_back_and_finds_what_got_slower() {
        let (base, other) = (base_mix(), Mix::defaults());
        let about = About { machine: "test".into(), threads: 1, commit: "abc".into(), date: "2026-10-17".into() };
        let t = Table::new(about.clone(), Settings::default(), vec![row(&other, 3.0, 8.0), row(&base, 1.0, 2.0)]);
        assert_eq!(t.mix[t.mix.iter().position(|r| r.versions == other.label()).unwrap()].busy.as_ref().unwrap().times_base, 4.0);
        assert_eq!(Table::parse(&t.text()).unwrap(), t);
        assert_eq!(t.row(&other).unwrap().young.as_ref().unwrap().ms, 3.0);

        // A third mix, so the middle of the table means something.
        let third = Mix::parse("economics=markets").unwrap();
        let t = Table::new(about.clone(), Settings::default(), vec![row(&other, 3.0, 8.0), row(&base, 1.0, 2.0), row(&third, 5.0, 10.0)]);
        // On a faster machine everything halves: nothing got slower but the busy day of the other
        // mix, which went from 8 to 6 ms while the rest halved: half as slow again as the rest.
        let later = Table::new(About { machine: "faster".into(), ..about.clone() }, Settings::default(), vec![row(&other, 1.5, 6.0), row(&base, 0.5, 1.0), row(&third, 2.5, 5.0)]);
        let slower = later.slower_than(&t, 1.2);
        assert_eq!(slower.len(), 1);
        assert_eq!((slower[0].moment, slower[0].against_rest, slower[0].was, slower[0].now, slower[0].ms), ("busy", 1.5, 4.0, 6.0, None));
        // The base mix measured a fifth low moves every multiple of it, but no mix is named: the
        // middle of the table didn't move.
        let low_base = Table::new(about.clone(), Settings::default(), vec![row(&other, 3.0, 8.0), row(&base, 0.8, 1.6), row(&third, 5.0, 10.0)]);
        assert!(low_base.slower_than(&t, 1.2).is_empty());
        // Measured alike, milliseconds compare too.
        let alike = Table::new(about, Settings::default(), vec![row(&other, 3.0, 12.0), row(&base, 1.0, 2.0), row(&third, 5.0, 10.0)]);
        assert_eq!(alike.slower_than(&t, 1.2)[0].ms, Some((8.0, 12.0)));
        // With fewer than three mixes in common there is no middle to go by.
        assert!(alike.slower_than(&Table { mix: t.mix[..2].to_vec(), ..t.clone() }, 1.2).is_empty());
    }

    #[test]
    fn the_shipped_table_reads() {
        Table::shipped().unwrap();
    }

    #[test]
    fn dates_are_civil() {
        let d = today();
        assert_eq!((d.len(), &d[4..5], &d[7..8]), (10, "-", "-"));
    }
}
