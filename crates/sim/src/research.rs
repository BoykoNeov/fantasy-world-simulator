//! Research v1, "tree" (build step 9): realms study techs and learn them.
//!
//! The techs and what each needs are data (`data/research.toml`); a world keeps its own copy in its
//! configuration. `realm.tech` has one column per tech, in the order of the techs' names (A to Z:
//! [`tech_names`]), holding the day the realm learned it, or none. `realm.studying` is the tech a
//! realm studies now, and `realm.knowledge` the study banked toward it, in thousandths of a year.
//!
//! Once a month each living realm studies: a middle-sized realm (by people, against the median of
//! the living realms) with an ordinary ruler banks [`ResearchParams::speed`] years of study a year.
//! A bigger realm banks more, but less than in proportion (people to the power `size_power`, at most
//! `size_cap` times as much); an able ruler more, an inept one less (`ruler_effect`). A tech costs
//! its `years`, less `neighbour_discount` times the share of the realm's neighbours (and, counted
//! `trade_weight` each, its trade partners of last month: [`Event::Trade`]) that know it, as they
//! stood when the month began. Once the study banked reaches the cost the realm learns the tech
//! ([`Event::Discovery`]), keeps what is left over, and chooses its next tech at random among those
//! whose needs it knows, the cheapest most often.
//!
//! Under an economy with coins, the realm's scholars (`scholar_share` of its people) are paid
//! `wage` coins a year from its treasury, spread over its provinces by their people (not those an
//! enemy occupies or that have risen up), asked of the keeper of the coins at low priority. The
//! month's study is credited when the wages come, the next day, in the share they were paid:
//! scholars unpaid don't study. Each wage request carries the study its scholars do in its tag, so
//! nothing is kept between the asking and the paying. Without coins, the people study for nothing,
//! at once.
//!
//! New realms: rebels (`realm.broke_from`) know everything the realm they left knew, on the day it
//! learned it; a realm made by the editor knows what the realm that held most of its land knew. A
//! realm that takes land in war learns nothing from it. A dissolved realm keeps its row and studies
//! nothing.
//!
//! This wave Research only produces knowledge; what techs do is wired up later.

use crate::calendar::Calendar;
use crate::config::ModuleConfig;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, Goods, Tier};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Flows, Manifest, Module, Slot};
use crate::requests::{split_proportional, Cause, Dest, Priority};
use crate::rng::{draw, purpose, round_stochastic, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The parameters shipped in `data/research.toml`.
pub const STANDARD: &str = include_str!("../data/research.toml");

/// One tech of the tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TechParams {
    /// Its field of knowledge, one of [`ResearchParams::fields`].
    pub field: String,
    /// Years of study it costs a middle-sized realm with an ordinary ruler, before any discount.
    pub years: f64,
    /// The techs a realm must know before it studies this one.
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchParams {
    /// Years of a tech's cost a middle-sized realm with an ordinary ruler studies in a year.
    pub speed: f64,
    /// A realm studies as fast as (its people ÷ the middle realm's) to this power...
    pub size_power: f64,
    /// ...at most this many times as fast as the middle realm.
    pub size_cap: f64,
    /// A ruler of ability 100 studies this share faster, one of 0 this share slower.
    pub ruler_effect: f64,
    /// A tech every neighbour and trade partner knows costs this share less.
    pub neighbour_discount: f64,
    /// A trade partner counts this much against a neighbour's 1 in the share that knows a tech.
    pub trade_weight: f64,
    /// Under coins: the share of people who are scholars, and the coins each is paid a year.
    pub scholar_share: f64,
    pub wage: f64,
    /// The fields of knowledge.
    pub fields: Vec<String>,
    /// Each tech, by name.
    pub techs: BTreeMap<String, TechParams>,
}

impl Default for ResearchParams {
    fn default() -> Self {
        toml::from_str(STANDARD).expect("bundled research parameters are valid")
    }
}

impl ResearchParams {
    /// Check the numbers and the tree: every need is a tech, no tech needs itself however far
    /// round, every field is listed. Returns each tech's needs as columns.
    pub fn resolve(&self) -> Result<Vec<Vec<usize>>, String> {
        let in_range = |x: f64, lo: f64, hi: f64| x.is_finite() && x >= lo && x <= hi;
        if !in_range(self.speed, 0.0, 1000.0) {
            return Err("study speed goes from 0 to 1000".into());
        }
        if !in_range(self.size_power, 0.0, 10.0) {
            return Err("the size power goes from 0 to 10".into());
        }
        if !in_range(self.size_cap, 1.0, 1e6) {
            return Err("a big realm studies at most from 1 to a million times as fast".into());
        }
        if !in_range(self.ruler_effect, 0.0, 1.0) {
            return Err("a ruler's effect on study goes from 0 to 1".into());
        }
        if !in_range(self.neighbour_discount, 0.0, 0.99) {
            return Err("the neighbours' discount goes from 0 to 0.99: a tech always costs something".into());
        }
        if !in_range(self.trade_weight, 0.0, 1000.0) {
            return Err("a trade partner's weight goes from 0 to 1000".into());
        }
        if !in_range(self.scholar_share, 0.0, 1.0) {
            return Err("the scholars' share goes from 0 to 1".into());
        }
        if !in_range(self.wage, 0.0, 1e6) {
            return Err("a scholar's wage goes from 0 to a million coins".into());
        }
        if self.techs.len() > u16::MAX as usize {
            return Err("too many techs".into());
        }
        let names: Vec<&String> = self.techs.keys().collect();
        let col = |n: &str| names.iter().position(|m| m.as_str() == n);
        let mut needs = Vec::with_capacity(names.len());
        for (name, t) in &self.techs {
            if !self.fields.contains(&t.field) {
                return Err(format!("{name}: {} is not one of the fields of knowledge", t.field));
            }
            if !(t.years.is_finite() && t.years >= 0.001 && t.years <= 1e6) {
                return Err(format!("{name}: a tech costs from 0.001 to a million years of study"));
            }
            let mut of = Vec::new();
            for n in &t.needs {
                of.push(col(n).ok_or_else(|| format!("{name} needs {n}, which is not a tech"))?);
            }
            of.sort_unstable();
            of.dedup();
            needs.push(of);
        }
        // No loops: every tech can be reached from those that need nothing.
        let mut done = vec![false; needs.len()];
        loop {
            let ready: Vec<usize> = (0..needs.len()).filter(|&t| !done[t] && needs[t].iter().all(|&n| done[n])).collect();
            if ready.is_empty() {
                break;
            }
            ready.into_iter().for_each(|t| done[t] = true);
        }
        if let Some(t) = done.iter().position(|d| !d) {
            return Err(format!("{} needs itself, however far round: no realm could ever study it", names[t]));
        }
        Ok(needs)
    }
}

/// The techs of the research tree, in the order of `realm.tech`'s columns (see
/// [`crate::world::World::techs`]): by name, A to Z; none without Research.
pub fn tech_names(config: &ModuleConfig) -> Vec<String> {
    config.research.as_ref().map_or(Vec::new(), |p| p.techs.keys().cloned().collect())
}

/// What the module keeps between days.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct State {
    /// Realms that exist and have been given their knowledge: those after it are new.
    seen: u32,
    /// Pairs of realms (lower first) that traded across their border since the month began.
    partners: BTreeSet<(u32, u32)>,
    /// Each province's owner as last seen (after the module's last run, event or edit), so a realm
    /// the editor makes knows who held its land before.
    owners: Vec<u32>,
}

pub struct Research {
    pub params: ResearchParams,
    /// Whether the world keeps coins (scholars are then paid from the treasury).
    pub coins: bool,
    /// Each tech's needs, as columns, and its cost in years; worked out from the parameters.
    needs: Vec<Vec<usize>>,
    years: Vec<f64>,
    st: State,
}

impl Research {
    pub fn new(params: ResearchParams, coins: bool) -> Research {
        // A tree that doesn't check is refused by `check` before any day runs.
        let needs = params.resolve().unwrap_or_default();
        let years = params.techs.values().map(|t| t.years).collect();
        Research { params, coins, needs, years, st: State::default() }
    }

    fn techs(&self) -> usize {
        self.needs.len()
    }

    /// The pairs of realms heard to trade since the month began.
    #[cfg(test)]
    pub(crate) fn partners(&self) -> &BTreeSet<(u32, u32)> {
        &self.st.partners
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Research,
    version: "tree",
    tier: Tier::Realm,
    cadence: Cadence::Monthly,
    reads: &[
        "pop.province",
        "pop.size",
        "province.owner",
        "province.occupier",
        "province.uprising",
        "realm.capital",
        "realm.broke_from",
        "realm.ruler",
        "character.ability",
    ],
    reads_earlier: &[],
    owns: &["realm.knowledge", "realm.studying", "realm.tech"],
    requests: &[],
    asks: &[],
    moves: &[],
    // Knowledge belongs to realms and grows with their people.
    requires: &["realm.capital", "pop.size"],
    emits: &[EventKind::Discovery],
    handles: &[EventKind::Trade, EventKind::Peace, EventKind::Rebellion],
    state_schema: "seen-partners-owners-1",
    goods: &[],
};

/// The same version where the world keeps coins: scholars' wages are moved from the treasuries
/// into the provinces.
static V1_COINS: Manifest = Manifest { requests: &["realm.stock"], asks: &["money"], moves: &[("realm.stock", "province.stock")], ..V1 };

/// Everything about the realms a month's study works from, as the month began.
pub(crate) struct Survey {
    /// People of each realm, and the provinces it pays scholars in with their people.
    pub(crate) people: Vec<u64>,
    pub(crate) paid_in: Vec<Vec<(u32, u64)>>,
    /// People of the middle living realm.
    pub(crate) median: f64,
    /// Each realm's neighbours, and its trade partners that aren't neighbours.
    pub(crate) neighbours: Vec<Vec<u32>>,
    pub(crate) partners: Vec<Vec<u32>>,
    /// `realm.tech` as the month began.
    pub(crate) known: Vec<u32>,
}

impl Research {
    pub(crate) fn survey(&self, w: &World) -> Survey {
        let n = w.realms.count();
        let pop = w.province_population();
        let mut people = vec![0u64; n];
        let mut paid_in: Vec<Vec<(u32, u64)>> = vec![Vec::new(); n];
        let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); n];
        for p in 0..w.provinces.count {
            let r = w.owner[p];
            if r == NONE || !w.realms.alive(r) {
                continue;
            }
            people[r as usize] += pop[p];
            let occupied = w.occupier.get(p).is_some_and(|&o| o != NONE);
            let risen = w.uprising.get(p).is_some_and(|&s| s != NONE);
            if pop[p] > 0 && !occupied && !risen {
                paid_in[r as usize].push((p as u32, pop[p]));
            }
            for l in w.routes.of(p as u32) {
                let s = w.owner[l.other(p as u32) as usize];
                if s != NONE && s != r && w.realms.alive(s) {
                    neighbours[r as usize].push(s);
                }
            }
        }
        for v in &mut neighbours {
            v.sort_unstable();
            v.dedup();
        }
        let mut partners: Vec<Vec<u32>> = vec![Vec::new(); n];
        for &(a, b) in &self.st.partners {
            if (a as usize) < n && (b as usize) < n && w.realms.alive(a) && w.realms.alive(b) {
                partners[a as usize].push(b);
                partners[b as usize].push(a);
            }
        }
        for (r, v) in partners.iter_mut().enumerate() {
            v.retain(|s| neighbours[r].binary_search(s).is_err());
            v.sort_unstable();
            v.dedup();
        }
        let mut sizes: Vec<u64> = (0..n).filter(|&r| w.realms.alive(r as u32) && people[r] > 0).map(|r| people[r]).collect();
        sizes.sort_unstable();
        let median = match sizes.len() {
            0 => 0.0,
            m if m % 2 == 1 => sizes[m / 2] as f64,
            m => (sizes[m / 2 - 1] + sizes[m / 2]) as f64 / 2.0,
        };
        Survey { people, paid_in, median, neighbours, partners, known: w.realm_tech.clone() }
    }

    /// Years of study a year realm `r` does, as it stands.
    pub(crate) fn rate(&self, w: &World, s: &Survey, r: usize) -> f64 {
        let p = &self.params;
        if s.median <= 0.0 || s.people[r] == 0 {
            return 0.0;
        }
        let size = (s.people[r] as f64 / s.median).powf(p.size_power).min(p.size_cap);
        let ruler = w.realms.ruler.get(r).copied().unwrap_or(NONE);
        let ability = if ruler != NONE && w.characters.alive(ruler) { w.characters.ability.get(ruler as usize).copied().unwrap_or(50) as f64 } else { 50.0 };
        let ruler = (1.0 + p.ruler_effect * (ability - 50.0) / 50.0).max(0.0);
        p.speed * size * ruler
    }

    /// What tech `t` costs realm `r`, in thousandths of a year, after what its neighbours and
    /// partners knew when the month began.
    pub(crate) fn cost(&self, s: &Survey, r: usize, t: usize) -> u64 {
        let k = self.techs();
        let knows = |q: u32| s.known.get(q as usize * k + t).is_some_and(|&d| d != NONE);
        let tw = self.params.trade_weight;
        let (nb, pt) = (&s.neighbours[r], &s.partners[r]);
        let weight = nb.len() as f64 + tw * pt.len() as f64;
        let share = if weight > 0.0 { (nb.iter().filter(|&&q| knows(q)).count() as f64 + tw * pt.iter().filter(|&&q| knows(q)).count() as f64) / weight } else { 0.0 };
        ((self.years[t] * (1.0 - self.params.neighbour_discount * share) * 1000.0).round() as u64).max(1)
    }

    /// Choose realm `r`'s next tech among those whose needs it knows, the cheapest most often; none
    /// when it can study nothing more.
    fn choose(&self, w: &World, s: &Survey, r: usize, seed: u64, day: u64, attempt: u64) -> u32 {
        let k = self.techs();
        let row = &w.realm_tech[r * k..(r + 1) * k];
        let open: Vec<(usize, f64)> =
            (0..k).filter(|&t| row[t] == NONE && self.needs[t].iter().all(|&n| row[n] != NONE)).map(|t| (t, 1.0 / self.cost(s, r, t) as f64)).collect();
        let total: f64 = open.iter().map(|o| o.1).sum();
        if open.is_empty() {
            return NONE;
        }
        let u = (draw(seed, Rng::Research, day, r as u64 | attempt << 32, purpose("study")) >> 11) as f64 / (1u64 << 53) as f64 * total;
        let mut acc = 0.0;
        for &(t, x) in &open {
            acc += x;
            if u < acc {
                return t as u32;
            }
        }
        open.last().expect("not empty").0 as u32
    }

    /// Learn whatever the study banked pays for, and choose what to study next.
    fn settle(&self, w: &mut World, s: &Survey, cx: &mut Ctx, r: usize) {
        let k = self.techs();
        for attempt in 0..=k as u64 {
            let t = w.realm_studying[r];
            if t == NONE || w.realm_tech[r * k + t as usize] != NONE {
                w.realm_studying[r] = self.choose(w, s, r, cx.seed, cx.day, attempt);
                if w.realm_studying[r] == NONE {
                    // Everything there is to know is known.
                    w.realm_knowledge[r] = 0;
                    return;
                }
                continue;
            }
            let cost = self.cost(s, r, t as usize);
            if w.realm_knowledge[r] < cost {
                return;
            }
            w.realm_knowledge[r] -= cost;
            w.realm_tech[r * k + t as usize] = cx.day as u32;
            w.realm_studying[r] = NONE;
            let capital = w.realms.capital[r];
            cx.emit(Event::Discovery { realm: r as u32, tech: t as u16, capital });
            if cx.traced(capital) {
                cx.note(format!("realm {r} learned {} ({cost} thousandths of a year of study)", w.techs[t as usize]));
            }
        }
    }

    /// Give realms made since the last look the knowledge they inherit: rebels everything the realm
    /// they left knew; a realm the editor made what the realm that held most of its land knew (as
    /// last seen).
    fn adopt(&mut self, w: &mut World) {
        let (n, k) = (w.realms.count(), self.techs());
        if (self.st.seen as usize) < n && k > 0 {
            for r in self.st.seen as usize..n {
                let from = match w.realms.broke_from.get(r).copied().unwrap_or(NONE) {
                    NONE => self.held_most(w, r as u32),
                    f => f,
                };
                if from == NONE || from as usize >= n || from as usize == r {
                    continue;
                }
                let f = from as usize;
                for t in 0..k {
                    if w.realm_tech[r * k + t] == NONE {
                        w.realm_tech[r * k + t] = w.realm_tech[f * k + t];
                    }
                }
            }
        }
        self.st.seen = self.st.seen.max(n as u32);
    }

    /// The realm that held most of realm `r`'s land, by provinces, as last seen.
    fn held_most(&self, w: &World, r: u32) -> u32 {
        let mut count: BTreeMap<u32, usize> = BTreeMap::new();
        for p in 0..w.provinces.count {
            if w.owner[p] == r {
                let before = self.st.owners.get(p).copied().unwrap_or(NONE);
                if before != NONE && before != r {
                    *count.entry(before).or_default() += 1;
                }
            }
        }
        // The most provinces; the lower number between equals.
        count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(NONE, |c| c.0)
    }

    fn remember_owners(&mut self, w: &World) {
        self.st.owners.clone_from(&w.owner);
    }

    /// Ask for the month's wages of realm `r`'s scholars; `study` (thousandths of a year) is
    /// credited as they are paid. Returns false when no wage is due (then they study for nothing).
    fn pay(&self, s: &Survey, cx: &mut Ctx, r: usize, study: u64) -> bool {
        let months = cx.calendar.months_per_year() as f64;
        let per_person = self.params.scholar_share * self.params.wage / months;
        let (seed, day) = (cx.seed, cx.day);
        let places = &s.paid_in[r];
        let wages: Vec<u64> = places.iter().map(|&(p, people)| round_stochastic(people as f64 * per_person, seed, Rng::Research, day, p as u64, purpose("wage"))).collect();
        if wages.iter().all(|&x| x == 0) {
            return false;
        }
        let ids: Vec<u64> = places.iter().map(|x| x.0 as u64).collect();
        let shares = split_proportional(study, &wages, &ids);
        let money = cx.goods.tree.find("money").expect("checked when the world was made");
        for (k, &(p, _)) in places.iter().enumerate() {
            if wages[k] > 0 {
                let to = Dest { field: known("province.stock"), entity: p };
                cx.ask_move(known("realm.stock"), r as u32, Some(money), wages[k], to, Priority::Low, Cause::Scholars, shares[k]);
                if cx.traced(p) {
                    cx.note(format!("{} coins of wages for realm {r}'s scholars here asked from its treasury, for {} thousandths of a year of study", wages[k], shares[k]));
                }
            }
        }
        true
    }
}

impl Module for Research {
    fn manifest(&self) -> &'static Manifest {
        if self.coins { &V1_COINS } else { &V1 }
    }

    fn check(&self, goods: &Goods) -> Result<(), String> {
        self.params.resolve()?;
        if self.coins {
            let kept = goods.tree.find("money").and_then(|n| goods.columns_under(n)).is_some_and(|c| !c.is_empty());
            if !kept {
                return Err("research: scholars are paid in money, which this world doesn't keep".into());
            }
        }
        Ok(())
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let (n, k) = (w.realms.count(), self.techs());
        w.realm_knowledge = vec![0; n];
        w.realm_studying = vec![NONE; n];
        w.realm_tech = vec![NONE; n * k];
        self.st = State { seen: n as u32, partners: BTreeSet::new(), owners: w.owner.clone() };
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        self.adopt(w);
        let (n, k) = (w.realms.count(), self.techs());
        if n > 0 && k > 0 {
            let s = self.survey(w);
            let months = cx.calendar.months_per_year() as f64;
            for r in 0..n {
                if !w.realms.alive(r as u32) {
                    w.realm_studying[r] = NONE;
                    continue;
                }
                let study = (self.rate(w, &s, r) * 1000.0 / months).round() as u64;
                if self.coins {
                    // What the last wages paid for is learned now; this month's is paid tomorrow.
                    self.settle(w, &s, cx, r);
                    if w.realm_studying[r] != NONE && study > 0 && !self.pay(&s, cx, r, study) {
                        w.realm_knowledge[r] += study;
                    }
                } else {
                    w.realm_knowledge[r] += study;
                    self.settle(w, &s, cx, r);
                }
                let capital = w.realms.capital[r];
                if cx.traced(capital) {
                    let what = w.realm_studying[r];
                    let what = if what == NONE { "nothing".to_string() } else { w.techs[what as usize].clone() };
                    cx.note(format!("realm {r} studies {what}: {study} thousandths of a year this month, {} banked", w.realm_knowledge[r]));
                }
            }
        }
        self.st.partners.clear();
        self.remember_owners(w);
    }

    fn handle(&mut self, w: &mut World, e: &Event, _cx: &mut Ctx) {
        match *e {
            Event::Granted { request: r, granted } if r.cause == Cause::Scholars => {
                let realm = r.entity as usize;
                if w.realms.alive(r.entity) && r.take > 0 && realm < w.realm_knowledge.len() {
                    w.realm_knowledge[realm] += (r.tag as u128 * granted as u128 / r.take as u128) as u64;
                }
            }
            // Kept whatever the trade weight, so a weight raised mid-month counts this month's
            // partners at once; the weight is used only in what a tech costs.
            Event::Trade { a, b, .. } => {
                self.st.partners.insert((a.min(b), a.max(b)));
            }
            // Land changed hands, or rebels took theirs.
            Event::Peace { .. } | Event::Rebellion { .. } => {
                self.adopt(w);
                self.remember_owners(w);
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.st)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.st = postcard::from_bytes(bytes).map_err(|e| format!("Research state: {e}"))?;
        Ok(())
    }

    fn check_edit(&self, w: &World, edit: &Edit) -> Option<Result<(), String>> {
        let Edit::SetTech { realm, ref tech, known } = *edit else { return None };
        let k = self.techs();
        Some(match w.techs.iter().position(|t| t == tech) {
            None => Err(format!("the research tree has no tech {tech}")),
            Some(t) => {
                let knows = w.realm_tech.get(realm as usize * k + t).is_some_and(|&d| d != NONE);
                match (knows, known) {
                    (true, true) => Err(format!("realm {realm} already knows {tech}")),
                    (false, false) => Err(format!("realm {realm} doesn't know {tech}")),
                    _ => Ok(()),
                }
            }
        })
    }

    /// A tech learned (on the edit's day) or forgotten. Techs that build on one forgotten stay
    /// known; one learned while being studied is studied no more, and the study banked goes to the
    /// next.
    fn carry_out(&mut self, w: &mut World, edit: &Edit, day: u64, _calendar: &Calendar) -> Flows {
        if let Edit::SetTech { realm, ref tech, known } = *edit {
            let k = self.techs();
            let t = w.techs.iter().position(|x| x == tech).expect("checked");
            let r = realm as usize;
            w.realm_tech[r * k + t] = if known { day as u32 } else { NONE };
            if known && w.realm_studying[r] == t as u32 {
                w.realm_studying[r] = NONE;
            }
        }
        Flows::default()
    }

    fn edited(&mut self, w: &mut World, _edit: &Edit) {
        self.adopt(w);
        self.remember_owners(w);
    }
}
