//! Diplomacy (step 7): wars, peace and truces between realms, and, in its second version, why
//! realms choose them.
//!
//! Diplomacy v0, "wars" ([`WarState`] without treaties), keeps the list of wars: who fights whom,
//! why, from which day to which, who won, and the day the truce after it ends. Two realms are at war
//! while a war between them goes on, under a truce until the day its truce ends, and at peace
//! otherwise. Wars start and end by the player's pokes ([`Event::DeclareWar`], [`Event::ForcePeace`]).
//!
//! Rebels who break away ([`Event::Rebellion`]) are at war with the realm they left, from the day
//! after. On a month's first day, a rebellion's war in which the realm the rebels left occupies the
//! rebels' capital ends, won by that realm; one still going after [`WarParams::rebel_war_years`]
//! ends with the rebels free: they win. At a rebellion's peace the realm they broke from, winning,
//! takes back all their land (the realms module does that). Other wars end by the player's pokes.
//!
//! At the peace the winner keeps every province of the loser's it occupies, and everything else
//! goes back: no peace changes hands any other land, and with nobody winning nothing changes hands
//! at all. Diplomacy works out nothing of this itself: it sends [`Event::Peace`], and the owner of
//! provinces and occupation (the realms module) hands the land over. A war one of whose realms
//! is dissolved ends at the next month's start, won by the other; until then [`at_war`] already
//! treats it as over.
//!
//! Diplomacy v1, "treaties" ([`WarParams::treaties`] set), keeps all of that and adds, each month:
//!
//! - **Opinions** ([`Relations`]): what each of two realms thinks of the other, from −100 to 100,
//!   kept for realms whose armies can reach each other and any two that have fought. At peace,
//!   outside a truce, opinions warm toward a ceiling; rivals (neighbours of about one strength)
//!   cool a little. A war declared, battles, occupation and land lost at a peace sour them.
//! - **Wants**: a realm whose people starved past a line over the last twelve months wants a
//!   neighbour's food; every realm likes the richest province it can reach (coins where the world
//!   keeps them, people and food stores where it doesn't), more under a greedy ruler.
//! - **Declaring war** on a realm its armies can reach, at peace with it and not under a truce,
//!   not much stronger than it, with a monthly chance that grows with what it wants, how much
//!   stronger it is, a warlike ruler and a low opinion. A realm already at war declares another
//!   only when it is much stronger than all its enemies together. The war keeps why (food or
//!   riches) and the province it was declared over (`war.target`), which the general goes for.
//! - **Peace by treaty**: each side of each war wears out with the share of its people under
//!   enemy occupation, the years at war, the battles it lost, the soldiers it lost and the unrest
//!   at home. A war ends when one side is worn out, or when the attacker holds the war's target and
//!   the defender is half worn out; the less worn side wins (nobody, when both are worn out alike).
//!   Rebellions' wars too, either way.
//!
//! A realm's strength is its people: Diplomacy can't read the armies, since Warfare reads the wars.
//! It counts battles and soldiers lost from the battle events. "treaties" needs armies: without
//! them nothing is occupied and no battle is fought, and every war would end with nobody winning.
//!
//! War numbers and relation numbers are never reused: an ended war keeps its entry, as dissolved
//! realms do, and so does a relation with a dissolved realm.

use crate::events::{Event, EventKind};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::rng::{purpose, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// The province the war was declared over (field `war.target`), [`NONE`] for the player's
    /// pokes and rebellions, and always under "wars".
    pub target: Vec<u32>,
    /// Soldiers each side lost in the war's battles (fields `war.attacker_dead`,
    /// `war.defender_dead`), and battles each side lost (`war.attacker_lost`,
    /// `war.defender_lost`). Counted under "treaties"; 0 under "wars".
    pub attacker_dead: Vec<u64>,
    pub defender_dead: Vec<u64>,
    pub attacker_lost: Vec<u32>,
    pub defender_lost: Vec<u32>,
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

    pub(crate) fn push(&mut self, attacker: u32, defender: u32, cause: WarCause, day: u64, target: u32) -> u32 {
        let k = self.count() as u32;
        self.attacker.push(attacker);
        self.defender.push(defender);
        self.cause.push(cause as u8);
        self.began.push(day);
        self.ended.push(NEVER);
        self.winner.push(NONE);
        self.truce_until.push(NEVER);
        self.target.push(target);
        self.attacker_dead.push(0);
        self.defender_dead.push(0);
        self.attacker_lost.push(0);
        self.defender_lost.push(0);
        k
    }

    /// Soldiers lost and battles lost by realm `r`'s side of war `k`.
    pub fn tally(&self, k: u32, r: u32) -> (u64, u32) {
        let ku = k as usize;
        if self.attacker[ku] == r { (self.attacker_dead[ku], self.attacker_lost[ku]) } else { (self.defender_dead[ku], self.defender_lost[ku]) }
    }
}

/// What two realms think of each other: one entry per pair, as parallel arrays. Kept by Diplomacy
/// "treaties" for realms whose armies can reach each other and any two that have fought; not a
/// table of every pair, which would grow without end as realms break away.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relations {
    /// The two realms (fields `relation.a`, `relation.b`), the lower number first.
    pub a: Vec<u32>,
    pub b: Vec<u32>,
    /// What `a` thinks of `b` (field `relation.a_of_b`) and `b` of `a` (`relation.b_of_a`), from
    /// −100 to 100.
    pub a_of_b: Vec<f32>,
    pub b_of_a: Vec<f32>,
}

impl Relations {
    pub fn count(&self) -> usize {
        self.a.len()
    }

    /// The relation between `x` and `y`, if one is kept.
    pub fn find(&self, x: u32, y: u32) -> Option<u32> {
        let (a, b) = (x.min(y), x.max(y));
        (0..self.count()).find(|&k| self.a[k] == a && self.b[k] == b).map(|k| k as u32)
    }

    /// What realm `x` thinks of realm `y`: 0 when no relation is kept.
    pub fn opinion(&self, x: u32, y: u32) -> f32 {
        self.find(x, y).map_or(0.0, |k| self.of(k, x))
    }

    /// What realm `x`, one of relation `k`'s two, thinks of the other.
    pub fn of(&self, k: u32, x: u32) -> f32 {
        let ku = k as usize;
        if self.a[ku] == x { self.a_of_b[ku] } else { self.b_of_a[ku] }
    }

    pub(crate) fn set(&mut self, k: u32, x: u32, value: f32) {
        let ku = k as usize;
        let v = value.clamp(-OPINION, OPINION);
        if self.a[ku] == x {
            self.a_of_b[ku] = v;
        } else {
            self.b_of_a[ku] = v;
        }
    }

    pub(crate) fn push(&mut self, x: u32, y: u32, start: f32) -> u32 {
        let k = self.count() as u32;
        self.a.push(x.min(y));
        self.b.push(x.max(y));
        self.a_of_b.push(start);
        self.b_of_a.push(start);
        k
    }
}

/// Opinions run from −this to this.
pub const OPINION: f32 = 100.0;

/// Why a war was declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WarCause {
    /// The player's poke.
    Poke = 0,
    /// Rebels broke away, and fight the realm they left.
    Rebellion = 1,
    /// A hungry realm wants a neighbour's food.
    Food = 2,
    /// A realm wants a rich province.
    Riches = 3,
}

impl WarCause {
    pub const ALL: [WarCause; 4] = [WarCause::Poke, WarCause::Rebellion, WarCause::Food, WarCause::Riches];

    pub fn from_u8(n: u8) -> WarCause {
        match n {
            1 => WarCause::Rebellion,
            2 => WarCause::Food,
            3 => WarCause::Riches,
            _ => WarCause::Poke,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            WarCause::Poke => "declared by the player",
            WarCause::Rebellion => "rebels broke away and fight for their freedom",
            WarCause::Food => "a hungry realm wants its neighbour's food",
            WarCause::Riches => "a realm wants a rich province",
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
    let wars = &w.wars;
    enemies_in(&w.realms.capital, &wars.attacker, &wars.defender, &wars.ended)
}

/// [`enemies`] from the fields themselves: each realm's capital (`realm.capital`, which says which
/// realms are dissolved) and the wars' `war.attacker`, `war.defender` and `war.ended`. The one rule
/// for who is at war, for readers of the fields as the day began (trade) as for the rest: a war
/// goes on until its peace, and a war with a dissolved realm counts as over.
pub fn enemies_in(capital: &[u32], attacker: &[u32], defender: &[u32], ended: &[u64]) -> Vec<Vec<u32>> {
    let alive = |r: u32| capital.get(r as usize).is_some_and(|&c| c != NONE);
    let mut out = vec![Vec::new(); capital.len()];
    for k in 0..attacker.len() {
        let (a, b) = (attacker[k], defender[k]);
        if ended[k] == NEVER && alive(a) && alive(b) {
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
    /// A rebellion's war still going after this many years ends with the rebels winning: a stand-in
    /// until armies fight it out. Scenario files written before rebellions get the default.
    #[serde(default = "rebel_war_years")]
    pub rebel_war_years: f64,
    /// Diplomacy v1, "treaties": realms choose war and make peace themselves. `None` is v0,
    /// "wars". Scenario files written before it have no key and get "wars".
    #[serde(default)]
    pub treaties: Option<TreatyParams>,
}

fn rebel_war_years() -> f64 {
    5.0
}

impl Default for WarParams {
    fn default() -> Self {
        WarParams { truce_years: 5.0, rebel_war_years: rebel_war_years(), treaties: None }
    }
}

/// How realms think of each other, want, go to war and make peace (Diplomacy "treaties"). Starting
/// values, tuned on 150-year runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreatyParams {
    /// The monthly chance a realm declares war on its best target at a score of 1 (see the module
    /// notes); scores multiply what it wants, how much stronger it is, its ruler's warlike trait
    /// and its opinion.
    pub war_chance: f64,
    /// Scores above this count as this.
    pub most_score: f64,
    /// A realm wants food when this share of its people starved over the last twelve months.
    pub hunger_line: f64,
    /// A realm never declares war on one stronger than itself over this (its strength at least
    /// this times the other's).
    pub least_strength: f64,
    /// A realm already at war declares another only when at least this times as strong as all its
    /// enemies together, the new one included.
    pub second_war: f64,
    /// What two realms think of each other when they first meet, and rebels and the realm they
    /// left.
    pub opinion_start: f64,
    pub opinion_rebels: f64,
    /// At peace and outside a truce, each opinion rises this much a month, up to `peace_ceiling`.
    pub peace_rise: f64,
    pub peace_ceiling: f64,
    /// Rivals, neighbours within this ratio of each other's strength, think this much less of each
    /// other each month.
    pub rival_ratio: f64,
    pub rival_fall: f64,
    /// A war declared: the realm attacked thinks this much less of the attacker, and the attacker
    /// `declarer_fall` less of it.
    pub attacked_fall: f64,
    pub declarer_fall: f64,
    /// Each battle between two realms: each thinks this much less of the other.
    pub battle_fall: f64,
    /// Each province occupied: its owner thinks this much less of the occupier.
    pub occupied_fall: f64,
    /// Land lost at a peace: the loser thinks this much less of the winner.
    pub land_lost_fall: f64,
    /// A side is worn out at this weariness.
    pub worn: f64,
    /// Weariness for the whole of a side's people living in land its enemy occupies.
    pub occupied_weariness: f64,
    /// Weariness a year at war.
    pub year_weariness: f64,
    /// Weariness a battle lost.
    pub battle_weariness: f64,
    /// Weariness for soldiers lost as many as `war_share` of its people.
    pub dead_weariness: f64,
    pub war_share: f64,
    /// Weariness for the whole realm on the brink of an uprising (average unrest).
    pub unrest_weariness: f64,
    /// The attacker holding the war's target makes peace once the defender's weariness reaches this.
    pub held_target_weariness: f64,
    /// Both sides worn out within this of each other: nobody wins.
    pub tie: f64,
    /// Trade warms opinions: at peace and outside a truce, a realm thinks up to this much better a
    /// month of a realm it trades with, on top of peace's rise and up to the same ceiling. 0 in
    /// scenario files written before.
    #[serde(default)]
    pub trade_opinion: f64,
    /// A month's trade with a realm worth this share of the coins in a realm's own land warms it
    /// the whole `trade_opinion`; less trade, in proportion.
    #[serde(default = "trade_full")]
    pub trade_full: f64,
}

fn trade_full() -> f64 {
    0.01
}

impl Default for TreatyParams {
    fn default() -> Self {
        TreatyParams {
            war_chance: 1.0 / 180.0,
            most_score: 10.0,
            hunger_line: 0.01,
            least_strength: 0.8,
            second_war: 2.0,
            opinion_start: 0.0,
            opinion_rebels: -50.0,
            peace_rise: 0.5,
            peace_ceiling: 50.0,
            rival_ratio: 1.5,
            rival_fall: 0.3,
            attacked_fall: 60.0,
            declarer_fall: 20.0,
            battle_fall: 5.0,
            occupied_fall: 10.0,
            land_lost_fall: 30.0,
            worn: 100.0,
            occupied_weariness: 100.0,
            year_weariness: 10.0,
            battle_weariness: 5.0,
            dead_weariness: 50.0,
            war_share: 0.05,
            unrest_weariness: 30.0,
            held_target_weariness: 50.0,
            tie: 10.0,
            trade_opinion: 0.5,
            trade_full: trade_full(),
        }
    }
}

impl TreatyParams {
    fn check(&self) -> Result<(), String> {
        let all = [
            self.war_chance,
            self.most_score,
            self.hunger_line,
            self.least_strength,
            self.second_war,
            self.peace_rise,
            self.peace_ceiling,
            self.rival_ratio,
            self.rival_fall,
            self.attacked_fall,
            self.declarer_fall,
            self.battle_fall,
            self.occupied_fall,
            self.land_lost_fall,
            self.worn,
            self.occupied_weariness,
            self.year_weariness,
            self.battle_weariness,
            self.dead_weariness,
            self.war_share,
            self.unrest_weariness,
            self.held_target_weariness,
            self.tie,
            self.trade_opinion,
            self.trade_full,
        ];
        if !all.iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err("treaties: every setting but the starting opinions is a number of 0 or more".into());
        }
        let opinion = OPINION as f64;
        if ![self.opinion_start, self.opinion_rebels, self.peace_ceiling].iter().all(|x| x.is_finite() && x.abs() <= opinion) {
            return Err(format!("treaties: opinions are between -{opinion} and {opinion}"));
        }
        if self.war_share <= 0.0 || self.worn <= 0.0 || self.trade_full <= 0.0 || self.rival_ratio < 1.0 {
            return Err("treaties: the war-time share, the worn-out line and trade's full share are above 0, and rivals' ratio is 1 or more".into());
        }
        Ok(())
    }
}

/// Months of hunger kept for each realm.
pub(crate) const HUNGER_MONTHS: usize = 12;

pub struct WarState {
    pub params: WarParams,
    /// Under "treaties": people who starved in each realm's land, month by month for the last
    /// twelve months (realm × [`HUNGER_MONTHS`]), and the month being filled.
    hunger: Vec<u64>,
    month: u8,
    /// Where each pair of realms is kept in the relations, rebuilt when the relations change from
    /// outside (not saved).
    index: BTreeMap<(u32, u32), u32>,
    /// The coins in each realm's own land, worked out once on a day trade is delivered: the day,
    /// and the coins by realm (not saved).
    land_coins: Option<(u64, Vec<u64>)>,
}

impl WarState {
    pub fn new(params: WarParams) -> WarState {
        WarState { params, hunger: Vec::new(), month: 0, index: BTreeMap::new(), land_coins: None }
    }
}

const WAR_FIELDS: [&str; 12] = [
    "war.attacker",
    "war.defender",
    "war.cause",
    "war.began",
    "war.ended",
    "war.winner",
    "war.truce_until",
    "war.target",
    "war.attacker_dead",
    "war.defender_dead",
    "war.attacker_lost",
    "war.defender_lost",
];

static V0: Manifest = Manifest {
    slot: Slot::Diplomacy,
    version: "wars",
    tier: crate::fields::Tier::War,
    cadence: Cadence::Monthly,
    reads: &["realm.capital", "province.occupier"],
    reads_earlier: &[],
    owns: &WAR_FIELDS,
    requests: &[],
    asks: &[],
    moves: &[],
    // Wars are between realms: without them there is nobody to fight.
    requires: &["realm.capital"],
    emits: &[EventKind::WarDeclared, EventKind::Peace, EventKind::WarRefused],
    handles: &[EventKind::DeclareWar, EventKind::ForcePeace, EventKind::Rebellion],
    state_schema: "none",
    goods: &[],
};

static V1: Manifest = Manifest {
    version: "treaties",
    reads: &[
        "realm.capital",
        "province.occupier",
        "province.owner",
        "province.cells",
        "province.unrest",
        "pop.province",
        "pop.size",
        "province.stock",
        "realm.ruler",
        "character.martial",
        "character.greed",
    ],
    owns: &[
        "war.attacker",
        "war.defender",
        "war.cause",
        "war.began",
        "war.ended",
        "war.winner",
        "war.truce_until",
        "war.target",
        "war.attacker_dead",
        "war.defender_dead",
        "war.attacker_lost",
        "war.defender_lost",
        "relation.a",
        "relation.b",
        "relation.a_of_b",
        "relation.b_of_a",
    ],
    // Without armies nothing is occupied and no battle fought: every war would end with nobody
    // winning.
    requires: &["realm.capital", "army.size"],
    handles: &[EventKind::DeclareWar, EventKind::ForcePeace, EventKind::Rebellion, EventKind::Famine, EventKind::Battle, EventKind::Occupied, EventKind::Trade],
    state_schema: "treaties-1",
    ..V0
};

impl Module for WarState {
    fn manifest(&self) -> &'static Manifest {
        if self.params.treaties.is_some() { &V1 } else { &V0 }
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let t = self.params.truce_years;
        if !(t.is_finite() && t >= 0.0) {
            return Err("a truce lasts zero years or more".into());
        }
        let r = self.params.rebel_war_years;
        if !(r.is_finite() && r >= 0.0) {
            return Err("a rebellion's war lasts zero years or more".into());
        }
        self.params.treaties.as_ref().map_or(Ok(()), TreatyParams::check)
    }

    /// Wars with a realm that has been dissolved end, won by the other side; a rebellion's war ends
    /// won by the realm the rebels left once it occupies their capital, and otherwise, after long
    /// enough, with the rebels free. Under "treaties", then: opinions, peace by treaty and new wars.
    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let rebels_free = (self.params.rebel_war_years * cx.calendar.days_per_year() as f64).round() as u64;
        for k in 0..w.wars.count() as u32 {
            if !w.wars.ongoing(k) {
                continue;
            }
            let ku = k as usize;
            let (a, d) = (w.wars.attacker[ku], w.wars.defender[ku]);
            let winner = match (w.realms.alive(a), w.realms.alive(d)) {
                // The realm the rebels left holds their capital: the rebellion is put down.
                (true, true) if WarCause::from_u8(w.wars.cause[ku]) == WarCause::Rebellion && w.occupier.get(w.realms.capital[a as usize] as usize) == Some(&d) => d,
                (true, true) if WarCause::from_u8(w.wars.cause[ku]) == WarCause::Rebellion && cx.day >= w.wars.began[ku].saturating_add(rebels_free) => a,
                (true, true) => continue,
                (true, false) => a,
                (false, true) => d,
                (false, false) => NONE,
            };
            self.end(w, cx, k, winner);
        }
        if let Some(t) = self.params.treaties.clone() {
            let s = Survey::take(w, &self.hunger);
            self.relate(w, &t, &s, cx.day);
            self.treat(w, cx, &t, &s);
            self.choose(w, cx, &t, &s);
            // The next month's hunger starts empty.
            self.month = ((self.month as usize + 1) % HUNGER_MONTHS) as u8;
            let m = self.month as usize;
            for r in 0..self.hunger.len() / HUNGER_MONTHS {
                self.hunger[r * HUNGER_MONTHS + m] = 0;
            }
        }
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        match e {
            &Event::DeclareWar { attacker, defender } => self.declare(w, cx, attacker, defender, WarCause::Poke, NONE),
            // The rebels fight the realm they left, if both are still there.
            &Event::Rebellion { realm, from, .. } if w.realms.alive(realm) && w.realms.alive(from) && w.wars.current(realm, from).is_none() => {
                if let Some(t) = self.params.treaties.clone() {
                    let k = self.relation(w, &t, realm, from);
                    w.relations.set(k, realm, t.opinion_rebels as f32);
                    w.relations.set(k, from, t.opinion_rebels as f32);
                }
                self.declare(w, cx, realm, from, WarCause::Rebellion, NONE)
            }
            &Event::ForcePeace { a, b, winner } => {
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
                    (None, Some(k)) => self.end(w, cx, k, winner),
                }
            }
            &Event::Famine { province, starved } if self.params.treaties.is_some() => {
                let r = w.owner.get(province as usize).copied().unwrap_or(NONE);
                if r != NONE {
                    let want = (r as usize + 1) * HUNGER_MONTHS;
                    if self.hunger.len() < want {
                        self.hunger.resize(want, 0);
                    }
                    self.hunger[r as usize * HUNGER_MONTHS + self.month as usize] += starved;
                }
            }
            Event::Battle { winner, loser, sides, .. } => {
                let Some(t) = self.params.treaties.clone() else { return };
                self.battle(w, &t, *winner, *loser, sides);
            }
            &Event::Occupied { province, by } => {
                let Some(t) = self.params.treaties.clone() else { return };
                let owner = w.owner.get(province as usize).copied().unwrap_or(NONE);
                if owner != NONE && owner != by && w.realms.alive(owner) && w.realms.alive(by) {
                    self.sour(w, &t, owner, by, t.occupied_fall);
                }
            }
            &Event::Trade { a, b, a_to_b, b_to_a } => {
                let Some(t) = self.params.treaties.clone().filter(|t| t.trade_opinion > 0.0) else { return };
                self.traded(w, &t, cx, a, b, a_to_b + b_to_a);
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        if self.params.treaties.is_none() {
            return Vec::new();
        }
        crate::sim::to_bytes(&(&self.hunger, self.month))
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.params.treaties.is_none() {
            return if bytes.is_empty() { Ok(()) } else { Err("Diplomacy \"wars\" keeps no private state".into()) };
        }
        (self.hunger, self.month) = postcard::from_bytes(bytes).map_err(|e| format!("Diplomacy state: {e}"))?;
        self.index.clear();
        self.land_coins = None;
        Ok(())
    }
}

/// What "treaties" works out once a month, from the world as it stands: people, strength, unrest,
/// occupation, hunger, the provinces each realm can reach, and how each pair stands.
struct Survey {
    people: Vec<u64>,
    /// Each realm's people (its strength), and its provinces' unrest summed.
    strength: Vec<u64>,
    unrest: Vec<u64>,
    provinces: Vec<u64>,
    /// People in land of realm `.0` that realm `.1` occupies.
    occupied: BTreeMap<(u32, u32), u64>,
    /// Share of each realm's people who starved over the last twelve months.
    hunger: Vec<f64>,
    /// For each realm, the provinces of each other realm its armies can reach (sorted).
    reach: Vec<BTreeMap<u32, Vec<u32>>>,
    /// Each pair (lower first) at war or under a truce: (at war, first day they may fight again).
    pairs: BTreeMap<(u32, u32), (bool, u64)>,
}

impl Survey {
    fn take(w: &World, hunger: &[u64]) -> Survey {
        let n = w.provinces.count;
        let realms = w.realms.count();
        let people = w.province_population();
        let mut strength = vec![0u64; realms];
        let mut unrest = vec![0u64; realms];
        let mut provinces = vec![0u64; realms];
        let mut occupied = BTreeMap::new();
        for p in 0..n {
            let o = w.owner.get(p).copied().unwrap_or(NONE);
            if o == NONE || w.provinces.cells[p] == 0 {
                continue;
            }
            strength[o as usize] += people[p];
            unrest[o as usize] += w.unrest.get(p).copied().unwrap_or(0);
            provinces[o as usize] += 1;
            let occ = w.occupier.get(p).copied().unwrap_or(NONE);
            if occ != NONE {
                *occupied.entry((o, occ)).or_insert(0) += people[p];
            }
        }
        let hunger = (0..realms)
            .map(|r| {
                let starved: u64 = hunger.get(r * HUNGER_MONTHS..(r + 1) * HUNGER_MONTHS).map_or(0, |m| m.iter().sum());
                let all = strength[r] + starved;
                if all == 0 { 0.0 } else { starved as f64 / all as f64 }
            })
            .collect();
        // A realm stages from land it holds (no enemy occupying it) and land it occupies.
        let stager = |q: usize| {
            let occ = w.occupier.get(q).copied().unwrap_or(NONE);
            if occ != NONE { occ } else { w.owner.get(q).copied().unwrap_or(NONE) }
        };
        let mut reach: Vec<BTreeMap<u32, Vec<u32>>> = vec![BTreeMap::new(); realms];
        for l in &w.routes.links {
            if l.land_days.is_none() && l.sea_days.is_none() {
                continue;
            }
            for (from, to) in [(l.a, l.b), (l.b, l.a)] {
                let (f, q) = (from as usize, to as usize);
                if w.provinces.cells[f] == 0 || w.provinces.cells[q] == 0 {
                    continue;
                }
                let x = stager(f);
                let o = w.owner[q];
                if x == NONE || o == NONE || o == x || w.occupier[q] == x || !w.realms.alive(x) || !w.realms.alive(o) {
                    continue;
                }
                reach[x as usize].entry(o).or_default().push(to);
            }
        }
        for m in &mut reach {
            for v in m.values_mut() {
                v.sort_unstable();
                v.dedup();
            }
        }
        let mut pairs: BTreeMap<(u32, u32), (bool, u64)> = BTreeMap::new();
        let wars = &w.wars;
        for k in 0..wars.count() {
            let (a, b) = (wars.attacker[k], wars.defender[k]);
            let e = pairs.entry((a.min(b), a.max(b))).or_insert((false, 0));
            if wars.ended[k] == NEVER {
                e.0 |= w.realms.alive(a) && w.realms.alive(b);
            } else {
                e.1 = e.1.max(wars.truce_until[k]);
            }
        }
        Survey { people, strength, unrest, provinces, occupied, hunger, reach, pairs }
    }

    /// Whether `a` and `b` are at peace on `day`: not at war, no truce.
    fn at_peace(&self, a: u32, b: u32, day: u64) -> bool {
        self.pairs.get(&(a.min(b), a.max(b))).is_none_or(|&(war, truce)| !war && truce <= day)
    }
}

/// How worn out realm `r`'s side of war `k` is: see [`TreatyParams`].
fn weariness(w: &World, t: &TreatyParams, s: &Survey, k: u32, r: u32, day: u64, days_per_year: f64) -> f64 {
    let ku = k as usize;
    let enemy = w.wars.other(k, r);
    let people = s.strength.get(r as usize).copied().unwrap_or(0) as f64;
    let held = s.occupied.get(&(r, enemy)).copied().unwrap_or(0) as f64;
    let (dead, lost) = w.wars.tally(k, r);
    let years = day.saturating_sub(w.wars.began[ku]) as f64 / days_per_year;
    let provinces = s.provinces.get(r as usize).copied().unwrap_or(0);
    let unrest = if provinces == 0 { 0.0 } else { s.unrest[r as usize] as f64 / provinces as f64 / crate::realms::LINE as f64 };
    let occupied = if people > 0.0 { held / people } else { 1.0 };
    let dead = if people > 0.0 { dead as f64 / (t.war_share * people) } else { 1.0 };
    t.occupied_weariness * occupied.min(1.0) + t.year_weariness * years + t.battle_weariness * lost as f64 + t.dead_weariness * dead + t.unrest_weariness * unrest
}

/// How worn out each side of war `k` is on `day` (attacker, defender), under the settings `t`, as
/// Diplomacy "treaties" works it out at a month's start: for the window and tests.
pub fn war_weariness(w: &World, t: &TreatyParams, k: u32, day: u64, days_per_year: f64) -> (f64, f64) {
    let s = Survey::take(w, &[]);
    let ku = k as usize;
    (weariness(w, t, &s, k, w.wars.attacker[ku], day, days_per_year), weariness(w, t, &s, k, w.wars.defender[ku], day, days_per_year))
}

/// What realm `x` would declare war over, and how much it wants to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pick {
    /// The realm it would declare war on.
    pub realm: u32,
    pub cause: WarCause,
    /// The province it wants.
    pub province: u32,
    /// What it wants of it, times how much stronger it is, its ruler's warlike trait and its
    /// opinion (see [`TreatyParams::war_chance`]).
    pub score: f64,
}

/// What provinces are worth to a realm that wants riches: coins where the world keeps them, people
/// and food stores (a year's rations counted as a person) where it doesn't; and each realm's own
/// provinces' worth on average.
struct Worth {
    coins: bool,
    own: Vec<f64>,
}

impl Worth {
    fn take(w: &World, s: &Survey) -> Worth {
        let coins = w.goods.tree.find("money").and_then(|m| w.goods.columns_under(m)).is_some_and(|c| !c.is_empty());
        let mut worth = Worth { coins, own: Vec::new() };
        let realms = w.realms.count();
        let (mut sum, mut n) = (vec![0.0; realms], vec![0.0; realms]);
        for p in 0..w.provinces.count {
            let o = w.owner[p];
            if o != NONE && w.occupier[p] == NONE && w.provinces.cells[p] > 0 {
                sum[o as usize] += worth.value(w, s, p as u32);
                n[o as usize] += 1.0;
            }
        }
        worth.own = (0..realms).map(|r| if n[r] > 0.0 { (sum[r] / n[r]).max(1.0) } else { 1.0 }).collect();
        worth
    }

    fn value(&self, w: &World, s: &Survey, p: u32) -> f64 {
        let pu = p as usize;
        if self.coins {
            w.stock_under(pu, "money").unwrap_or(0) as f64
        } else {
            s.people[pu] as f64 + w.stock_under(pu, "food").unwrap_or(0) as f64 / 365.0
        }
    }
}

/// The war realm `x` most wants to declare on `day`, if any: on a realm its armies can reach, at
/// peace with it (no truce), not much stronger than it, and, if `x` already fights (its `foes`),
/// only when much stronger than all its enemies together.
fn pick(w: &World, t: &TreatyParams, s: &Survey, worth: &Worth, foes: &[Vec<u32>], day: u64, x: u32) -> Option<Pick> {
    let xu = x as usize;
    if !w.realms.alive(x) || s.strength[xu] == 0 {
        return None;
    }
    let ruler = w.realms.ruler.get(xu).copied().unwrap_or(NONE);
    // A realm without a ruler yet (a breakaway's first weeks) is neither warlike nor cautious.
    let trait_of = |v: &Vec<u8>| if ruler == NONE { 50.0 } else { v.get(ruler as usize).copied().unwrap_or(50) as f64 };
    let (martial, greed) = (trait_of(&w.characters.martial), trait_of(&w.characters.greed));
    let mine = s.strength[xu] as f64;
    let at_war: f64 = foes[xu].iter().map(|&e| s.strength[e as usize] as f64).sum();
    let hungry = s.hunger[xu] >= t.hunger_line;
    let mut best: Option<Pick> = None;
    for (&y, provinces) in &s.reach[xu] {
        let theirs = s.strength[y as usize] as f64;
        if !w.realms.alive(y) || theirs <= 0.0 || !s.at_peace(x, y, day) || foes[xu].contains(&y) {
            continue;
        }
        if mine < t.least_strength * theirs || (at_war > 0.0 && mine < t.second_war * (at_war + theirs)) {
            continue;
        }
        // What it wants of them: their food if hungry, and the richest province it can reach.
        let mut want: Option<(f64, WarCause, u32)> = None;
        if hungry {
            let food = |p: u32| w.stock_under(p as usize, "food").unwrap_or(0);
            let granary = provinces.iter().copied().max_by_key(|&p| (food(p), std::cmp::Reverse(p)));
            if let Some(p) = granary.filter(|&p| food(p) > 0) {
                want = Some(((s.hunger[xu] / t.hunger_line).min(3.0), WarCause::Food, p));
            }
        }
        let rich = provinces.iter().copied().max_by(|&a, &b| worth.value(w, s, a).total_cmp(&worth.value(w, s, b)).then(b.cmp(&a)));
        if let Some(p) = rich {
            let pull = (worth.value(w, s, p) / worth.own[xu] - 1.0).clamp(0.0, 3.0) * (0.5 + greed / 100.0);
            if pull > 0.0 && want.is_none_or(|(v, _, _)| pull > v) {
                want = Some((pull, WarCause::Riches, p));
            }
        }
        let Some((pull, cause, province)) = want else { continue };
        let opinion = w.relations.opinion(x, y) as f64;
        let score = pull * (mine / theirs).min(3.0) * 2f64.powf((martial - 50.0) / 50.0) * (-opinion / 50.0).exp();
        if best.is_none_or(|b| score > b.score) {
            best = Some(Pick { realm: y, cause, province, score });
        }
    }
    best
}

/// [`pick`] for realm `x` in the world as it stands, with `hunger` the starved per realm and
/// month as Diplomacy keeps it (realm × 12): for tests.
#[cfg(test)]
pub(crate) fn pick_for(w: &World, t: &TreatyParams, hunger: &[u64], day: u64, x: u32) -> Option<Pick> {
    let s = Survey::take(w, hunger);
    let worth = Worth::take(w, &s);
    pick(w, t, &s, &worth, &enemies(w), day, x)
}

/// The provinces of each realm that realm `x`'s armies can reach, as Diplomacy sees them: for
/// tests.
#[cfg(test)]
pub(crate) fn reach_of(w: &World, x: u32) -> BTreeMap<u32, Vec<u32>> {
    Survey::take(w, &[]).reach.swap_remove(x as usize)
}

impl WarState {
    /// The relation between `x` and `y`, made (at the starting opinion) if none is kept.
    fn relation(&mut self, w: &mut World, t: &TreatyParams, x: u32, y: u32) -> u32 {
        let key = (x.min(y), x.max(y));
        let rel = &w.relations;
        let fresh = |k: u32| (k as usize) < rel.count() && rel.a[k as usize] == key.0 && rel.b[k as usize] == key.1;
        if let Some(&k) = self.index.get(&key)
            && fresh(k)
        {
            return k;
        }
        // The relations changed from outside (a load, going back, an edit): look again.
        if self.index.len() != rel.count() || self.index.contains_key(&key) {
            self.index = (0..rel.count() as u32).map(|k| ((rel.a[k as usize], rel.b[k as usize]), k)).collect();
            if let Some(&k) = self.index.get(&key) {
                return k;
            }
        }
        let k = w.relations.push(x, y, t.opinion_start as f32);
        self.index.insert(key, k);
        k
    }

    /// `x` thinks `by` less of `y`.
    fn sour(&mut self, w: &mut World, t: &TreatyParams, x: u32, y: u32, by: f64) {
        let k = self.relation(w, t, x, y);
        let v = w.relations.of(k, x) - by as f32;
        w.relations.set(k, x, v);
    }

    /// Opinions over the month: every pair that can reach each other is kept; at peace, outside a
    /// truce, opinions warm up to the ceiling, and rivals cool.
    fn relate(&mut self, w: &mut World, t: &TreatyParams, s: &Survey, day: u64) {
        let mut near: BTreeMap<(u32, u32), ()> = BTreeMap::new();
        for (x, m) in s.reach.iter().enumerate() {
            for &y in m.keys() {
                near.insert(((x as u32).min(y), (x as u32).max(y)), ());
            }
        }
        for &(a, b) in near.keys() {
            self.relation(w, t, a, b);
        }
        let (rise, ceiling, fall) = (t.peace_rise as f32, t.peace_ceiling as f32, t.rival_fall as f32);
        for k in 0..w.relations.count() {
            let (a, b) = (w.relations.a[k], w.relations.b[k]);
            if !(w.realms.alive(a) && w.realms.alive(b)) || !s.at_peace(a, b, day) {
                continue;
            }
            let (sa, sb) = (s.strength[a as usize] as f64, s.strength[b as usize] as f64);
            let rivals = near.contains_key(&(a, b)) && sa > 0.0 && sb > 0.0 && sa.max(sb) <= t.rival_ratio * sa.min(sb);
            for x in [a, b] {
                let mut v = w.relations.of(k as u32, x);
                if v < ceiling {
                    v = (v + rise).min(ceiling);
                }
                if rivals {
                    v -= fall;
                }
                w.relations.set(k as u32, x, v);
            }
        }
    }

    /// Realms `a` and `b` traded goods worth `worth` coins across their border last month: at peace,
    /// outside a truce, each thinks better of the other, by up to `trade_opinion` (for trade worth
    /// `trade_full` of the coins in its own land, and in proportion for less), up to peace's ceiling.
    /// A pair with no relation yet gets one.
    fn traded(&mut self, w: &mut World, t: &TreatyParams, cx: &mut Ctx, a: u32, b: u32, worth: u64) {
        if a == b || !(w.realms.alive(a) && w.realms.alive(b)) || standing(w, a, b, cx.day) != Standing::Peace {
            return;
        }
        if self.land_coins.as_ref().is_none_or(|c| c.0 != cx.day) {
            let mut coins = vec![0u64; w.realms.count()];
            for p in 0..w.provinces.count {
                let o = w.owner.get(p).copied().unwrap_or(NONE);
                if o != NONE && (o as usize) < coins.len() {
                    coins[o as usize] += w.stock_under(p, "money").unwrap_or(0);
                }
            }
            self.land_coins = Some((cx.day, coins));
        }
        let ceiling = t.peace_ceiling as f32;
        let k = self.relation(w, t, a, b);
        for x in [a, b] {
            let own = self.land_coins.as_ref().and_then(|c| c.1.get(x as usize)).copied().unwrap_or(0) as f64;
            let share = if own > 0.0 { (worth as f64 / (t.trade_full * own)).min(1.0) } else { 1.0 };
            let v = w.relations.of(k, x);
            if v < ceiling {
                w.relations.set(k, x, (v + (t.trade_opinion * share) as f32).min(ceiling));
            }
            let capital = w.realms.capital[x as usize];
            if cx.traced(capital) {
                let other = if x == a { b } else { a };
                cx.note(format!("trade with realm {other} worth {worth} coins last month: realm {x} thinks {:.2} better of it", t.trade_opinion * share));
            }
        }
    }

    /// A battle is over: its dead and the loss go to the war between its realms, and they think
    /// less of each other.
    fn battle(&mut self, w: &mut World, t: &TreatyParams, winner: u32, loser: u32, sides: &[(u32, u64)]) {
        for &(x, dead) in sides {
            // Its dead go to its war with the first enemy beside it (a realm fighting two enemies
            // in one battle is rare).
            let war = sides.iter().find_map(|&(y, _)| if y == x { None } else { w.wars.current(x, y) });
            let Some(k) = war else { continue };
            let ku = k as usize;
            if w.wars.attacker[ku] == x {
                w.wars.attacker_dead[ku] += dead;
            } else {
                w.wars.defender_dead[ku] += dead;
            }
        }
        // The side that broke lost it, even when nobody held the field.
        let foe = sides.iter().map(|s| s.0).find(|&y| y != loser && loser != NONE && w.wars.current(loser, y).is_some());
        let foe = if winner != NONE && loser != NONE && w.wars.current(winner, loser).is_some() { Some(winner) } else { foe };
        if let Some(y) = foe
            && let Some(k) = w.wars.current(loser, y)
        {
            let ku = k as usize;
            if w.wars.attacker[ku] == loser {
                w.wars.attacker_lost[ku] += 1;
            } else {
                w.wars.defender_lost[ku] += 1;
            }
        }
        for (i, &(x, _)) in sides.iter().enumerate() {
            for &(y, _) in &sides[i + 1..] {
                if x != y && w.wars.current(x, y).is_some() {
                    self.sour(w, t, x, y, t.battle_fall);
                    self.sour(w, t, y, x, t.battle_fall);
                }
            }
        }
    }

    /// Peace by treaty: wars one side of which is worn out, or whose attacker holds the target and
    /// whose defender is half worn out.
    fn treat(&mut self, w: &mut World, cx: &mut Ctx, t: &TreatyParams, s: &Survey) {
        let dpy = cx.calendar.days_per_year() as f64;
        for k in 0..w.wars.count() as u32 {
            let ku = k as usize;
            let (a, d) = (w.wars.attacker[ku], w.wars.defender[ku]);
            if !w.wars.ongoing(k) || !(w.realms.alive(a) && w.realms.alive(d)) {
                continue;
            }
            let wa = weariness(w, t, s, k, a, cx.day, dpy);
            let wd = weariness(w, t, s, k, d, cx.day, dpy);
            let target = w.wars.target[ku];
            let held = target != NONE && w.occupier.get(target as usize) == Some(&a);
            let winner = if held && wd >= t.held_target_weariness && wa < t.worn {
                a
            } else if wa >= t.worn && wd >= t.worn && (wa - wd).abs() < t.tie {
                NONE
            } else if wa >= t.worn || wd >= t.worn {
                if wa < wd { a } else { d }
            } else {
                continue;
            };
            for r in [a, d] {
                let capital = w.realms.capital[r as usize];
                if cx.traced(capital) {
                    cx.note(format!("war {k}: realm {a} worn out {wa:.0}, realm {d} {wd:.0} (worn out at {:.0}){}", t.worn, if held { ", the target held" } else { "" }));
                }
            }
            self.end(w, cx, k, winner);
        }
    }

    /// Each realm may declare war on the realm it most wants to, with a chance that grows with how
    /// much it wants to.
    fn choose(&mut self, w: &mut World, cx: &mut Ctx, t: &TreatyParams, s: &Survey) {
        let mut foes = enemies(w);
        let worth = Worth::take(w, s);
        let mut declared = Vec::new();
        for x in 0..w.realms.count() as u32 {
            let Some(p) = pick(w, t, s, &worth, &foes, cx.day, x) else { continue };
            let chance = (p.score.min(t.most_score) * t.war_chance).min(1.0);
            if unit(cx.seed, Rng::Diplomacy, cx.day, x as u64, purpose("declare")) >= chance {
                continue;
            }
            foes[x as usize].push(p.realm);
            foes[p.realm as usize].push(x);
            declared.push((x, p));
        }
        for (x, p) in declared {
            self.declare(w, cx, x, p.realm, p.cause, p.province);
        }
    }

    /// `attacker` declares war on `defender` today, for `cause`, over province `target` (or none).
    /// A truce between them is broken: it ends today.
    fn declare(&mut self, w: &mut World, cx: &mut Ctx, attacker: u32, defender: u32, cause: WarCause, target: u32) {
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
        let war = wars.push(attacker, defender, cause, cx.day, target);
        cx.emit(Event::WarDeclared { war, attacker, defender, cause });
        if let Some(t) = self.params.treaties.clone() {
            self.sour(w, &t, defender, attacker, t.attacked_fall);
            self.sour(w, &t, attacker, defender, t.declarer_fall);
        }
        for r in [attacker, defender] {
            let capital = w.realms.capital[r as usize];
            if cx.traced(capital) {
                let over = if target == NONE { String::new() } else { format!(", over province {target}") };
                cx.note(format!("realm {attacker} declared war on realm {defender} (war {war}): {}{over}", cause.describe()));
            }
        }
    }

    /// War `k` ends today: `winner` (or nobody, [`NONE`]) keeps what it occupies of the other's land,
    /// handed over by the realms module when [`Event::Peace`] arrives. A truce follows.
    fn end(&mut self, w: &mut World, cx: &mut Ctx, k: u32, winner: u32) {
        let ku = k as usize;
        let truce = (self.params.truce_years * cx.calendar.days_per_year() as f64).round() as u64;
        w.wars.ended[ku] = cx.day;
        w.wars.winner[ku] = winner;
        w.wars.truce_until[ku] = cx.day.saturating_add(truce);
        let (attacker, defender) = (w.wars.attacker[ku], w.wars.defender[ku]);
        let rebellion = WarCause::from_u8(w.wars.cause[ku]) == WarCause::Rebellion;
        cx.emit(Event::Peace { war: k, attacker, defender, winner, rebellion });
        // The loser of land thinks less of the winner.
        if let Some(t) = self.params.treaties.clone()
            && winner != NONE
        {
            let loser = w.wars.other(k, winner);
            let retaken = rebellion && w.realms.broke_from.get(loser as usize) == Some(&winner);
            let loses = (0..w.provinces.count).any(|p| w.owner[p] == loser && (w.occupier[p] == winner || retaken));
            if loses && w.realms.alive(loser) {
                self.sour(w, &t, loser, winner, t.land_lost_fall);
            }
        }
        for r in [attacker, defender] {
            let capital = w.realms.capital.get(r as usize).copied().unwrap_or(NONE);
            if capital != NONE && cx.traced(capital) {
                let who = if winner == NONE { "nobody won".to_string() } else { format!("realm {winner} won") };
                cx.note(format!("peace between realms {attacker} and {defender} (war {k}): {who}"));
            }
        }
    }
}
