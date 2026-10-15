//! Scenario files: a world as readable text (TOML) that can be edited by hand. A scenario holds the
//! world's settings, its module versions and their parameters, its calendar, and for each timeline
//! the commands given in it (the player's edits and pokes, including pokes scheduled for days still
//! to come) and the day it had reached. Loading one makes the world again from its settings and
//! replays each timeline's commands up to its day, so it gives exactly the world that was saved.
//! That takes as long as running those days did; a binary save loads at once.
//!
//! Scenarios double as presets for play and as test worlds.

use crate::branch::{Timeline, Timelines};
use crate::config::Config;
use crate::edit::Edit;
use crate::events::Event;
use crate::sim::{Command, CommandKind, Sim, YearSummary};
use serde::{Deserialize, Serialize};

/// Bump on any change to the layout below or to the types it holds.
pub const SCENARIO_FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub format: u32,
    /// The timeline played when the scenario is opened.
    pub active: u32,
    pub config: Config,
    pub timeline: Vec<ScenarioTimeline>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioTimeline {
    pub id: u32,
    /// The timeline it left, if any.
    pub parent: Option<u32>,
    /// The day it left its parent.
    pub fork_day: Option<u64>,
    /// The day it had reached: loading runs it to here.
    pub day: u64,
    /// The commands given in it, in day order, after the configuration's own first one.
    pub command: Vec<ScenarioCommand>,
    /// What should have happened by chosen years: a scenario with these is a test case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<Expect>,
}

/// A year total an expectation can check, from that year's summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// People at the end of the year.
    Population,
    /// Stored food at the end of the year.
    FoodStock,
    /// Food harvested during the year.
    Produced,
    Births,
    /// All deaths, starvation included.
    Deaths,
    Starved,
    /// Provinces where anyone starved during the year.
    FamineProvinces,
}

impl Measure {
    pub fn of(self, y: &YearSummary) -> u64 {
        match self {
            Measure::Population => y.population,
            Measure::FoodStock => y.food_stock,
            Measure::Produced => y.ledger.produced,
            Measure::Births => y.ledger.births,
            Measure::Deaths => y.ledger.deaths,
            Measure::Starved => y.ledger.starved,
            Measure::FamineProvinces => y.famine_provinces as u64,
        }
    }
}

/// An expected range for one year's total, either end optional.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    pub year: i64,
    pub measure: Measure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

impl Expect {
    /// `Err` with what was found when the value is out of range.
    pub fn check(&self, value: u64) -> Result<(), String> {
        let low = self.min.is_some_and(|m| value < m);
        let high = self.max.is_some_and(|m| value > m);
        if low || high {
            let range = match (self.min, self.max) {
                (Some(a), Some(b)) => format!("{a} to {b}"),
                (Some(a), None) => format!("at least {a}"),
                (None, Some(b)) => format!("at most {b}"),
                (None, None) => unreachable!("an empty range can't fail"),
            };
            return Err(format!("year {}: {:?} was {value}, expected {range}", self.year, self.measure));
        }
        Ok(())
    }
}

/// A command as a scenario writes it: its day, and an edit or a poke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioCommand {
    pub day: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<Edit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poke: Option<Event>,
}

impl ScenarioCommand {
    fn of(c: &Command) -> Option<ScenarioCommand> {
        match &c.kind {
            CommandKind::Edit { edit } => Some(ScenarioCommand { day: c.day, edit: Some(edit.clone()), poke: None }),
            CommandKind::Poke { event } => Some(ScenarioCommand { day: c.day, edit: None, poke: Some(event.clone()) }),
            CommandKind::StartSegment { .. } => None,
        }
    }

    fn command(&self) -> Result<Command, String> {
        let kind = match (&self.edit, &self.poke) {
            (Some(edit), None) => CommandKind::Edit { edit: edit.clone() },
            (None, Some(event)) => CommandKind::Poke { event: event.clone() },
            _ => return Err(format!("the command on day {} needs one edit or one poke", self.day)),
        };
        Ok(Command { day: self.day, kind })
    }
}

impl Timelines {
    /// Every timeline as a scenario.
    pub fn to_scenario(&self) -> Scenario {
        let timeline = self
            .list
            .iter()
            .map(|t| ScenarioTimeline {
                id: t.id,
                parent: t.parent.map(|p| p.0),
                fork_day: t.parent.map(|p| p.1),
                day: t.sim.day,
                command: t.sim.commands.iter().filter_map(ScenarioCommand::of).collect(),
                expect: t.expect.clone(),
            })
            .collect();
        Scenario { format: SCENARIO_FORMAT, active: self.active_id(), config: self.sim().config.clone(), timeline }
    }

    /// The scenario as TOML text.
    pub fn scenario_text(&self) -> String {
        let header = "# A Fantasy Planet scenario. Each [[timeline]] lists the edits and pokes given in it,\n\
                      # day by day; opening the scenario replays them on the world the [config] makes.\n\n";
        header.to_string() + &toml::to_string(&self.to_scenario()).expect("scenarios serialise")
    }

    /// Make every timeline of a scenario: the world from its settings, then each timeline's commands
    /// replayed up to its day.
    pub fn from_scenario(s: &Scenario) -> Result<Timelines, String> {
        if s.format != SCENARIO_FORMAT {
            return Err(format!("scenario format {}; this build reads format {SCENARIO_FORMAT}", s.format));
        }
        let mut list = Vec::new();
        for t in &s.timeline {
            if t.command.windows(2).any(|w| w[1].day < w[0].day) {
                return Err(format!("timeline {}: commands out of day order", t.id));
            }
            let commands = t.command.iter().map(ScenarioCommand::command).collect::<Result<Vec<_>, _>>().map_err(|e| format!("timeline {}: {e}", t.id))?;
            let mut sim = Sim::new(&s.config).map_err(|e| e.to_string())?;
            sim.replay_commands(commands, t.day);
            list.push(Timeline { id: t.id, parent: t.parent.zip(t.fork_day), sim, expect: t.expect.clone() });
        }
        let active = list.iter().position(|t| t.id == s.active).ok_or_else(|| format!("no timeline {} to play", s.active))?;
        Ok(Timelines { list, active })
    }

    /// Read a scenario from TOML text and make its timelines.
    pub fn from_scenario_text(text: &str) -> Result<Timelines, String> {
        let s: Scenario = toml::from_str(text).map_err(|e| format!("cannot read scenario: {e}"))?;
        Timelines::from_scenario(&s)
    }
}

impl Timelines {
    /// Check every timeline's expectations, running each on to the end of the last year they name.
    /// Returns what failed, as `(timeline, why)`; empty when everything held.
    pub fn check_expectations(&mut self) -> Vec<(u32, String)> {
        let mut failed = Vec::new();
        for t in &mut self.list {
            let Some(last) = t.expect.iter().map(|e| e.year).max() else { continue };
            let s = &mut t.sim;
            s.run_to_year(last + 1);
            // The year just ended is closed only when the next one runs.
            let mut years = s.history.clone();
            if s.day > 0 {
                years.push(s.year_so_far());
            }
            for e in &t.expect {
                let result = match years.iter().find(|y| y.year == e.year) {
                    Some(y) => e.check(e.measure.of(y)),
                    None => Err(format!("year {} was never reached", e.year)),
                };
                if let Err(why) = result {
                    failed.push((t.id, why));
                }
            }
        }
        failed
    }

    /// Open a world from a file: a scenario (`.toml`), every timeline ([`Timelines::save`]), or
    /// one simulation ([`Sim::save`]).
    pub fn open(path: &std::path::Path) -> Result<Timelines, String> {
        let name = path.display();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml")) {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
            return Timelines::from_scenario_text(&text);
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        if bytes.starts_with(crate::branch::MAGIC) {
            Timelines::load(&bytes).map_err(|e| e.to_string())
        } else {
            Sim::load(&bytes).map(Timelines::new).map_err(|e| e.to_string())
        }
    }

    /// Write every timeline to a file: as a scenario if it ends in `.toml`, else as saves.
    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        let scenario = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let bytes = if scenario { self.scenario_text().into_bytes() } else { self.save() };
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

impl Sim {
    /// On a world just made: take `commands` as the command log after the configuration's own, and
    /// run to `day`, applying them on their days.
    fn replay_commands(&mut self, commands: Vec<Command>, day: u64) {
        debug_assert_eq!(self.day, 0);
        self.commands.truncate(1);
        self.commands.extend(commands);
        // Day 0 was arrived at before the log was known.
        self.apply_todays_edits();
        while self.day < day {
            self.step_day();
        }
    }
}
