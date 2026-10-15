//! Realms (step 7): who owns each province, and each realm's own values.
//!
//! Realm numbers are never reused. A dissolved realm stays as an entry whose capital is
//! [`NONE`], as a province merged away stays as an empty placeholder, so requests and events in
//! flight, checkpoints, undo and branching all still find what they name.
//!
//! Realms v1, "basic" ([`Founding`]), makes the starting realms once the people are placed, and
//! each month lets realms claim settled land next to them that nobody owns. A world starts in
//! one of three ways, or a mix of them:
//!
//! - **realms over settled land**: about one realm per [`RealmParams::provinces_per_realm`]
//!   provinces, and every settled province owned;
//! - **city-states**: many small realms of one to three settled provinces each;
//! - **a frontier**: realms that hold only the land nearest their capital, with unclaimed land
//!   between them that they claim over time.
//!
//! A mix goes by region, not by realm: a large, smooth pattern over the planet (the same noise
//! the terrain uses) splits the settled land into areas of each way, in the shares asked for, so
//! one coast is city-states while the plains inland are kingdoms.
//!
//! Realms grow out from spread-out starting provinces by travel time along the province ways
//! (by land, and by sea where coasts face each other), so mountains and open water make borders.
//! Settled land a realm can't reach (an island far out) starts a realm of its own. A realm holds
//! empty land (no people) only as far out from its start as its farthest settled province, so
//! deserts and ice beyond the settled land stay nobody's. The capital is its most populous
//! province. Names come from the world's seed and the realm's number alone ([`names`]).

use crate::grid::{dot, NONE, V3};
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::rng::{draw, purpose, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// One entry per realm, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Realms {
    /// The realm's capital province (field `realm.capital`); [`NONE`] for a dissolved realm.
    pub capital: Vec<u32>,
}

impl Realms {
    pub fn count(&self) -> usize {
        self.capital.len()
    }

    /// Whether realm `r` exists and has not been dissolved.
    pub fn alive(&self, r: u32) -> bool {
        self.capital.get(r as usize).is_some_and(|&c| c != NONE)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealmParams {
    /// How the settled land is shared out between the three ways to start, as weights (any
    /// scale; at least one above zero): realms over settled land, city-states, and a frontier.
    pub settled: f64,
    pub city_states: f64,
    pub frontier: f64,
    /// About how many provinces a realm over settled land (or on the frontier) is made from.
    pub provinces_per_realm: f64,
    /// On the frontier, the share of each realm's settled land left unclaimed at the start.
    pub unclaimed_share: f64,
    /// Each month, the chance that a settled province nobody owns is claimed by a realm next to it.
    pub claim_per_month: f64,
}

impl Default for RealmParams {
    fn default() -> Self {
        RealmParams { settled: 1.0, city_states: 0.0, frontier: 0.0, provinces_per_realm: 25.0, unclaimed_share: 0.6, claim_per_month: 0.005 }
    }
}

/// The largest city-state, in settled provinces.
pub const CITY_STATE_MAX: u32 = 3;

/// The three ways to start, as the areas the settled land is split into.
const SETTLED: u8 = 0;
const CITY: u8 = 1;
const FRONTIER: u8 = 2;

pub struct Founding {
    pub params: RealmParams,
}

static V1: Manifest = Manifest {
    slot: Slot::Realms,
    version: "basic",
    tier: crate::fields::Tier::Realm,
    cadence: Cadence::Monthly,
    reads: &["pop.province", "pop.size", "province.cells", "province.center"],
    owns: &["province.owner", "realm.capital"],
    requests: &[],
    asks: &[],
    moves: &[],
    // Realms are made over settled land: without people there is none.
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Founding {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        let weights = [p.settled, p.city_states, p.frontier];
        if weights.iter().any(|x| !(x.is_finite() && *x >= 0.0)) || weights.iter().sum::<f64>() <= 0.0 {
            return Err("the ways to start need weights of zero or more, at least one above zero".into());
        }
        if !(p.provinces_per_realm.is_finite() && p.provinces_per_realm >= 1.0) {
            return Err("a realm is made from at least one province".into());
        }
        if !(0.0..1.0).contains(&p.unclaimed_share) {
            return Err("the unclaimed share goes from 0 up to (not including) 1".into());
        }
        if !(0.0..=1.0).contains(&p.claim_per_month) {
            return Err("the chance to claim goes from 0 to 1".into());
        }
        Ok(())
    }

    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let (owner, capital) = found(w, &self.params, cx.seed);
        w.owner = owner;
        w.realms.capital = capital;
    }

    /// Settled land nobody owns may be claimed by a realm next to it: by the one nearest in days,
    /// as the owners stood at the start of the month (a claim doesn't open the way to another in
    /// the same month).
    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        if w.realms.count() == 0 || self.params.claim_per_month <= 0.0 {
            return;
        }
        let people = w.province_population();
        let before = w.owner.clone();
        for p in 0..w.provinces.count {
            if before[p] != NONE || w.provinces.cells[p] == 0 || people[p] == 0 {
                continue;
            }
            if unit(cx.seed, Rng::Realms, cx.day, p as u64, purpose("claim")) >= self.params.claim_per_month {
                continue;
            }
            let by = w
                .routes
                .of(p as u32)
                .filter_map(|l| {
                    let r = before[l.other(p as u32) as usize];
                    (r != NONE && w.realms.alive(r)).then(|| (l.days().unwrap_or(f32::INFINITY), r))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            if let Some((_, r)) = by {
                w.owner[p] = r;
                if cx.traced(p as u32) {
                    cx.note(format!("unclaimed settled land: claimed by realm {r}, the nearest realm next to it"));
                }
            }
        }
    }
}

/// One realm being made: where it starts, which way it was made in, and how many settled
/// provinces it may take.
struct Start {
    province: u32,
    way: u8,
    quota: u32,
}

/// The starting realms: each province's owner, and each realm's capital.
pub fn found(w: &World, params: &RealmParams, seed: u64) -> (Vec<u32>, Vec<u32>) {
    let n = w.provinces.count;
    let people = w.province_population();
    let settled: Vec<bool> = (0..n).map(|p| w.provinces.cells[p] > 0 && people[p] > 0).collect();
    let way = ways(w, params, seed, &settled);

    let mut starts: Vec<Start> = Vec::new();
    let quota = |way: u8, k: usize| match way {
        CITY => 1 + (draw(seed, Rng::Realms, 0, k as u64, purpose("city-state size")) % CITY_STATE_MAX as u64) as u32,
        _ => u32::MAX,
    };
    for wy in [SETTLED, CITY, FRONTIER] {
        let cands: Vec<u32> = (0..n as u32).filter(|&p| settled[p as usize] && way[p as usize] == wy).collect();
        if cands.is_empty() {
            continue;
        }
        let per = if wy == CITY { (1 + CITY_STATE_MAX) as f64 / 2.0 } else { params.provinces_per_realm };
        let k = ((cands.len() as f64 / per).round() as usize).clamp(1, cands.len());
        for p in spread(w, &cands, k, &people, seed, wy) {
            let q = quota(wy, starts.len());
            starts.push(Start { province: p, way: wy, quota: q });
        }
    }

    let mut owner = vec![NONE; n];
    let mut dist = vec![u64::MAX; n];
    let mut taken: Vec<u32> = Vec::new();
    grow(w, &settled, &way, &starts, 0, &mut owner, &mut dist, &mut taken);
    // Settled land no realm reached starts a realm of its own, the most populous first.
    loop {
        let left = (0..n).filter(|&p| settled[p] && owner[p] == NONE).max_by(|&a, &b| people[a].cmp(&people[b]).then(b.cmp(&a)));
        let Some(p) = left else { break };
        let first = starts.len();
        starts.push(Start { province: p as u32, way: way[p], quota: quota(way[p], first) });
        grow(w, &settled, &way, &starts, first, &mut owner, &mut dist, &mut taken);
    }

    // The frontier: each realm there keeps the settled land nearest its start, the rest unclaimed.
    let mut reach = vec![0u64; starts.len()];
    let mut mine: Vec<Vec<u32>> = vec![Vec::new(); starts.len()];
    for p in 0..n {
        if owner[p] != NONE && settled[p] {
            mine[owner[p] as usize].push(p as u32);
        }
    }
    for (r, list) in mine.iter_mut().enumerate() {
        list.sort_by_key(|&p| (dist[p as usize], p));
        if starts[r].way == FRONTIER {
            let keep = ((list.len() as f64 * (1.0 - params.unclaimed_share)).ceil() as usize).max(1);
            for &p in &list[keep..] {
                owner[p as usize] = NONE;
            }
            list.truncate(keep);
        }
        reach[r] = list.last().map_or(0, |&p| dist[p as usize]);
    }
    // Empty land only as far out as the realm's farthest settled province. What a realm keeps
    // stays in one piece: every province on the way to a kept one is nearer its start.
    for p in 0..n {
        let r = owner[p];
        if r != NONE && !settled[p] && dist[p] > reach[r as usize] {
            owner[p] = NONE;
        }
    }
    let capital = mine
        .iter()
        .map(|list| *list.iter().max_by(|&&a, &&b| people[a as usize].cmp(&people[b as usize]).then(b.cmp(&a))).expect("every realm keeps its start"))
        .collect();
    (owner, capital)
}

/// Which way each province's area starts in. With one way asked for, all of them; with several,
/// a smooth pattern over the planet splits the settled land in the shares asked for.
pub(crate) fn ways(w: &World, params: &RealmParams, seed: u64, settled: &[bool]) -> Vec<u8> {
    let n = w.provinces.count;
    let weights = [params.settled, params.city_states, params.frontier];
    let total: f64 = weights.iter().sum();
    if let Some(only) = (0..3).find(|&k| weights[k] == total) {
        return vec![only as u8; n];
    }
    let key = draw(seed, Rng::Realms, 0, 0, purpose("ways"));
    let value: Vec<f64> = (0..n)
        .map(|p| {
            let c: V3 = w.provinces.center[p];
            crate::terrain::fbm([c[0] * 1.8 + 11.0, c[1] * 1.8, c[2] * 1.8], 3, key)
        })
        .collect();
    let mut sorted: Vec<f64> = (0..n).filter(|&p| settled[p]).map(|p| value[p]).collect();
    sorted.sort_by(f64::total_cmp);
    let cut = |share: f64| sorted.get((share * sorted.len() as f64).round() as usize).copied().unwrap_or(f64::INFINITY);
    let (a, b) = (cut(weights[0] / total), cut((weights[0] + weights[1]) / total));
    value.iter().map(|&v| if v < a { SETTLED } else if v < b { CITY } else { FRONTIER }).collect()
}

/// `k` starting provinces among `cands`, spread out: the most populous first, then each time the
/// one farthest from those chosen, favouring the more populous and shuffled a little.
fn spread(w: &World, cands: &[u32], k: usize, people: &[u64], seed: u64, way: u8) -> Vec<u32> {
    let most = cands.iter().map(|&p| people[p as usize]).max().unwrap_or(1).max(1) as f64;
    let weight: Vec<f64> = cands
        .iter()
        .map(|&p| {
            let jitter = 0.7 + 0.6 * unit(seed, Rng::Realms, way as u64, p as u64, purpose("spread"));
            (people[p as usize] as f64 / most).powf(0.25) * jitter
        })
        .collect();
    let center = |p: u32| w.provinces.center[p as usize];
    let angle = |a: u32, b: u32| dot(center(a), center(b)).clamp(-1.0, 1.0).acos();
    let first = (0..cands.len()).max_by(|&a, &b| people[cands[a] as usize].cmp(&people[cands[b] as usize]).then(b.cmp(&a))).expect("not empty");
    let mut out = vec![cands[first]];
    let mut nearest: Vec<f64> = cands.iter().map(|&p| angle(p, cands[first])).collect();
    while out.len() < k {
        let next = (0..cands.len()).max_by(|&a, &b| (nearest[a] * weight[a]).total_cmp(&(nearest[b] * weight[b])).then(b.cmp(&a))).expect("not empty");
        if nearest[next] <= 0.0 {
            break;
        }
        out.push(cands[next]);
        for (i, &p) in cands.iter().enumerate() {
            nearest[i] = nearest[i].min(angle(p, cands[next]));
        }
    }
    out
}

/// Grow the realms `starts[from..]` out over land nobody owns yet, nearest first by travel time
/// (thousandths of a day), within their own way's area. A city-state stops once it holds its
/// share of settled provinces. Ties go to the lower realm, then the lower province.
#[allow(clippy::too_many_arguments)]
fn grow(w: &World, settled: &[bool], way: &[u8], starts: &[Start], from: usize, owner: &mut [u32], dist: &mut [u64], taken: &mut Vec<u32>) {
    taken.resize(starts.len(), 0);
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    for (r, s) in starts.iter().enumerate().skip(from) {
        heap.push(Reverse((0, r as u32, s.province)));
    }
    while let Some(Reverse((d, r, p))) = heap.pop() {
        let (pu, s) = (p as usize, &starts[r as usize]);
        if owner[pu] != NONE || way[pu] != s.way || taken[r as usize] >= s.quota {
            continue;
        }
        owner[pu] = r;
        dist[pu] = d;
        if settled[pu] {
            taken[r as usize] += 1;
        }
        for l in w.routes.of(p) {
            let q = l.other(p);
            if let (NONE, Some(days)) = (owner[q as usize], l.days()) {
                heap.push(Reverse((d + ((days as f64 * 1000.0).round() as u64).max(1), r, q)));
            }
        }
    }
}

/// Put realms right after an edit changed the map: a province left with no land belongs to
/// nobody, a capital the realm no longer holds moves to its most populous province, and a realm
/// left with no land is dissolved. Nothing to do in a world without realms.
pub fn repair(w: &mut World) {
    if w.realms.count() == 0 {
        return;
    }
    for p in 0..w.provinces.count {
        if w.provinces.cells[p] == 0 {
            w.owner[p] = NONE;
        }
    }
    let people = w.province_population();
    for r in 0..w.realms.count() {
        let c = w.realms.capital[r];
        if c == NONE || w.owner[c as usize] == r as u32 {
            continue;
        }
        let best = (0..w.provinces.count).filter(|&p| w.owner[p] == r as u32).max_by(|&a, &b| people[a].cmp(&people[b]).then(b.cmp(&a)));
        w.realms.capital[r] = best.map_or(NONE, |p| p as u32);
    }
}

const ONSETS: &[&str] = &["", "b", "br", "c", "d", "dr", "f", "g", "gr", "h", "k", "kr", "l", "m", "n", "p", "r", "s", "st", "t", "th", "tr", "v", "z"];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "a", "e", "o", "ae", "ei", "au"];
const CODAS: &[&str] = &["", "", "", "n", "r", "l", "s", "th", "nd", "rn", "m", "sk"];
const ENDINGS: &[&str] = &["", "", "", "ia", "or", "heim", "mark", "land", "ar", "en", "os", "grad", "ira", "ane", "ton", "wyn", "esh"];

/// Realm names, the first `count` realms', from the world's seed alone: two or three syllables,
/// sometimes with an ending ("Dreinmark", "Tholia"). No two are the same.
pub fn names(seed: u64, count: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(count);
    for r in 0..count as u64 {
        for attempt in 0.. {
            let mut k = 0u64;
            let mut pick = |list: &[&'static str]| {
                k += 1;
                list[(draw(seed, Rng::Realms, attempt, r, purpose("name") ^ k) % list.len() as u64) as usize]
            };
            let syllables = 2 + (pick(&["", "", "x"]) == "x") as usize;
            let mut name = String::new();
            for s in 0..syllables {
                // Only the first syllable may start with a vowel, so vowels don't pile up.
                name.push_str(pick(if s == 0 { ONSETS } else { &ONSETS[1..] }));
                name.push_str(pick(VOWELS));
                if s + 1 == syllables {
                    name.push_str(pick(CODAS));
                }
            }
            // An ending goes on only where a vowel meets a consonant ("Tholia", not "Tholaia").
            let ending = pick(ENDINGS);
            let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c));
            if vowel(name.chars().last()) != vowel(ending.chars().next()) {
                name.push_str(ending);
            }
            if name.len() >= 4 && seen.insert(name.clone()) {
                let mut cs = name.chars();
                let first = cs.next().expect("not empty").to_ascii_uppercase();
                out.push(std::iter::once(first).chain(cs).collect());
                break;
            }
        }
    }
    out
}
