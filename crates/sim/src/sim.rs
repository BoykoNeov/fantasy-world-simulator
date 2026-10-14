//! The running simulation: a world, a calendar, and a minimal daily scheduler.
//!
//! The clock ticks one day at a time. Each system runs on its own cadence, asked of the calendar, in
//! a fixed order. A day on which nothing is due costs almost nothing.

use crate::calendar::Calendar;
use crate::pops::{Ledger, PopParams, PopState};
use crate::world::{Fingerprint, World, WorldParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Cadence {
    pub fn due(self, cal: &Calendar, day: u64) -> bool {
        match self {
            Cadence::Daily => true,
            Cadence::Weekly => cal.is_week_start(day),
            Cadence::Monthly => cal.is_month_start(day),
            Cadence::Yearly => cal.is_year_start(day),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    /// Closes the year that just ended: records a summary and resets yearly counters.
    YearSummary,
    Pops,
}

/// Run order within a day. The summary runs first so it sees the year as it ended.
pub const SCHEDULE: [(Cadence, System); 2] = [(Cadence::Yearly, System::YearSummary), (Cadence::Monthly, System::Pops)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YearSummary {
    pub year: i64,
    /// Population at the end of the year.
    pub population: u64,
    pub food_stock: u64,
    pub ledger: Ledger,
    /// Provinces where anyone starved during the year.
    pub famine_provinces: usize,
}

pub struct Sim {
    pub world: World,
    pub calendar: Calendar,
    pub pop_params: PopParams,
    pub pops: PopState,
    /// The day about to run; day 0 is the first day of the calendar's first year.
    pub day: u64,
    /// Totals for the year in progress.
    pub year_ledger: Ledger,
    pub history: Vec<YearSummary>,
}

impl Sim {
    pub fn new(params: &WorldParams, calendar: Calendar, pop_params: PopParams) -> Sim {
        let world = World::generate(params);
        let pops = PopState::initial(&world, &calendar, &pop_params);
        Sim { world, calendar, pop_params, pops, day: 0, year_ledger: Ledger::default(), history: Vec::new() }
    }

    /// Run one day. Returns whether any system ran (false = an idle day).
    pub fn step_day(&mut self) -> bool {
        let mut ran = false;
        for (cadence, system) in SCHEDULE {
            if cadence.due(&self.calendar, self.day) {
                self.run(system);
                ran = true;
            }
        }
        self.day += 1;
        ran
    }

    pub fn run_days(&mut self, days: u64) {
        for _ in 0..days {
            self.step_day();
        }
    }

    /// Run until the first day of `year` (the year has not started yet).
    pub fn run_to_year(&mut self, year: i64) {
        let target = self.calendar.year_start(year);
        while self.day < target {
            self.step_day();
        }
    }

    fn run(&mut self, system: System) {
        match system {
            System::YearSummary => {
                if self.day > 0 {
                    let summary = self.year_so_far();
                    self.history.push(summary);
                }
                self.year_ledger = Ledger::default();
                self.pops.starved_this_year.iter_mut().for_each(|s| *s = 0);
            }
            System::Pops => {
                let l = self.pops.month(&self.world, &self.calendar, &self.pop_params, self.day);
                self.year_ledger.add(&l);
            }
        }
    }

    /// Summary of the year that contains the last day run, as it stands now.
    pub fn year_so_far(&self) -> YearSummary {
        YearSummary {
            year: self.calendar.date(self.day.saturating_sub(1)).year,
            population: self.pops.total_population(),
            food_stock: self.pops.food_stock.iter().sum(),
            ledger: self.year_ledger,
            famine_provinces: self.pops.starved_this_year.iter().filter(|&&s| s > 0).count(),
        }
    }

    /// Bit-exact hash of the whole simulation state.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fingerprint::default();
        self.world.fingerprint(&mut h);
        h.u64s(&[self.day]);
        let p = &self.pops.pops;
        h.u32s(&p.province);
        h.bytes(&p.species.iter().chain(&p.culture).chain(&p.religion).flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());
        h.bytes(&p.class);
        h.u64s(&p.size);
        h.u64s(&self.pops.food_stock);
        h.u64s(&self.pops.starved_this_year);
        h.0
    }
}
