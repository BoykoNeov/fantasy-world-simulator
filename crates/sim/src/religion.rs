//! Religion "conversion" (build step 9): faiths spread over regions, and people take up the faith
//! that pulls hardest on them. Built on the pattern of Culture "assimilation" ([`crate::culture`]).
//!
//! **Starting faiths.** When the world is made, [`ReligionParams::starting`] faiths are spread over
//! the settled land by region, as cultures are: starting provinces spread out (the most populous
//! first, then each time the farthest from those chosen), and each faith grows out from its start
//! along the province ways, nearest first by travel days. Faiths are fewer and bigger than cultures
//! and drawn from their own random stream, so they cross realm and culture borders. Where two faiths
//! meet, a share of each border province's people ([`ReligionParams::borderland`]) is of the
//! neighbouring faith: mixed borderlands. Everyone starts as faith 0 (the pops' first value), so the
//! people are given their faith by shift requests to the pops' owner, made while the world is made
//! and applied before its first day ([`Cause::Placed`]).
//!
//! **Each month** every faith pulls on the people of each province: by its share of the province's
//! own people ([`ReligionParams::own`]); by its share of the people next door
//! ([`ReligionParams::neighbours`], each neighbour counted less the more travel days away it is); by
//! being the faith of the realm that holds the province ([`ReligionParams::ruler`]); and by being
//! the "kin" faith of the province ([`ReligionParams::kin`]): the faith most of the people of the
//! province's main culture hold across the whole world. The kin pull counts only where the world
//! has more than one culture with people, so with Culture off there is none (with one culture,
//! the kin faith would be the world's biggest faith, pulling everyone toward it). The faith that
//! pulls hardest leads; people of every other faith there take it up, a share a month in proportion
//! to how much harder the leader pulls ([`ReligionParams::per_year`]). Small groups join the leader
//! at once, and so do the groups beyond the [`ReligionParams::most`] biggest faiths of a province
//! (the leader counted among them), so a province holds a few faiths, not dozens. A realm's faith
//! (`realm.religion`) is its capital's main faith. With realms off, there is no ruler's pull.
//!
//! Culture pulls on faith (the kin pull), not faith on culture. Religion reads nothing Culture owns
//! (never `realm.culture`): the kin faith is worked out from the pops.
//!
//! **Schisms** ([`SchismParams`], a part that can be switched off): once a year, the provinces where
//! a faith is the main one are put in groups that hang together along the ways. Its biggest group
//! (by people) is the faith's heartland; a group of [`SchismParams::provinces`] or more provinces
//! cut off from it for [`SchismParams::years`] may break away as a new faith, a chance a year,
//! remembering its parent ([`Cause::Schism`]).
//!
//! Events: [`Event::Converted`] when a province's main faith changed (seen at the next month's
//! start), [`Event::ReligionBorn`] when a faith breaks away, [`Event::ReligionGone`] when the last
//! follower of a faith is gone. Names come from the world's seed and the faith's number ([`names`]).
//!
//! Nobody is made or lost: people only change faith, so no flow is booked.

use crate::calendar::Calendar;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, Tier};
use crate::grid::{dot, NONE};
use crate::module::{Cadence, Ctx, Flows, Manifest, Module, Slot};
use crate::pops::Pops;
use crate::requests::{Cause, Priority};
use crate::rng::{draw, purpose, round_stochastic, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// The most faiths there can be: a pop's faith is a 16-bit number.
pub const MOST: usize = u16::MAX as usize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReligionParams {
    /// About how many faiths the world starts with (at most one per settled province).
    pub starting: u32,
    /// Share of a border province's people who are of the neighbouring faith at the start.
    pub borderland: f64,
    /// Share of a faith's people in a province who take up the leading faith in a year, when the
    /// leader pulls on them as hard as it can (and in proportion for less).
    pub per_year: f64,
    /// How hard a faith pulls on a province's people for being all of them (and in proportion for
    /// a share): how people hold to their faith.
    pub own: f64,
    /// How hard a faith pulls for being all of the people next door, travel days fading it.
    pub neighbours: f64,
    /// A neighbour's pull halves with every this many days of travel to it.
    pub neighbour_half_days: f64,
    /// How hard the faith of the realm holding a province pulls on its people.
    pub ruler: f64,
    /// How hard the faith most of the province's main culture holds across the world pulls on its
    /// people (only where the world has more than one culture with people).
    pub kin: f64,
    /// A group of fewer people than this takes up the leading faith at once.
    pub small_people: u64,
    /// A group less than this share of its province's people takes up the leading faith at once.
    pub small_share: f64,
    /// The most faiths a province keeps, the leading one counted: the groups of smaller faiths
    /// take up the leading faith at once.
    pub most: u32,
    /// Share of the people who are priests: only followed people's jobs read it.
    pub priests: f64,
    /// New faiths from groups cut off from their own; none for no schisms. A missing key in a file
    /// means none (not the default), so switching it off survives a settings change.
    #[serde(default)]
    pub schisms: Option<SchismParams>,
}

impl Default for ReligionParams {
    fn default() -> Self {
        ReligionParams {
            starting: 8,
            borderland: 0.10,
            per_year: 0.02,
            own: 1.0,
            neighbours: 0.5,
            neighbour_half_days: 5.0,
            ruler: 1.25,
            kin: 0.5,
            small_people: 100,
            small_share: 0.01,
            most: 3,
            priests: 0.01,
            schisms: Some(SchismParams::default()),
        }
    }
}

/// When a group cut off from its faith breaks away as a faith of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchismParams {
    /// Years a group must have been cut off before it may break away.
    pub years: f64,
    /// The fewest provinces a group that breaks away holds.
    pub provinces: u32,
    /// Each year, the chance that a group cut off long enough breaks away.
    pub per_year: f64,
}

impl Default for SchismParams {
    fn default() -> Self {
        SchismParams { years: 80.0, provinces: 3, per_year: 0.03 }
    }
}

/// One entry per faith, as parallel arrays. Numbers are never reused: a faith with nobody left
/// keeps its entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Religions {
    /// The faith it broke away from, [`crate::grid::NONE`] for a starting faith (field
    /// `religion.parent`).
    pub parent: Vec<u32>,
    /// The day it began (field `religion.born`).
    pub born: Vec<u64>,
    /// The province where it began (field `religion.hearth`).
    pub hearth: Vec<u32>,
    /// The day its last follower was gone, `u64::MAX` while it lives (field `religion.died`).
    pub died: Vec<u64>,
}

impl Religions {
    pub fn count(&self) -> usize {
        self.parent.len()
    }

    /// Whether faith `f` exists and has followers (as of its module's last look).
    pub fn alive(&self, f: u32) -> bool {
        self.died.get(f as usize).is_some_and(|&d| d == u64::MAX)
    }

    fn push(&mut self, parent: u32, born: u64, hearth: u32) -> u32 {
        self.parent.push(parent);
        self.born.push(born);
        self.hearth.push(hearth);
        self.died.push(u64::MAX);
        (self.parent.len() - 1) as u32
    }
}

pub struct Conversion {
    pub params: ReligionParams,
    /// Each province's main faith at the last month's start (or as an edit of ours left it),
    /// [`NONE`] where nobody lives: a change is told as [`Event::Converted`].
    main: Vec<u32>,
    /// The day each province's group was found cut off from its faith's heartland, `u64::MAX`
    /// while it isn't.
    cut_since: Vec<u64>,
}

impl Conversion {
    pub fn new(params: ReligionParams) -> Conversion {
        Conversion { params, main: Vec::new(), cut_since: Vec::new() }
    }

    fn fit(&mut self, n: usize) {
        self.main.resize(n, NONE);
        self.cut_since.resize(n, u64::MAX);
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Religion,
    version: "conversion",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    // The realms' owners and capitals give the ruler's pull and the realms' faiths, the pops'
    // cultures the kin pull; all have defaults (nobody, none, one culture), so religion runs with
    // realms and culture off.
    reads: &["pop.province", "pop.religion", "pop.culture", "pop.size", "province.cells", "province.center", "province.owner", "realm.capital"],
    reads_earlier: &[],
    owns: &["religion.parent", "religion.born", "religion.hearth", "religion.died", "realm.religion"],
    // People take up another faith by a shift request to the pops' owner.
    requests: &["pop.religion"],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[EventKind::ReligionBorn, EventKind::ReligionGone, EventKind::Converted],
    handles: &[],
    state_schema: "faith-main-cut-1",
    goods: &[],
};

impl Module for Conversion {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    /// Followed people may be priests: the priests' share of the people.
    fn jobs(&self) -> Option<crate::lives::Jobs> {
        let priests = self.params.priests;
        Some(crate::lives::Jobs {
            reads: &["pop.province", "pop.size"],
            offer: std::sync::Arc::new(move |w: &World, p: usize, out: &mut Vec<crate::lives::Job>| {
                let people: u64 = (0..w.pops.size.len()).filter(|&j| w.pops.province[j] as usize == p).map(|j| w.pops.size[j]).sum();
                out.push(crate::lives::Job { name: "priest".into(), holders: people as f64 * priests });
            }),
        })
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        if p.starting < 1 || p.starting as usize > MOST {
            return Err(format!("the world starts with 1 to {MOST} faiths"));
        }
        if !(0.0..=1.0).contains(&p.priests) {
            return Err("the priests' share goes from 0 to 1".into());
        }
        if !(0.0..=0.5).contains(&p.borderland) {
            return Err("the mixed share of a borderland goes from 0 to a half".into());
        }
        if !(0.0..=1.0).contains(&p.per_year) {
            return Err("the share taking up the leading faith in a year goes from 0 to 1".into());
        }
        if ![p.own, p.neighbours, p.ruler, p.kin].iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err("the pulls of a faith's own people, its neighbours, the ruler and one's people are 0 or more".into());
        }
        if !(p.neighbour_half_days.is_finite() && p.neighbour_half_days > 0.0) {
            return Err("a neighbour's pull halves over more than 0 days".into());
        }
        if !(0.0..=1.0).contains(&p.small_share) {
            return Err("a small group's share goes from 0 to 1".into());
        }
        if p.most < 1 {
            return Err("a province keeps at least one faith".into());
        }
        if let Some(s) = &p.schisms {
            if !(s.years.is_finite() && s.years > 0.0) {
                return Err("schisms: a group is cut off for more than 0 years before it breaks away".into());
            }
            if s.provinces < 1 {
                return Err("schisms: a group that breaks away holds a province or more".into());
            }
            if !(0.0..=1.0).contains(&s.per_year) {
                return Err("schisms: the chance a year goes from 0 to 1".into());
            }
        }
        Ok(())
    }

    /// The starting faiths, by region, asked of the pops' owner as shifts from faith 0 (applied
    /// before the first day).
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        let people = w.province_population();
        w.religions = Religions::default();
        self.main = vec![NONE; n];
        self.cut_since = vec![u64::MAX; n];
        let region = regions(w, &people, self.params.starting as usize, cx.seed);
        for (f, &start) in region.starts.iter().enumerate() {
            debug_assert_eq!(f, w.religions.count());
            w.religions.push(NONE, 0, start);
        }
        let field = known("pop.religion");
        for p in 0..n {
            let f = region.of[p];
            if f == NONE {
                continue;
            }
            // The neighbouring faith with the most people next door, for a borderland.
            let mut other: Option<(u64, u32)> = None;
            for l in w.routes.of(p as u32) {
                let q = l.other(p as u32) as usize;
                let d = region.of[q];
                if d == NONE || d == f || l.days().is_none() {
                    continue;
                }
                let next: u64 = w.routes.of(p as u32).filter(|m| m.days().is_some() && region.of[m.other(p as u32) as usize] == d).map(|m| people[m.other(p as u32) as usize]).sum();
                if other.is_none_or(|(most, o)| next > most || (next == most && d < o)) {
                    other = Some((next, d));
                }
            }
            let mixed = other.map_or(0, |_| (people[p] as f64 * self.params.borderland).round() as u64);
            let other = other.map_or(NONE, |o| o.1);
            if f != 0 && people[p] > mixed {
                cx.ask_shift(field, p as u32, 0, f as u16, people[p] - mixed, Priority::Normal, Cause::Placed, p as u64);
            }
            if other != 0 && other != NONE && mixed > 0 {
                cx.ask_shift(field, p as u32, 0, other as u16, mixed, Priority::Normal, Cause::Placed, p as u64);
            }
            self.main[p] = if mixed > people[p] - mixed || (mixed == people[p] - mixed && other < f) { other } else { f };
        }
        w.realm_religion = realm_faiths(w, &self.main);
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let n = w.provinces.count;
        self.fit(n);
        if w.religions.count() == 0 {
            return;
        }
        let counts = Tally::build(&w.pops, n, |pops, k| pops.religion[k]);
        let main: Vec<u32> = (0..n).map(|p| counts.main(p)).collect();

        // Faiths with nobody left are gone; one with followers again (an edit) lives again.
        let mut total = vec![0u64; w.religions.count()];
        for (f, x) in counts.key.iter().zip(&counts.people) {
            if let Some(t) = total.get_mut(*f as usize) {
                *t += x;
            }
        }
        for (f, &t) in total.iter().enumerate() {
            let died = &mut w.religions.died[f];
            if *died == u64::MAX && t == 0 {
                *died = cx.day;
                cx.emit(Event::ReligionGone { religion: f as u32 });
            } else if *died != u64::MAX && t > 0 {
                *died = u64::MAX;
            }
        }

        w.realm_religion = realm_faiths(w, &main);

        // Main faiths that changed since last month.
        for p in 0..n {
            let (was, now) = (self.main[p], main[p]);
            if was != now {
                if was != NONE && now != NONE {
                    cx.emit(Event::Converted { province: p as u32, from: was, to: now });
                    if cx.traced(p as u32) {
                        cx.note(format!("the main faith changed from {} to {}", name(cx.seed, was), name(cx.seed, now)));
                    }
                }
                self.cut_since[p] = u64::MAX;
            }
        }
        self.main.clone_from(&main);

        let mut broke = vec![false; n];
        if let Some(s) = self.params.schisms.clone()
            && cx.calendar.is_year_start(cx.day)
        {
            self.schisms(w, cx, &s, &counts, &main, &mut broke);
        }
        if self.params.per_year > 0.0 {
            self.convert(w, cx, &counts, &broke);
        }
    }

    /// Grants for our shifts: nothing to do (the pops' owner has moved the people).
    fn handle(&mut self, _w: &mut World, _e: &Event, _cx: &mut Ctx) {}

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&(&self.main, &self.cut_since))
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        (self.main, self.cut_since) = postcard::from_bytes(bytes).map_err(|e| format!("Religion state: {e}"))?;
        Ok(())
    }

    fn check_edit(&self, w: &World, edit: &Edit) -> Option<Result<(), String>> {
        match *edit {
            Edit::SetReligion { religion, .. } => Some(if (religion as usize) < w.religions.count() { Ok(()) } else { Err(format!("no faith {religion}")) }),
            Edit::NewReligion { .. } => Some(if w.religions.count() < MOST { Ok(()) } else { Err(format!("there are already {MOST} faiths, the most there can be")) }),
            _ => None,
        }
    }

    /// Everyone in the provinces takes the faith (a new one for [`Edit::NewReligion`], born there in
    /// the most populous of them). Nobody is made or lost.
    fn carry_out(&mut self, w: &mut World, edit: &Edit, day: u64, _calendar: &Calendar) -> Flows {
        let (provinces, faith) = match *edit {
            Edit::SetReligion { ref provinces, religion } => {
                // A faith that was gone lives again.
                w.religions.died[religion as usize] = u64::MAX;
                (provinces, religion)
            }
            Edit::NewReligion { ref provinces } => {
                let people = w.province_population();
                let hearth = *provinces.iter().max_by(|&&a, &&b| people[a as usize].cmp(&people[b as usize]).then(b.cmp(&a))).expect("checked: some provinces");
                (provinces, w.religions.push(NONE, day, hearth))
            }
            _ => return Flows::default(),
        };
        set_religion(&mut w.pops, w.provinces.count, provinces, faith as u16);
        self.fit(w.provinces.count);
        let people = w.province_population();
        for &p in provinces {
            self.main[p as usize] = if people[p as usize] > 0 { faith } else { NONE };
            self.cut_since[p as usize] = u64::MAX;
        }
        Flows::default()
    }

    /// Provinces may have been added, and realms' capitals moved: their faiths follow.
    fn edited(&mut self, w: &mut World, _edit: &Edit) {
        self.fit(w.provinces.count);
        if w.religions.count() > 0 {
            let main = main_faiths(w);
            w.realm_religion = realm_faiths(w, &main);
        }
    }
}

impl Conversion {
    /// Groups cut off from their faith's heartland, and those that break away this year.
    fn schisms(&mut self, w: &mut World, cx: &mut Ctx, s: &SchismParams, counts: &Tally, main: &[u32], broke: &mut [bool]) {
        let n = main.len();
        // Provinces of one main faith that hang together along the ways, each group named by its
        // lowest province.
        let mut up: Vec<u32> = (0..n as u32).collect();
        fn find(up: &mut [u32], mut p: u32) -> u32 {
            while up[p as usize] != p {
                up[p as usize] = up[up[p as usize] as usize];
                p = up[p as usize];
            }
            p
        }
        for l in &w.routes.links {
            let (a, b) = (l.a as usize, l.b as usize);
            if a < n && b < n && main[a] != NONE && main[a] == main[b] && l.days().is_some() {
                let (ra, rb) = (find(&mut up, l.a), find(&mut up, l.b));
                if ra != rb {
                    up[ra.max(rb) as usize] = ra.min(rb);
                }
            }
        }
        let root: Vec<u32> = (0..n as u32).map(|p| find(&mut up, p)).collect();
        // Each group's people, and each faith's biggest group (ties: the lower group).
        let mut people = vec![0u64; n];
        for p in 0..n {
            if main[p] != NONE {
                people[root[p] as usize] += counts.total(p);
            }
        }
        let mut heart = vec![NONE; w.religions.count()];
        for g in 0..n {
            let f = main[g];
            if f == NONE || root[g] != g as u32 || f as usize >= heart.len() {
                continue;
            }
            let h = &mut heart[f as usize];
            if *h == NONE || people[g] > people[*h as usize] {
                *h = g as u32;
            }
        }
        let years = (s.years * cx.calendar.days_per_year() as f64).round() as u64;
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
        for p in 0..n {
            let f = main[p];
            let cut = f != NONE && (f as usize) < heart.len() && heart[f as usize] != root[p];
            if !cut {
                self.cut_since[p] = u64::MAX;
                continue;
            }
            if self.cut_since[p] == u64::MAX {
                self.cut_since[p] = cx.day;
            }
            members[root[p] as usize].push(p as u32);
        }
        for g in 0..n {
            let group = &members[g];
            let long = group.iter().filter(|&&p| self.cut_since[p as usize].saturating_add(years) <= cx.day).count();
            if group.is_empty() || long < s.provinces as usize || w.religions.count() >= MOST {
                continue;
            }
            if unit(cx.seed, Rng::Religion, cx.day, g as u64, purpose("schism")) >= s.per_year {
                continue;
            }
            let parent = main[g];
            let hearth = *group.iter().max_by(|&&a, &&b| counts.total(a as usize).cmp(&counts.total(b as usize)).then(b.cmp(&a))).expect("not empty");
            let born = w.religions.push(parent, cx.day, hearth);
            cx.emit(Event::ReligionBorn { religion: born, parent, hearth, provinces: group.len() as u32 });
            for &p in group {
                let p = p as usize;
                let x = counts.people_of(p, parent);
                if x > 0 {
                    cx.ask_shift(known("pop.religion"), p as u32, parent as u16, born as u16, x, Priority::Normal, Cause::Schism, p as u64);
                }
                broke[p] = true;
                self.main[p] = born;
                self.cut_since[p] = u64::MAX;
                if cx.traced(p as u32) {
                    cx.note(format!("cut off from the heartland of {} for {} years: its {x} followers break away as {}", name(cx.seed, parent), s.years, name(cx.seed, born)));
                }
            }
        }
    }

    /// The month's pull on each province, and the people who take up the leading faith.
    fn convert(&self, w: &World, cx: &mut Ctx, counts: &Tally, broke: &[bool]) {
        let pr = &self.params;
        let months = cx.calendar.months_per_year() as f64;
        let month_rate = 1.0 - (1.0 - pr.per_year).powf(1.0 / months);
        let field = known("pop.religion");
        let n = counts.provinces();
        let kin = if pr.kin > 0.0 { kin_faiths(&w.pops, n) } else { Vec::new() };
        // (faith, own people, people next door counted by their fade)
        let mut cand: Vec<(u32, u64, f64)> = Vec::new();
        let mut ranked: Vec<(u32, u64)> = Vec::new();
        let totals: Vec<u64> = (0..n).map(|p| counts.total(p)).collect();
        // The one faith of a province that has only one, NONE otherwise.
        let only: Vec<u32> = (0..n).map(|p| counts.only(p)).collect();
        for p in 0..n {
            let total = totals[p];
            if total == 0 || broke[p] {
                continue;
            }
            let owner = w.owner.get(p).copied().unwrap_or(NONE);
            let ruler = if owner != NONE && w.realms.alive(owner) { w.realm_religion.get(owner as usize).copied().unwrap_or(NONE) } else { NONE };
            let kin_faith = kin.get(p).copied().unwrap_or(NONE);
            // Most provinces are all of one faith, ruled by it or by nobody, among neighbours all
            // of it, their people's faith that one too: nobody there has anything to take up.
            let f = only[p];
            if f != NONE
                && (ruler == NONE || ruler == f || pr.ruler == 0.0)
                && (kin_faith == NONE || kin_faith == f)
                && (pr.neighbours == 0.0
                    || w.routes.of(p as u32).all(|l| {
                        let q = l.other(p as u32) as usize;
                        l.days().is_none() || q >= n || totals[q] == 0 || only[q] == f
                    }))
                && !cx.traced(p as u32)
            {
                continue;
            }
            cand.clear();
            cand.extend(counts.of(p).map(|(f, x)| (f as u32, x, 0.0)));
            let mut next_door = 0u64;
            for l in w.routes.of(p as u32) {
                let Some(days) = l.days() else { continue };
                let q = l.other(p as u32) as usize;
                if q >= n || totals[q] == 0 {
                    continue;
                }
                next_door += totals[q];
                let fade = (-(days as f64) / pr.neighbour_half_days).exp2();
                for (f, x) in counts.of(q) {
                    let f = f as u32;
                    match cand.iter_mut().find(|e| e.0 == f) {
                        Some(e) => e.2 += fade * x as f64,
                        None => cand.push((f, 0, fade * x as f64)),
                    }
                }
            }
            for extra in [ruler, kin_faith] {
                if extra != NONE && (extra as usize) < MOST && !cand.iter().any(|e| e.0 == extra) {
                    cand.push((extra, 0, 0.0));
                }
            }
            let pull = |e: &(u32, u64, f64)| {
                let own = pr.own * e.1 as f64 / total as f64;
                let near = if next_door > 0 { pr.neighbours * e.2 / next_door as f64 } else { 0.0 };
                own + near + if e.0 == ruler { pr.ruler } else { 0.0 } + if e.0 == kin_faith { pr.kin } else { 0.0 }
            };
            // The leader: the hardest pull; then the most people here; then the lower number.
            let lead = *cand.iter().max_by(|a, b| pull(a).total_cmp(&pull(b)).then(a.1.cmp(&b.1)).then(b.0.cmp(&a.0))).expect("someone lives here");
            let top = pull(&lead);
            if cx.traced(p as u32) {
                let list: Vec<String> = cand.iter().map(|e| format!("{} {:.2}", name(cx.seed, e.0), pull(e))).collect();
                cx.note(format!("pulls on its {total} people: {}; {} leads", list.join(", "), name(cx.seed, lead.0)));
            }
            if top <= 0.0 {
                continue;
            }
            // The faiths kept here: the leader and the biggest others, up to `most` in all; the
            // groups of the rest join the leader at once.
            ranked.clear();
            ranked.extend(cand.iter().filter(|e| e.1 > 0 && e.0 != lead.0).map(|e| (e.0, e.1)));
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let keep = (pr.most as usize).saturating_sub(1);
            for e in &cand {
                let (f, x) = (e.0, e.1);
                if f == lead.0 || x == 0 {
                    continue;
                }
                let beyond = ranked.iter().position(|r| r.0 == f).is_some_and(|i| i >= keep);
                let gap = top - pull(e);
                if gap <= 0.0 && !beyond {
                    continue;
                }
                let small = beyond || x < pr.small_people || (x as f64) < pr.small_share * total as f64;
                let take = if small { x } else { round_stochastic(x as f64 * month_rate * gap / top, cx.seed, Rng::Religion, cx.day, p as u64, purpose("convert") ^ f as u64).min(x) };
                if take > 0 {
                    cx.ask_shift(field, p as u32, f as u16, lead.0 as u16, take, Priority::Normal, Cause::Converted, p as u64);
                    if cx.traced(p as u32) {
                        let why = if beyond {
                            format!(" (beyond the {} biggest faiths here, all at once)", pr.most)
                        } else if small {
                            " (a small group, all at once)".to_string()
                        } else {
                            String::new()
                        };
                        cx.note(format!("{take} of its {x} {} take up {}{why}", name(cx.seed, f), name(cx.seed, lead.0)));
                    }
                }
            }
        }
    }
}

/// Each province's people by one pop key (faith or culture), from the pops: `of(p)` lists its
/// values that have people.
struct Tally {
    first: Vec<u32>,
    end: Vec<u32>,
    key: Vec<u16>,
    people: Vec<u64>,
}

impl Tally {
    fn build(pops: &Pops, n: usize, key_of: impl Fn(&Pops, usize) -> u16) -> Tally {
        let mut first = vec![0u32; n + 1];
        for (k, &p) in pops.province.iter().enumerate() {
            if pops.size[k] > 0 && (p as usize) < n {
                first[p as usize + 1] += 1;
            }
        }
        for p in 0..n {
            first[p + 1] += first[p];
        }
        let rows = first[n] as usize;
        let (mut key, mut people) = (vec![0u16; rows], vec![0u64; rows]);
        let mut end: Vec<u32> = first[..n].to_vec();
        for (k, &p) in pops.province.iter().enumerate() {
            let (p, x) = (p as usize, pops.size[k]);
            if x == 0 || p >= n {
                continue;
            }
            let (from, to) = (first[p] as usize, end[p] as usize);
            let v = key_of(pops, k);
            match key[from..to].iter().position(|&c| c == v) {
                Some(i) => people[from + i] += x,
                None => {
                    key[to] = v;
                    people[to] = x;
                    end[p] += 1;
                }
            }
        }
        first.truncate(n);
        Tally { first, end, key, people }
    }

    fn of_world(w: &World) -> Tally {
        Tally::build(&w.pops, w.provinces.count, |pops, k| pops.religion[k])
    }

    fn of(&self, p: usize) -> impl Iterator<Item = (u16, u64)> + '_ {
        let (a, b) = (self.first[p] as usize, self.end[p] as usize);
        self.key[a..b].iter().copied().zip(self.people[a..b].iter().copied())
    }

    fn provinces(&self) -> usize {
        self.first.len()
    }

    fn total(&self, p: usize) -> u64 {
        self.of(p).map(|e| e.1).sum()
    }

    /// The province's one value, if it has exactly one; [`NONE`] otherwise.
    fn only(&self, p: usize) -> u32 {
        if self.end[p] == self.first[p] + 1 { self.key[self.first[p] as usize] as u32 } else { NONE }
    }

    fn people_of(&self, p: usize, v: u32) -> u64 {
        self.of(p).filter(|e| e.0 as u32 == v).map(|e| e.1).sum()
    }

    /// The value with the most people, the lower number of equals; [`NONE`] where nobody lives.
    fn main(&self, p: usize) -> u32 {
        self.of(p).max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(NONE, |e| e.0 as u32)
    }
}

/// Each province's kin faith: the faith most of the people of its main culture hold across the
/// world (the lower number of equals). Empty when the world has fewer than two cultures with
/// people (with Culture off, everyone is culture 0), and [`NONE`] where nobody lives.
pub(crate) fn kin_faiths(pops: &Pops, n: usize) -> Vec<u32> {
    // People by (culture, faith) across the world, in a fixed order.
    let mut pairs: Vec<(u16, u16, u64)> = (0..pops.size.len()).filter(|&k| pops.size[k] > 0 && (pops.province[k] as usize) < n).map(|k| (pops.culture[k], pops.religion[k], pops.size[k])).collect();
    if pairs.iter().all(|e| e.0 == pairs[0].0) {
        return Vec::new();
    }
    pairs.sort_unstable_by_key(|e| (e.0, e.1));
    let cultures = pairs.last().map_or(0, |e| e.0 as usize + 1);
    // For each culture: (people of its best faith so far, that faith).
    let mut best: Vec<(u64, u32)> = vec![(0, NONE); cultures];
    for run in pairs.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
        let (c, f) = (run[0].0 as usize, run[0].1 as u32);
        let x: u64 = run.iter().map(|e| e.2).sum();
        // Runs come by faith ascending, so a tie keeps the lower faith.
        if x > best[c].0 {
            best[c] = (x, f);
        }
    }
    let culture = Tally::build(pops, n, |pops, k| pops.culture[k]);
    (0..n).map(|p| culture.main(p)).map(|c| if c == NONE { NONE } else { best.get(c as usize).map_or(NONE, |b| b.1) }).collect()
}

/// Each province's main faith: the one with the most followers (the lower number of equals),
/// [`NONE`] where nobody lives.
pub fn main_faiths(w: &World) -> Vec<u32> {
    let c = Tally::of_world(w);
    (0..c.provinces()).map(|p| c.main(p)).collect()
}

/// Each province's people by faith, most first (the lower number of equals); empty where nobody
/// lives. For showing: the window's faith map and province page.
pub fn mix(w: &World) -> Vec<Vec<(u32, u64)>> {
    let c = Tally::of_world(w);
    (0..c.provinces())
        .map(|p| {
            let mut v: Vec<(u32, u64)> = c.of(p).map(|(k, x)| (k as u32, x)).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        })
        .collect()
}

/// Each realm's faith: its capital's main faith; [`NONE`] for a dissolved realm or an empty
/// capital.
fn realm_faiths(w: &World, main: &[u32]) -> Vec<u32> {
    (0..w.realms.count()).map(|r| w.realms.capital[r]).map(|c| if c == NONE { NONE } else { main.get(c as usize).copied().unwrap_or(NONE) }).collect()
}

/// For unrest from rule by a foreign faith: each province's share of people not of its realm's
/// faith, in thousandths (0 where no living realm holds it, or its realm has no faith). A realm's
/// faith is its capital's main faith, worked out here from the pops, so a module that reads only the
/// pops, the owners and the capitals can use it (no same-day loop with the religion module, which
/// reads those; never from `realm.religion`). With Religion off everyone is faith 0, and it is 0
/// everywhere.
pub fn foreign_share(w: &World) -> Vec<u64> {
    let counts = Tally::of_world(w);
    let n = counts.provinces();
    let realm: Vec<u32> = (0..w.realms.count())
        .map(|r| if w.realms.alive(r as u32) { w.realms.capital[r] } else { NONE })
        .map(|c| if c == NONE || c as usize >= n { NONE } else { counts.main(c as usize) })
        .collect();
    (0..n)
        .map(|p| {
            let owner = w.owner.get(p).copied().unwrap_or(NONE);
            let ruling = if owner == NONE { NONE } else { realm.get(owner as usize).copied().unwrap_or(NONE) };
            let total = counts.total(p);
            if ruling == NONE || total == 0 {
                return 0;
            }
            let foreign = total - counts.people_of(p, ruling);
            ((foreign as u128 * 1000 + total as u128 / 2) / total as u128) as u64
        })
        .collect()
}

/// Everyone in `provinces` takes faith `to`: each pop of another faith joins the province's pop of
/// the same kind with `to` (the lowest numbered), or takes `to` itself if there is none. Pops are
/// never removed or reordered.
fn set_religion(pops: &mut Pops, n: usize, provinces: &[u32], to: u16) {
    let mut wanted = vec![false; n];
    provinces.iter().for_each(|&p| wanted[p as usize] = true);
    let mut of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, &p) in pops.province.iter().enumerate() {
        if wanted.get(p as usize) == Some(&true) {
            of[p as usize].push(k);
        }
    }
    let mut done = vec![false; n];
    for &p in provinces {
        let p = p as usize;
        if std::mem::replace(&mut done[p], true) {
            continue;
        }
        let mut list = std::mem::take(&mut of[p]);
        for i in 0..list.len() {
            let k = list[i];
            if pops.religion[k] != to && pops.size[k] > 0 {
                let size = pops.size[k];
                crate::requests::shift_part(pops, &mut list, "pop.religion", k, size, to);
            }
        }
    }
}

/// Where each starting faith begins, and each province's faith ([`NONE`] for unsettled land).
struct Regions {
    starts: Vec<u32>,
    of: Vec<u32>,
}

/// `k` faiths (at most one per settled province) spread over the settled land: spread-out starts,
/// each growing along the ways nearest first by travel days. Settled land none reaches (an island
/// with no way to it) takes the faith whose start is nearest as the crow flies. (Culture's regions,
/// drawn from Religion's own random stream.)
fn regions(w: &World, people: &[u64], k: usize, seed: u64) -> Regions {
    let n = w.provinces.count;
    let settled: Vec<u32> = (0..n as u32).filter(|&p| w.provinces.cells[p as usize] > 0 && people[p as usize] > 0).collect();
    let mut of = vec![NONE; n];
    if settled.is_empty() {
        return Regions { starts: Vec::new(), of };
    }
    let starts = spread(w, &settled, k.min(settled.len()).min(MOST), people, seed);
    let mut dist = vec![u64::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::new();
    for (f, &s) in starts.iter().enumerate() {
        heap.push(Reverse((0, f as u32, s)));
    }
    while let Some(Reverse((d, f, p))) = heap.pop() {
        let pu = p as usize;
        if dist[pu] != u64::MAX {
            continue;
        }
        dist[pu] = d;
        of[pu] = f;
        for l in w.routes.of(p) {
            let q = l.other(p);
            if let (u64::MAX, Some(days)) = (dist[q as usize], l.days()) {
                heap.push(Reverse((d + ((days as f64 * 1000.0).round() as u64).max(1), f, q)));
            }
        }
    }
    let center = |p: u32| w.provinces.center[p as usize];
    for &p in &settled {
        if of[p as usize] == NONE {
            let nearest = (0..starts.len()).max_by(|&a, &b| dot(center(p), center(starts[a])).total_cmp(&dot(center(p), center(starts[b]))).then(b.cmp(&a)));
            of[p as usize] = nearest.map_or(NONE, |f| f as u32);
        }
    }
    // Only settled land has a faith.
    for p in 0..n {
        if people[p] == 0 || w.provinces.cells[p] == 0 {
            of[p] = NONE;
        }
    }
    Regions { starts, of }
}

/// `k` starting provinces among `cands`, spread out: the most populous first, then each time the
/// one farthest from those chosen, favouring the more populous and shuffled a little (as cultures
/// and realms start).
fn spread(w: &World, cands: &[u32], k: usize, people: &[u64], seed: u64) -> Vec<u32> {
    let most = cands.iter().map(|&p| people[p as usize]).max().unwrap_or(1).max(1) as f64;
    let weight: Vec<f64> = cands
        .iter()
        .map(|&p| {
            let jitter = 0.7 + 0.6 * unit(seed, Rng::Religion, 0, p as u64, purpose("spread"));
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

const ONSETS: &[&str] = &["", "b", "c", "d", "f", "g", "h", "k", "l", "m", "n", "p", "r", "s", "t", "v", "y", "z", "th", "sh", "al", "el", "or"];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "a", "e", "ae", "io", "au"];
const CODAS: &[&str] = &["", "", "n", "r", "l", "s", "m", "th", "ph", "x"];
const ENDINGS: &[&str] = &["", "ism", "ite", "a", "ian", "ar", "is", "on", "ene", "ism"];

/// Faith names, the first `count` faiths', from the world's seed alone: two or three syllables,
/// often with a faith's ending ("Tholism", "Saphite"). No two are the same.
pub fn names(seed: u64, count: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(count);
    for f in 0..count as u64 {
        for attempt in 0.. {
            let mut k = 0u64;
            let mut pick = |list: &[&'static str]| {
                k += 1;
                list[(draw(seed, Rng::Religion, attempt, f, purpose("name") ^ k) % list.len() as u64) as usize]
            };
            let syllables = 2 + (pick(&["", "", "", "x"]) == "x") as usize;
            let mut name = String::new();
            for s in 0..syllables {
                name.push_str(pick(if s == 0 { ONSETS } else { &ONSETS[1..20] }));
                name.push_str(pick(VOWELS));
                if s + 1 == syllables {
                    name.push_str(pick(CODAS));
                }
            }
            let ending = pick(ENDINGS);
            let vowel = |c: Option<char>| c.is_some_and(|c| "aeiou".contains(c));
            if !ending.is_empty() && vowel(name.chars().last()) != vowel(ending.chars().next()) {
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

/// One faith's name (for notes); its number for one past what [`names`] makes cheaply.
fn name(seed: u64, f: u32) -> String {
    if f == NONE {
        return "nobody".into();
    }
    names(seed, f as usize + 1).pop().unwrap_or_else(|| format!("faith {f}"))
}
