//! Diplomacy (step 7): wars, peace and truces between realms.
//!
//! Diplomacy v0, "wars" ([`WarState`]), keeps the list of wars: who fights whom, why, from which
//! day to which, who won, and the day the truce after it ends. Two realms are at war while a war
//! between them goes on, under a truce until the day its truce ends, and at peace otherwise. Wars
//! start and end by the player's pokes ([`Event::DeclareWar`], [`Event::ForcePeace`]); why realms
//! choose war themselves is a later version's.
//!
//! At the peace the winner keeps every province of the loser's it occupies, and everything else
//! goes back: no peace changes hands any other land, and with nobody winning nothing changes hands
//! at all. Diplomacy works out nothing of this itself: it sends [`Event::Peace`], and the owner of
//! provinces and occupation (the realms module) hands the land over. A war one of whose realms
//! is dissolved ends at the next month's start, won by the other; until then [`at_war`] already
//! treats it as over.
//!
//! War numbers are never reused: an ended war keeps its entry, as dissolved realms do.

use crate::events::{Event, EventKind};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::world::World;
use serde::{Deserialize, Serialize};

/// `war.ended` while the war goes on, and `war.truce_until` before it has ended.
pub const NEVER: u64 = u64::MAX;

/// One entry per war, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wars {
    /// The realm that declared it (field `war.attacker`).
    pub attacker: Vec<u32>,
    /// The realm it was declared on (field `war.defender`).
    pub defender: Vec<u32>,
    /// Why, a [`WarCause`] as a number (field `war.cause`).
    pub cause: Vec<u8>,
    /// The day it was declared (field `war.began`).
    pub began: Vec<u64>,
    /// The day the peace was made (field `war.ended`), [`NEVER`] while it goes on.
    pub ended: Vec<u64>,
    /// The realm that won (field `war.winner`), [`NONE`] while it goes on or when nobody won.
    pub winner: Vec<u32>,
    /// The first day the two may fight again (field `war.truce_until`), [`NEVER`] while it goes
    /// on. A war declared during the truce breaks it: the truce then ends that day.
    pub truce_until: Vec<u64>,
}

impl Wars {
    pub fn count(&self) -> usize {
        self.attacker.len()
    }

    pub fn ongoing(&self, war: u32) -> bool {
        self.ended.get(war as usize).is_some_and(|&d| d == NEVER)
    }

    /// Whether war `war` is between realms `a` and `b`, either way round.
    pub fn between(&self, war: u32, a: u32, b: u32) -> bool {
        let (x, y) = (self.attacker[war as usize], self.defender[war as usize]);
        (x, y) == (a, b) || (x, y) == (b, a)
    }

    /// The other side of war `war` from `r`, or [`NONE`] if `r` isn't in it.
    pub fn other(&self, war: u32, r: u32) -> u32 {
        let (x, y) = (self.attacker[war as usize], self.defender[war as usize]);
        if r == x { y } else if r == y { x } else { NONE }
    }

    /// The war going on between `a` and `b`, if one is.
    pub fn current(&self, a: u32, b: u32) -> Option<u32> {
        (0..self.count() as u32).rev().find(|&k| self.ongoing(k) && self.between(k, a, b))
    }

    pub(crate) fn push(&mut self, attacker: u32, defender: u32, cause: WarCause, day: u64) -> u32 {
        let k = self.count() as u32;
        self.attacker.push(attacker);
        self.defender.push(defender);
        self.cause.push(cause as u8);
        self.began.push(day);
        self.ended.push(NEVER);
        self.winner.push(NONE);
        self.truce_until.push(NEVER);
        k
    }
}

/// Why a war was declared. Later versions add their reasons (food, a rich province, rebellion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WarCause {
    /// The player's poke.
    Poke = 0,
}

impl WarCause {
    pub fn from_u8(_n: u8) -> WarCause {
        WarCause::Poke
    }

    pub fn describe(self) -> &'static str {
        match self {
            WarCause::Poke => "declared by the player",
        }
    }
}

/// Why a poke about war or peace was turned down. The refusal is sent as [`Event::WarRefused`],
/// so the event log says what happened to the poke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// A realm can't fight itself.
    SameRealm,
    /// One of the two realms doesn't exist, or has been dissolved.
    NoSuchRealm,
    /// The two are at war already.
    AlreadyAtWar,
    /// The two are not at war, so there is no peace to make.
    NotAtWar,
    /// The winner named is neither of the two.
    WinnerNotInWar,
}

impl Refusal {
    pub fn describe(self) -> &'static str {
        match self {
            Refusal::SameRealm => "a realm can't go to war with itself",
            Refusal::NoSuchRealm => "one of the realms doesn't exist or has been dissolved",
            Refusal::AlreadyAtWar => "they are already at war",
            Refusal::NotAtWar => "they are not at war",
            Refusal::WinnerNotInWar => "the winner named is not one of the two",
        }
    }
}

/// How two realms stand with each other on a day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Peace,
    /// Under a truce, until the day given (the first day they may fight again).
    Truce(u64),
    /// At war, in the war given.
    War(u32),
}

/// How realms `a` and `b` stand on `day`. A war with a dissolved realm counts as over, though its
/// entry ends only at the next month's start.
pub fn standing(w: &World, a: u32, b: u32, day: u64) -> Standing {
    let wars = &w.wars;
    let mut truce = None;
    for k in (0..wars.count() as u32).rev() {
        if !wars.between(k, a, b) {
            continue;
        }
        if wars.ongoing(k) {
            if w.realms.alive(a) && w.realms.alive(b) {
                return Standing::War(k);
            }
            continue;
        }
        let until = wars.truce_until[k as usize];
        if truce.is_none() && until > day {
            truce = Some(until);
        }
    }
    truce.map_or(Standing::Peace, Standing::Truce)
}

/// Whether realms `a` and `b` are at war.
pub fn at_war(w: &World, a: u32, b: u32) -> bool {
    a != b && a != NONE && b != NONE && matches!(standing(w, a, b, 0), Standing::War(_))
}

/// Each realm's enemies today, sorted: for readers that ask about many pairs (trade along every
/// way) and should work them out once.
pub fn enemies(w: &World) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); w.realms.count()];
    let wars = &w.wars;
    for k in 0..wars.count() {
        let (a, b) = (wars.attacker[k], wars.defender[k]);
        if wars.ended[k] == NEVER && w.realms.alive(a) && w.realms.alive(b) {
            out[a as usize].push(b);
            out[b as usize].push(a);
        }
    }
    for v in &mut out {
        v.sort_unstable();
        v.dedup();
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarParams {
    /// How long a truce lasts after a peace, in years.
    pub truce_years: f64,
}

impl Default for WarParams {
    fn default() -> Self {
        WarParams { truce_years: 5.0 }
    }
}

pub struct WarState {
    pub params: WarParams,
}

static V0: Manifest = Manifest {
    slot: Slot::Diplomacy,
    version: "wars",
    tier: crate::fields::Tier::War,
    cadence: Cadence::Monthly,
    reads: &["realm.capital"],
    owns: &["war.attacker", "war.defender", "war.cause", "war.began", "war.ended", "war.winner", "war.truce_until"],
    requests: &[],
    asks: &[],
    moves: &[],
    // Wars are between realms: without them there is nobody to fight.
    requires: &["realm.capital"],
    emits: &[EventKind::WarDeclared, EventKind::Peace, EventKind::WarRefused],
    handles: &[EventKind::DeclareWar, EventKind::ForcePeace],
    state_schema: "none",
    goods: &[],
};

impl Module for WarState {
    fn manifest(&self) -> &'static Manifest {
        &V0
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let t = self.params.truce_years;
        if !(t.is_finite() && t >= 0.0) {
            return Err("a truce lasts zero years or more".into());
        }
        Ok(())
    }

    /// Wars with a realm that has been dissolved end, won by the other side.
    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        for k in 0..w.wars.count() as u32 {
            if !w.wars.ongoing(k) {
                continue;
            }
            let (a, d) = (w.wars.attacker[k as usize], w.wars.defender[k as usize]);
            let winner = match (w.realms.alive(a), w.realms.alive(d)) {
                (true, true) => continue,
                (true, false) => a,
                (false, true) => d,
                (false, false) => NONE,
            };
            end(w, cx, &self.params, k, winner);
        }
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        match *e {
            Event::DeclareWar { attacker, defender } => declare(w, cx, attacker, defender),
            Event::ForcePeace { a, b, winner } => {
                let refused = if a == b {
                    Some(Refusal::SameRealm)
                } else if !(w.realms.alive(a) && w.realms.alive(b)) {
                    Some(Refusal::NoSuchRealm)
                } else if winner != NONE && winner != a && winner != b {
                    Some(Refusal::WinnerNotInWar)
                } else {
                    None
                };
                match (refused, w.wars.current(a, b)) {
                    (Some(why), _) => cx.emit(Event::WarRefused { attacker: a, defender: b, why }),
                    (None, None) => cx.emit(Event::WarRefused { attacker: a, defender: b, why: Refusal::NotAtWar }),
                    (None, Some(k)) => end(w, cx, &self.params, k, winner),
                }
            }
            _ => {}
        }
    }
}

/// `attacker` declares war on `defender` today, by the player's poke. A truce between them is
/// broken: it ends today.
fn declare(w: &mut World, cx: &mut Ctx, attacker: u32, defender: u32) {
    let why = if attacker == defender {
        Some(Refusal::SameRealm)
    } else if !(w.realms.alive(attacker) && w.realms.alive(defender)) {
        Some(Refusal::NoSuchRealm)
    } else if w.wars.current(attacker, defender).is_some() {
        Some(Refusal::AlreadyAtWar)
    } else {
        None
    };
    if let Some(why) = why {
        cx.emit(Event::WarRefused { attacker, defender, why });
        return;
    }
    let wars = &mut w.wars;
    for k in 0..wars.count() as u32 {
        if wars.between(k, attacker, defender) && wars.truce_until[k as usize] > cx.day {
            wars.truce_until[k as usize] = cx.day;
        }
    }
    let war = wars.push(attacker, defender, WarCause::Poke, cx.day);
    cx.emit(Event::WarDeclared { war, attacker, defender, cause: WarCause::Poke });
    for r in [attacker, defender] {
        let capital = w.realms.capital[r as usize];
        if cx.traced(capital) {
            cx.note(format!("realm {attacker} declared war on realm {defender} (war {war}): {}", WarCause::Poke.describe()));
        }
    }
}

/// War `k` ends today: `winner` (or nobody, [`NONE`]) keeps what it occupies of the other's land,
/// handed over by the realms module when [`Event::Peace`] arrives. A truce follows.
fn end(w: &mut World, cx: &mut Ctx, params: &WarParams, k: u32, winner: u32) {
    let ku = k as usize;
    let truce = (params.truce_years * cx.calendar.days_per_year() as f64).round() as u64;
    w.wars.ended[ku] = cx.day;
    w.wars.winner[ku] = winner;
    w.wars.truce_until[ku] = cx.day.saturating_add(truce);
    let (attacker, defender) = (w.wars.attacker[ku], w.wars.defender[ku]);
    cx.emit(Event::Peace { war: k, attacker, defender, winner });
    for r in [attacker, defender] {
        let capital = w.realms.capital.get(r as usize).copied().unwrap_or(NONE);
        if capital != NONE && cx.traced(capital) {
            let who = if winner == NONE { "nobody won".to_string() } else { format!("realm {winner} won") };
            cx.note(format!("peace between realms {attacker} and {defender} (war {k}): {who}"));
        }
    }
}
