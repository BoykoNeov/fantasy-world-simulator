//! Lives (step 9): a few named people followed through their lives.
//!
//! A followed person is a *sample* of their province's people, not an addition: they are not
//! counted in the people total, and nothing they do changes the world. Lives sends no requests,
//! owns no field and emits no events; its people are its own state, shown beside the world
//! ([`World::lives`]) as battles and trade are. With Lives on or off the world runs the same, day
//! for day; only Lives' own state differs.
//!
//! Two versions:
//!
//! - **"diary"** (v0): each person ages, marries, has children and may die each month: of age
//!   (the rulers' table, [`crate::characters::death_per_year`]), and at home of famine and plague
//!   by the share of their province's people who starved and died of plague that month. Each year
//!   a line of how the year went where they live: people more or fewer, starved, plague dead, and,
//!   where the world keeps coins, the coins a head ("lives" tells only the years that stood out).
//! - **"lives"** (v1): the diary, and jobs and what befalls people by the world's events
//!   ([`RoleParams`]). The jobs are the ones the modules that are on offer ([`Jobs`], declared in
//!   each module's own file): a farmer under the food economy, under markets the goods the
//!   province's workers make, a scholar where a realm studies, a priest where faiths are kept, a
//!   hunter where the wild has game. Grown people of fighting age may be taken for soldiers when
//!   their province raises them, by the share of its people taken; a soldier fights the battles
//!   their company fights, dying by the share of their side that died, and may fall to camp
//!   disease, cold, hunger or desert by the shares their realm's soldiers did, or go home when
//!   their company is sent home. At home they see uprisings, occupation, wars and peace, new
//!   rulers, plagues arriving, faiths and cultures changing (taking them up by the share of their
//!   province that follows them), the woods cleared and the fish gone; scholars see their realm's
//!   discoveries.
//!
//! People are followed from the world's first day ([`LifeParams::at_start`], across the world by
//! where people live) and by the player's edit ([`Edit::Follow`]). When a followed person dies,
//! their story carries on with their eldest grown child alive, if they have one: children and
//! spouses are kept as quiet kin, who age and may die but have no story of their own. Nobody
//! moves between provinces in the world, so followed people don't either, except soldiers, who
//! come home.

use crate::characters::death_per_year;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::rng::{purpose, unit, Module as Rng};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `Person::died` while they live.
pub const NEVER: u64 = u64::MAX;
/// Name numbers of followed people and their kin start here, so their names come out unlike the
/// rulers' (both are made by [`crate::characters::name`]); family names start at `FAMILIES_FROM`.
const NAMES_FROM: u32 = 1 << 30;
const FAMILIES_FROM: u32 = NAMES_FROM | 1 << 29;
/// A job's name for a child, before they are grown.
pub const CHILD: &str = "child";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LifeParams {
    /// People followed from the world's first day, picked across the world by where people live.
    pub at_start: u32,
    /// Multiplies everyone's chance of dying of age and illness; 1 is as the rulers' table has it.
    pub mortality: f64,
    /// The age at which a child is grown: works, may marry and be taken for a soldier, and may
    /// carry their parent's story on.
    pub grown: f64,
    /// Chance a year that a grown, unmarried person younger than [`LifeParams::marry_until`]
    /// marries.
    pub marriage: f64,
    pub marry_until: f64,
    /// Chance a year that a married couple has a child, while the mother is younger than
    /// [`LifeParams::last_child`].
    pub children: f64,
    pub last_child: f64,
    /// Jobs and what befalls people by the world's events: Lives "lives". None for the "diary". A
    /// missing key in a file means none (not the default), so the diary survives a settings change.
    #[serde(default)]
    pub roles: Option<RoleParams>,
}

impl Default for LifeParams {
    fn default() -> Self {
        LifeParams { at_start: 8, mortality: 1.0, grown: 16.0, marriage: 0.15, marry_until: 40.0, children: 0.2, last_child: 45.0, roles: Some(RoleParams::default()) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleParams {
    /// Chance a year that a grown person takes up another of the jobs open where they live.
    pub job_change: f64,
    /// Years in one job, at middling talent, to master it (the talented sooner).
    pub master_years: f64,
    /// Ages at which a person may be taken for a soldier.
    pub soldier_from: f64,
    pub soldier_until: f64,
    /// Share of a province's people who are of those ages: a person of them is taken one over this
    /// times as readily as the province's people on average.
    pub soldier_share: f64,
    /// Soldiers wounded in a battle for each one killed.
    pub wounded: f64,
    /// Chance that the boldest soldier on the winning side of a battle is praised for it (the less
    /// bold in proportion).
    pub praised: f64,
    /// Multiplies the chance of taking up a faith or culture new to their province: its share of
    /// the province's people.
    pub swayed: f64,
}

impl Default for RoleParams {
    fn default() -> Self {
        RoleParams { job_change: 0.03, master_years: 15.0, soldier_from: 16.0, soldier_until: 45.0, soldier_share: 0.4, wounded: 2.0, praised: 0.1, swayed: 1.0 }
    }
}

/// Jobs a module offers followed people, declared in that module's own file
/// ([`Module::jobs`]); only the modules that are on offer theirs.
#[derive(Clone)]
pub struct Jobs {
    /// The fields `offer` reads (Lives' manifest reads them all).
    pub reads: &'static [&'static str],
    /// The jobs open in a province today, each with how many people there hold it.
    pub offer: Offer,
}

/// What a module's jobs say are open in a province (see [`Jobs::offer`]).
pub type Offer = Arc<dyn Fn(&World, usize, &mut Vec<Job>) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub name: String,
    pub holders: f64,
}

/// How a person died.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Death {
    /// Old age, illness or an accident: the rulers' table.
    Age,
    Famine,
    Plague,
    Battle,
    /// Camp disease or cold, away with their company.
    Camp,
    /// Hunger, with their company.
    Hunger,
}

impl Death {
    pub fn describe(self) -> &'static str {
        match self {
            Death::Age => "of age and illness",
            Death::Famine => "of famine",
            Death::Plague => "of plague",
            Death::Battle => "in battle",
            Death::Camp => "of camp disease and cold",
            Death::Hunger => "of hunger with the company",
        }
    }
}

/// One line of a story: what happened, on which day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub day: u64,
    pub what: What,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum What {
    /// The story begins: followed from today, `age` years old.
    Begun { age: f32 },
    /// The story carries on with this person, after their parent `from` died.
    CarriedOn { from: u32 },
    Married { spouse: u32 },
    Child { child: u32 },
    /// A spouse, child or parent died.
    KinDied { who: u32 },
    Died { how: Death },
    /// How the year that just ended went where they live: people at its end against its start,
    /// starved and dead of plague in it, and coins a head at its start and end (none where the
    /// world keeps no coins).
    Year { people_was: u64, people: u64, starved: u64, plague: u64, coins: Option<(f32, f32)> },
    /// The realm their province belongs to changed hands (`NONE`: no realm).
    Realm { from: u32, to: u32 },
    Job { job: String },
    Master { job: String },
    /// Taken for a soldier, into company `army` of `realm` (a guard if `guard`).
    Taken { army: u32, realm: u32, guard: bool },
    Battle { province: u32, won: bool, wounded: bool },
    Praised { province: u32 },
    /// Back home from soldiering: sent home, or deserted.
    Home { deserted: bool },
    Rising { province: u32 },
    Occupied { province: u32, by: u32 },
    War { against: u32 },
    /// A war of their realm ended, with `against`; `won` is none when nobody won.
    Peace { against: u32, won: Option<bool> },
    NewRuler { realm: u32, ruler: u32 },
    Discovery { realm: u32, tech: u16 },
    Faith { to: u32 },
    Culture { to: u32 },
    PlagueCame { outbreak: u32 },
    WoodsCleared,
    FishGone,
}

/// One person, followed or kin of someone followed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Person {
    /// Their first name and their family's, as numbers [`crate::characters::name`] turns into
    /// words. A child takes the family of their followed parent.
    pub name: u32,
    pub family: u32,
    pub female: bool,
    /// The day they were born, a whole number of days; below 0 before the world's first day.
    pub born: f64,
    /// The day they died, [`NEVER`] while alive; and how.
    pub died: u64,
    pub how: Option<Death>,
    /// The province they live in (a soldier's home).
    pub province: u32,
    pub culture: u32,
    pub religion: u32,
    /// Followed: has a story. Kin: a spouse or child of someone followed, without one.
    pub followed: bool,
    pub parent: u32,
    pub spouse: u32,
    pub children: Vec<u32>,
    /// Their job ("" none yet), and the day they took it up.
    pub job: String,
    pub job_since: u64,
    pub master: bool,
    /// The company they serve in, [`NONE`] when not a soldier.
    pub army: u32,
    /// 0 to 100: hardiness (the hardy die less readily), talent (they master a job sooner),
    /// boldness (praised, and wounded, more readily).
    pub hardiness: u8,
    pub talent: u8,
    pub boldness: u8,
    /// Their coins against their province's a head: what they have is this times it.
    pub wealth: f32,
    /// The realm their province belonged to when last looked at.
    pub realm: u32,
    pub lines: Vec<Line>,
}

impl Person {
    pub fn alive(&self) -> bool {
        self.died == NEVER
    }

    /// Age in years on `day` (or on the day they died).
    pub fn age(&self, day: u64, days_per_year: u32) -> f64 {
        (self.died.min(day) as f64 - self.born) / days_per_year as f64
    }

    /// How much more readily than most they die: the hardiest half as readily, the frailest half
    /// again as readily.
    fn frailty(&self) -> f64 {
        1.5 - self.hardiness as f64 / 100.0
    }
}

/// Everyone Lives keeps: the followed and their kin. Numbers are never reused.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct People {
    pub people: Vec<Person>,
}

impl People {
    /// The followed, living and dead, in the order they were first followed.
    pub fn followed(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.people.len()).filter(|&k| self.people[k].followed)
    }

    /// Where person `k` is on the map today: their company's province while a soldier, else home.
    pub fn place(&self, w: &World, k: usize) -> u32 {
        let p = &self.people[k];
        match w.armies.province.get(p.army as usize) {
            Some(&at) if p.army != NONE => at,
            _ => p.province,
        }
    }
}

/// What Lives watches between months, to tell what changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Watch {
    /// Each company's soldiers at the last month's start.
    army_size: Vec<u64>,
    /// Each realm's soldiers lost to camp disease and cold, hunger and desertion so far, at the
    /// last month's start.
    realm_lost: Vec<[u64; 3]>,
    /// Per province where someone lives: starved and plague dead since the year began, at the last
    /// month's start.
    seen: BTreeMap<u32, (u64, u64)>,
    /// Per province where someone followed lives: the year so far (people and coins a head at its
    /// start, starved, plague dead).
    year: BTreeMap<u32, YearSoFar>,
    /// Soldiers each company is thought to have lost in battles since the month began.
    battle_dead: BTreeMap<u32, f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct YearSoFar {
    /// The day it began: the year's start, or when the first person there was followed.
    since: u64,
    people: u64,
    coins: Option<f32>,
    starved: u64,
    plague: u64,
}

pub struct Lives {
    pub params: LifeParams,
    /// The jobs the modules that are on offer.
    jobs: Vec<Jobs>,
    people: Arc<People>,
    watch: Watch,
}

impl Lives {
    pub fn new(params: LifeParams, jobs: Vec<Jobs>) -> Lives {
        Lives { params, jobs, people: Arc::default(), watch: Watch::default() }
    }

    /// Change the people, and show the world the result.
    fn change(&mut self, w: &mut World, f: impl FnOnce(&mut People, &mut Watch, &Self, &World)) {
        // The world's copy let go first, so the people aren't copied if nobody else holds them.
        w.lives = Arc::default();
        let mut people = std::mem::take(&mut self.people);
        let mut watch = std::mem::take(&mut self.watch);
        f(Arc::make_mut(&mut people), &mut watch, self, w);
        self.people = people;
        self.watch = watch;
        w.lives = self.people.clone();
    }

    /// The jobs open in province `p` today.
    fn open_jobs(&self, w: &World, p: usize) -> Vec<Job> {
        let mut out = Vec::new();
        for j in &self.jobs {
            (j.offer)(w, p, &mut out);
        }
        out.retain(|j| j.holders.is_finite() && j.holders > 0.0);
        out
    }
}

/// The fields every version reads. The jobs' fields are added to "lives"'s list: a test checks
/// every module's jobs read only fields listed here.
const READS: &[&str] = &["pop.province", "pop.size", "pop.culture", "pop.religion", "province.owner", "province.starved_this_year", "province.plague_dead", "province.stock"];

static V0: Manifest = Manifest {
    slot: Slot::Lives,
    version: "diary",
    tier: crate::fields::Tier::Province,
    cadence: Cadence::Monthly,
    reads: READS,
    reads_earlier: &[],
    owns: &[],
    requests: &[],
    asks: &[],
    moves: &[],
    // People are followed out of the pops.
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "lives-1",
    goods: &[],
};

/// Every field "lives" reads: the diary's, the soldiers', and every module's jobs'.
pub const LIVES_READS: &[&str] = &[
    "pop.province",
    "pop.size",
    "pop.culture",
    "pop.religion",
    "province.owner",
    "province.starved_this_year",
    "province.plague_dead",
    "province.stock",
    "army.realm",
    "army.province",
    "army.size",
    "army.home",
    "army.role",
    "realm.lost_sickness",
    "realm.lost_hunger",
    "realm.lost_deserted",
    "province.workers",
    "realm.studying",
    "province.game_lost",
];

static V1: Manifest = Manifest {
    version: "lives",
    reads: LIVES_READS,
    handles: &[
        EventKind::Battle,
        EventKind::Uprising,
        EventKind::Occupied,
        EventKind::WarDeclared,
        EventKind::Peace,
        EventKind::Succession,
        EventKind::Discovery,
        EventKind::Converted,
        EventKind::Assimilated,
        EventKind::Outbreak,
        EventKind::PlagueArrived,
        EventKind::WoodsCleared,
        EventKind::FisheryCollapsed,
    ],
    ..V0
};

/// A chance a year as a chance a month.
fn monthly(per_year: f64, months: f64) -> f64 {
    1.0 - (1.0 - per_year.clamp(0.0, 1.0)).powf(1.0 / months)
}

/// A share of `of`, safe for nobody.
fn share(x: u64, of: u64) -> f64 {
    if of == 0 { 0.0 } else { x as f64 / of as f64 }
}

/// Coins a head in province `p`, where the world keeps coins.
fn coins_a_head(w: &World, p: usize, people: u64) -> Option<f32> {
    let money = w.goods.tree.find("money")?;
    let cols = w.goods.columns_under(money)?;
    if cols.is_empty() || people == 0 {
        return None;
    }
    let row = w.stock.get(p * w.goods.stride()..(p + 1) * w.goods.stride())?;
    Some((cols.iter().map(|&c| row[c]).sum::<u64>() as f64 / people as f64) as f32)
}

/// A trait from 0 to 100, most often near the middle.
fn trait_of(seed: u64, key: u64, which: &str) -> u8 {
    let u = |k: u64| unit(seed, Rng::Lives, k, key, purpose(which));
    ((u(0) + u(1)) * 50.0).round() as u8
}

/// The realm of the province `p`, [`NONE`] if none.
fn owner(w: &World, p: u32) -> u32 {
    w.owner.get(p as usize).copied().unwrap_or(NONE)
}

/// A new person of province `p`, `age` years old on `day`: their culture and faith those of one of
/// the province's pops, picked by its size. Their number `number` makes the draws theirs.
fn make(w: &World, seed: u64, day: u64, days_per_year: u32, number: u32, p: u32, age: f64) -> Person {
    let key = number as u64;
    let u = |what: &str| unit(seed, Rng::Lives, day, key, purpose(what));
    let pops = &w.pops;
    let total: u64 = (0..pops.size.len()).filter(|&j| pops.province[j] == p).map(|j| pops.size[j]).sum();
    let mut pick = (u("pop") * total as f64) as u64;
    let (mut culture, mut religion) = (NONE, NONE);
    for j in 0..pops.size.len() {
        if pops.province[j] != p {
            continue;
        }
        (culture, religion) = (pops.culture[j] as u32, pops.religion[j] as u32);
        if pick < pops.size[j] {
            break;
        }
        pick -= pops.size[j];
    }
    // About one in three is half or twice as well off as their province's people on average.
    let z = (u("wealth 1") + u("wealth 2") + u("wealth 3") - 1.5) * 2.0;
    Person {
        name: NAMES_FROM + number,
        family: FAMILIES_FROM + number,
        female: u("female") < 0.5,
        born: day as f64 - (age * days_per_year as f64).round(),
        died: NEVER,
        how: None,
        province: p,
        culture,
        religion,
        followed: false,
        parent: NONE,
        spouse: NONE,
        children: Vec::new(),
        job: String::new(),
        job_since: day,
        master: false,
        army: NONE,
        hardiness: trait_of(seed, key, "hardiness"),
        talent: trait_of(seed, key, "talent"),
        boldness: trait_of(seed, key, "boldness"),
        wealth: (0.5 * z).exp() as f32,
        realm: owner(w, p),
        lines: Vec::new(),
    }
}

/// Follow a new person of province `p` from today, `age` years old.
fn follow(people: &mut People, w: &World, seed: u64, day: u64, days_per_year: u32, p: u32, age: f64) {
    let k = people.people.len() as u32;
    let mut person = make(w, seed, day, days_per_year, k, p, age);
    person.followed = true;
    person.lines.push(Line { day, what: What::Begun { age: age as f32 } });
    people.people.push(person);
}

impl Module for Lives {
    fn manifest(&self) -> &'static Manifest {
        if self.params.roles.is_some() { &V1 } else { &V0 }
    }

    fn check(&self, _goods: &crate::fields::Goods) -> Result<(), String> {
        let p = &self.params;
        let ok = |x: f64, lo: f64, hi: f64| x.is_finite() && x >= lo && x <= hi;
        if p.at_start > 10_000 {
            return Err("at most 10,000 people are followed from the start".into());
        }
        if !ok(p.mortality, 0.0, 100.0) {
            return Err("mortality goes from 0 to 100".into());
        }
        if !ok(p.grown, 0.0, 100.0) || !ok(p.marry_until, 0.0, 150.0) || !ok(p.last_child, 0.0, 150.0) {
            return Err("ages go from 0 to 150 years (grown at most 100)".into());
        }
        if !ok(p.marriage, 0.0, 1.0) || !ok(p.children, 0.0, 1.0) {
            return Err("the chances of marrying and of children go from 0 to 1 a year".into());
        }
        if let Some(r) = &p.roles {
            if !ok(r.job_change, 0.0, 1.0) || !ok(r.praised, 0.0, 1.0) {
                return Err("the chances of changing job and of being praised go from 0 to 1".into());
            }
            if !(ok(r.master_years, 0.0, 1000.0) && r.master_years > 0.0) {
                return Err("mastering a job takes more than 0 years".into());
            }
            if !ok(r.soldier_from, 0.0, 150.0) || !ok(r.soldier_until, 0.0, 150.0) {
                return Err("soldiers' ages go from 0 to 150 years".into());
            }
            if !(ok(r.soldier_share, 0.0, 1.0) && r.soldier_share > 0.0) {
                return Err("the share of people of fighting age is above 0 and at most 1".into());
            }
            if !ok(r.wounded, 0.0, 1000.0) || !ok(r.swayed, 0.0, 1000.0) {
                return Err("the wounded for each killed and how readily people are swayed are 0 or more".into());
            }
        }
        Ok(())
    }

    /// The people followed from the first day, across the world by where people live.
    fn init(&mut self, w: &mut World, cx: &mut Ctx) {
        let people = w.province_population();
        let total: u64 = people.iter().sum();
        let (seed, day, year) = (cx.seed, cx.day, cx.calendar.days_per_year());
        let at_start = if total == 0 { 0 } else { self.params.at_start };
        let grown = self.params.grown;
        self.change(w, |st, watch, _, w| {
            for i in 0..at_start as u64 {
                let u = |what: &str| unit(seed, Rng::Lives, day, i, purpose(what));
                let mut pick = (u("where") * total as f64) as u64;
                let p = people.iter().position(|&n| {
                    if pick < n {
                        return true;
                    }
                    pick -= n;
                    false
                });
                let p = p.unwrap_or(people.len() - 1) as u32;
                follow(st, w, seed, day, year, p, grown + 24.0 * u("age"));
            }
            for k in 0..st.people.len() {
                let p = st.people[k].province;
                watch.year.entry(p).or_insert_with(|| YearSoFar { since: day, people: people[p as usize], coins: coins_a_head(w, p as usize, people[p as usize]), ..Default::default() });
            }
            watch.army_size = w.armies.size.clone();
            watch.realm_lost = realm_lost(w);
        });
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        let people = w.province_population();
        self.change(w, |st, watch, me, w| me.month(st, watch, w, cx, &people));
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let Some(roles) = self.params.roles.clone() else { return };
        // Most of the world's events touch nobody followed: the people aren't copied for them.
        if !touches(&self.people, w, e) {
            return;
        }
        let params = self.params.clone();
        self.change(w, |st, watch, _, w| event(st, watch, w, cx, e, &params, &roles));
    }

    fn check_edit(&self, w: &World, edit: &Edit) -> Option<Result<(), String>> {
        let Edit::Follow { province } = *edit else { return None };
        let people: u64 = (0..w.pops.size.len()).filter(|&j| w.pops.province[j] == province).map(|j| w.pops.size[j]).sum();
        Some(if people > 0 { Ok(()) } else { Err(format!("nobody lives in province {province} to follow")) })
    }

    /// Follow someone of the province from today: grown, of any age up to middle age.
    fn carry_out(&mut self, w: &mut World, edit: &Edit, day: u64, calendar: &crate::calendar::Calendar) -> crate::module::Flows {
        if let Edit::Follow { province } = *edit {
            let (seed, year, grown) = (w.params.seed, calendar.days_per_year(), self.params.grown);
            let people = w.province_population();
            self.change(w, |st, watch, _, w| {
                let key = st.people.len() as u64;
                let age = grown + 24.0 * unit(seed, Rng::Lives, day, key, purpose("followed age"));
                follow(st, w, seed, day, year, province, age);
                let n = people.get(province as usize).copied().unwrap_or(0);
                watch.year.entry(province).or_insert_with(|| YearSoFar { since: day, people: n, coins: coins_a_head(w, province as usize, n), ..Default::default() });
            });
        }
        crate::module::Flows::default()
    }

    fn save_state(&self) -> Vec<u8> {
        postcard::to_stdvec(&(&*self.people, &self.watch)).expect("in-memory serialisation")
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let (people, watch): (People, Watch) = postcard::from_bytes(bytes).map_err(|e| format!("lives' state: {e}"))?;
        self.people = Arc::new(people);
        self.watch = watch;
        Ok(())
    }

    fn publish(&self, w: &mut World) {
        w.lives = self.people.clone();
    }

    fn jobs(&self) -> Option<Jobs> {
        None
    }
}

fn realm_lost(w: &World) -> Vec<[u64; 3]> {
    let at = |v: &Vec<u64>, r: usize| v.get(r).copied().unwrap_or(0);
    (0..w.realms.count()).map(|r| [at(&w.realm_lost_sickness, r), at(&w.realm_lost_hunger, r), at(&w.realm_lost_deserted, r)]).collect()
}

impl Lives {
    /// The month's start: what the month that ended did to everyone, then their lives go on.
    fn month(&self, st: &mut People, watch: &mut Watch, w: &World, cx: &Ctx, people: &[u64]) {
        let (seed, day) = (cx.seed, cx.day);
        let year = cx.calendar.days_per_year();
        let months = cx.calendar.months_per_year() as f64;
        let p = &self.params;
        let u = |k: usize, what: &str| unit(seed, Rng::Lives, day, k as u64, purpose(what));
        let grown_born = day as f64 - p.grown * year as f64;

        // Starved and dead of plague in each province where someone lives, over the month.
        let mut month: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
        for person in st.people.iter().filter(|x| x.alive()) {
            let q = person.province;
            if month.contains_key(&q) {
                continue;
            }
            let now = (w.starved_this_year.get(q as usize).copied().unwrap_or(0), w.plague_dead.get(q as usize).copied().unwrap_or(0));
            // A count below last month's was cleared at the year's start.
            let since = |now: u64, then: u64| if now >= then { now - then } else { now };
            let delta = match watch.seen.get(&q) {
                Some(&(s, d)) => (since(now.0, s), since(now.1, d)),
                None => (0, 0),
            };
            watch.seen.insert(q, now);
            month.insert(q, delta);
            if let Some(y) = watch.year.get_mut(&q) {
                y.starved += delta.0;
                y.plague += delta.1;
            }
        }
        watch.seen.retain(|q, _| month.contains_key(q));

        // Soldiers: what their realm's soldiers suffered over the month, a share of those it had.
        let lost_now = realm_lost(w);
        let mut realm_rate: BTreeMap<u32, [f64; 3]> = BTreeMap::new();
        if p.roles.is_some() {
            for k in 0..st.people.len() {
                let x = &st.people[k];
                if !x.alive() || x.army == NONE {
                    continue;
                }
                let a = x.army as usize;
                let r = w.armies.realm.get(a).copied().unwrap_or(NONE);
                let rate = *realm_rate.entry(r).or_insert_with(|| {
                    let had: u64 = (0..watch.army_size.len().min(w.armies.count())).filter(|&j| w.armies.realm[j] == r).map(|j| watch.army_size[j]).sum();
                    let then = watch.realm_lost.get(r as usize).copied().unwrap_or([0; 3]);
                    let now = lost_now.get(r as usize).copied().unwrap_or([0; 3]);
                    std::array::from_fn(|i| share(now[i].saturating_sub(then[i]), had))
                });
                let old = watch.army_size.get(a).copied().unwrap_or(0);
                let new = w.armies.size.get(a).copied().unwrap_or(0);
                let in_battle = watch.battle_dead.get(&x.army).copied().unwrap_or(0.0);
                let frail = x.frailty();
                let (camp, hunger, deserted) = ((rate[0] * frail).min(1.0), (rate[1] * frail).min(1.0), rate[2]);
                let gone = (old as f64 - new as f64 - in_battle).max(0.0) / old.max(1) as f64;
                let sent_home = (gone - rate[0] - rate[1] - rate[2]).max(0.0);
                if u(k, "camp") < camp {
                    die(st, k, day, Death::Camp, grown_born);
                } else if u(k, "hunger") < hunger {
                    die(st, k, day, Death::Hunger, grown_born);
                } else if u(k, "deserted") < deserted {
                    home(st, k, day, true);
                } else if new == 0 || u(k, "sent home") < sent_home {
                    home(st, k, day, false);
                }
            }
        }
        watch.battle_dead.clear();

        // Deaths: of age, and at home of famine and plague by the province's month.
        for k in 0..st.people.len() {
            let x = &st.people[k];
            if !x.alive() {
                continue;
            }
            let frail = x.frailty();
            let age = x.age(day, year);
            let q = x.province;
            let n = people.get(q as usize).copied().unwrap_or(0);
            let (starved, plague) = month.get(&q).copied().unwrap_or((0, 0));
            let away = x.army != NONE;
            let of_age = monthly((death_per_year(age) * p.mortality * frail).min(1.0), months);
            if !away && u(k, "plague") < (share(plague, n) * frail).min(1.0) {
                die(st, k, day, Death::Plague, grown_born);
            } else if !away && u(k, "famine") < (share(starved, n) * frail).min(1.0) {
                die(st, k, day, Death::Famine, grown_born);
            } else if u(k, "age") < of_age {
                die(st, k, day, Death::Age, grown_born);
            }
        }

        // Marriages and children of the followed.
        for k in 0..st.people.len() {
            let x = &st.people[k];
            if !x.followed || !x.alive() {
                continue;
            }
            let age = x.age(day, year);
            let spouse = x.spouse;
            let married = spouse != NONE && st.people[spouse as usize].alive();
            if !married && (p.grown..p.marry_until).contains(&age) && u(k, "marry") < monthly(p.marriage, months) {
                let number = st.people.len() as u32;
                let gap = 12.0 * (u(k, "spouse age") - 0.5);
                let mut s = make(w, seed, day, year, number, x.province, (age + gap).max(p.grown));
                s.female = !x.female;
                (s.culture, s.religion, s.spouse, s.realm) = (x.culture, x.religion, k as u32, x.realm);
                st.people.push(s);
                st.people[k].spouse = number;
                st.people[k].lines.push(Line { day, what: What::Married { spouse: number } });
            } else if married {
                let mother = if x.female { age } else { st.people[spouse as usize].age(day, year) };
                if mother >= p.grown && mother < p.last_child && u(k, "child") < monthly(p.children, months) {
                    let number = st.people.len() as u32;
                    let mut c = make(w, seed, day, year, number, x.province, 0.0);
                    (c.family, c.culture, c.religion, c.parent, c.realm) = (x.family, x.culture, x.religion, k as u32, x.realm);
                    c.wealth = x.wealth;
                    st.people.push(c);
                    st.people[k].children.push(number);
                    st.people[spouse as usize].children.push(number);
                    st.people[k].lines.push(Line { day, what: What::Child { child: number } });
                }
            }
        }

        // Jobs and soldiering.
        if let Some(roles) = &p.roles {
            for k in 0..st.people.len() {
                let x = &st.people[k];
                if !x.followed || !x.alive() || x.army != NONE {
                    continue;
                }
                let age = x.age(day, year);
                let q = x.province as usize;
                // Taken for a soldier: by the share of the province's people its companies took.
                if (roles.soldier_from..roles.soldier_until).contains(&age) && age >= p.grown {
                    let grew: Vec<(usize, u64)> = (0..w.armies.count())
                        .filter(|&a| w.armies.home[a] == x.province)
                        .map(|a| (a, w.armies.size[a].saturating_sub(watch.army_size.get(a).copied().unwrap_or(0))))
                        .filter(|&(_, g)| g > 0)
                        .collect();
                    let taken: u64 = grew.iter().map(|g| g.1).sum();
                    let n = people.get(q).copied().unwrap_or(0) as f64 * roles.soldier_share;
                    if taken > 0 && u(k, "taken") < (taken as f64 / n.max(taken as f64)).min(1.0) {
                        let mut pick = u(k, "company") * taken as f64;
                        let a = grew.iter().find(|g| {
                            if pick < g.1 as f64 {
                                return true;
                            }
                            pick -= g.1 as f64;
                            false
                        });
                        let a = a.unwrap_or(grew.last().expect("some grew")).0;
                        let guard = w.armies.role[a] != crate::warfare::FIELD;
                        let x = &mut st.people[k];
                        x.army = a as u32;
                        x.lines.push(Line { day, what: What::Taken { army: a as u32, realm: w.armies.realm[a], guard } });
                        continue;
                    }
                }
                if age < p.grown {
                    continue;
                }
                let open = self.open_jobs(w, q);
                let has = open.iter().any(|j| j.name == x.job);
                let change = has && open.len() > 1 && u(k, "new job") < monthly(roles.job_change, months);
                if !has || change {
                    let total: f64 = open.iter().filter(|j| !change || j.name != x.job).map(|j| j.holders).sum();
                    let mut pick = u(k, "which job") * total;
                    let job = open.iter().filter(|j| !change || j.name != x.job).find(|j| {
                        if pick < j.holders {
                            return true;
                        }
                        pick -= j.holders;
                        false
                    });
                    if let Some(job) = job.or(open.last()) {
                        let x = &mut st.people[k];
                        if x.job != job.name {
                            x.job = job.name.clone();
                            (x.job_since, x.master) = (day, false);
                            x.lines.push(Line { day, what: What::Job { job: x.job.clone() } });
                        }
                    }
                } else if !x.master {
                    let years = (day - x.job_since) as f64 / year as f64;
                    if years * (0.5 + x.talent as f64 / 100.0) >= roles.master_years {
                        let x = &mut st.people[k];
                        x.master = true;
                        x.lines.push(Line { day, what: What::Master { job: x.job.clone() } });
                    }
                }
            }
        }

        // The realm their province belongs to.
        for x in st.people.iter_mut().filter(|x| x.alive()) {
            let r = owner(w, x.province);
            if r != x.realm {
                if x.followed {
                    x.lines.push(Line { day, what: What::Realm { from: x.realm, to: r } });
                }
                x.realm = r;
            }
        }

        // The year that ended, where each followed person lives.
        // Told on the year's last day. "lives" tells only a year that stood out; the diary every one.
        if cx.calendar.is_year_start(day) {
            let mut lines: BTreeMap<u32, What> = BTreeMap::new();
            for (&q, y) in watch.year.iter().filter(|(_, y)| y.since < day) {
                let n = people.get(q as usize).copied().unwrap_or(0);
                let coins = y.coins.zip(coins_a_head(w, q as usize, n));
                let year = What::Year { people_was: y.people, people: n, starved: y.starved, plague: y.plague, coins };
                if p.roles.is_none() || !quiet(&year) {
                    lines.insert(q, year);
                }
            }
            for x in st.people.iter_mut().filter(|x| x.followed && x.alive()) {
                if let Some(l) = lines.get(&x.province) {
                    x.lines.push(Line { day: day - 1, what: l.clone() });
                }
            }
            watch.year.clear();
        }
        for x in st.people.iter().filter(|x| x.followed && x.alive()) {
            let q = x.province;
            let n = people.get(q as usize).copied().unwrap_or(0);
            watch.year.entry(q).or_insert_with(|| YearSoFar { since: day, people: n, coins: coins_a_head(w, q as usize, n), ..Default::default() });
        }
        watch.year.retain(|q, _| st.people.iter().any(|x| x.followed && x.alive() && x.province == *q));
        watch.army_size = w.armies.size.clone();
        watch.realm_lost = lost_now;
    }
}

/// Person `k` dies today: their story ends, their followed kin hear of it, and a followed person's
/// story carries on with their eldest grown child alive (born on or before day `grown_born`).
fn die(st: &mut People, k: usize, day: u64, how: Death, grown_born: f64) {
    let x = &mut st.people[k];
    x.died = day;
    x.how = Some(how);
    x.army = NONE;
    if x.followed {
        x.lines.push(Line { day, what: What::Died { how } });
    }
    let (spouse, parent, children, followed) = (x.spouse, x.parent, x.children.clone(), x.followed);
    for kin in children.iter().copied().chain([spouse, parent]) {
        if let Some(o) = st.people.get_mut(kin as usize)
            && o.followed
            && o.alive()
        {
            o.lines.push(Line { day, what: What::KinDied { who: k as u32 } });
        }
    }
    if followed {
        st.heir_takes_up(k, day, grown_born);
    }
}

impl People {
    /// The eldest child of `k` alive and grown (born on or before day `grown_born`) carries the
    /// story on, if there is one.
    fn heir_takes_up(&mut self, k: usize, day: u64, grown_born: f64) {
        let heir = self.people[k].children.iter().map(|&c| c as usize).filter(|&c| {
            let c = &self.people[c];
            c.alive() && !c.followed && c.born <= grown_born
        });
        if let Some(c) = heir.min_by(|&a, &b| self.people[a].born.total_cmp(&self.people[b].born)) {
            let c = &mut self.people[c];
            c.followed = true;
            c.lines.push(Line { day, what: What::CarriedOn { from: k as u32 } });
        }
    }
}

/// How much a province's people may change in a year, as a share, and its coins a head, before the
/// year is told as standing out.
const PEOPLE_CHANGE: f64 = 0.02;
const COINS_CHANGE: f64 = 0.1;

/// Whether a year line has nothing that stood out: people about the same, nobody starved or dead of
/// plague, coins a head about the same.
fn quiet(year: &What) -> bool {
    let What::Year { people_was, people, starved, plague, coins } = year else { return false };
    let change = share(*people, (*people_was).max(1)) - 1.0;
    let coins = coins.map_or(0.0, |(was, now)| if was > 0.0 { (now / was - 1.0) as f64 } else { 0.0 });
    change.abs() <= PEOPLE_CHANGE && *starved == 0 && *plague == 0 && coins.abs() <= COINS_CHANGE
}

/// Person `k` is back home from soldiering.
fn home(st: &mut People, k: usize, day: u64, deserted: bool) {
    let x = &mut st.people[k];
    x.army = NONE;
    if x.followed {
        x.lines.push(Line { day, what: What::Home { deserted } });
    }
}

/// The realm person `x` belongs to: their company's while a soldier, else their province's.
fn realm_of(w: &World, x: &Person) -> u32 {
    if x.army != NONE {
        return w.armies.realm.get(x.army as usize).copied().unwrap_or(NONE);
    }
    owner(w, x.province)
}

/// The share of province `p`'s people of culture (or, `faith`, of faith) `to`.
fn share_following(w: &World, p: u32, to: u32, faith: bool) -> f64 {
    let pops = &w.pops;
    let (mut of, mut all) = (0u64, 0u64);
    for j in 0..pops.size.len() {
        if pops.province[j] == p {
            all += pops.size[j];
            let k = if faith { pops.religion[j] as u32 } else { pops.culture[j] as u32 };
            if k == to {
                of += pops.size[j];
            }
        }
    }
    share(of, all)
}

/// Whether an event of the world touches anyone followed (alive): a battle their company fought, a
/// change in the province they live in, or news of their realm.
fn touches(st: &People, w: &World, e: &Event) -> bool {
    let mut living = st.people.iter().filter(|x| x.followed && x.alive());
    match e {
        Event::Battle { province, .. } => living.any(|x| x.army != NONE && w.armies.province.get(x.army as usize) == Some(province)),
        Event::Uprising { province, .. }
        | Event::Occupied { province, .. }
        | Event::Converted { province, .. }
        | Event::Assimilated { province, .. }
        | Event::Outbreak { province, .. }
        | Event::PlagueArrived { province, .. }
        | Event::WoodsCleared { province, .. }
        | Event::FisheryCollapsed { province } => living.any(|x| x.army == NONE && x.province == *province),
        Event::WarDeclared { attacker, defender, .. } | Event::Peace { attacker, defender, .. } => living.any(|x| [*attacker, *defender].contains(&realm_of(w, x))),
        Event::Succession { realm, .. } | Event::Discovery { realm, .. } => living.any(|x| realm_of(w, x) == *realm),
        _ => false,
    }
}

/// What an event of the world does to the followed it touches.
fn event(st: &mut People, watch: &mut Watch, w: &World, cx: &Ctx, e: &Event, params: &LifeParams, roles: &RoleParams) {
    let (seed, day) = (cx.seed, cx.day);
    let grown_born = day as f64 - params.grown * cx.calendar.days_per_year() as f64;
    let u = |k: usize, what: &str| unit(seed, Rng::Lives, day, k as u64, purpose(what));
    let at_home = |x: &Person, p: u32| x.followed && x.alive() && x.army == NONE && x.province == p;
    match e {
        Event::Battle { province, winner, sides, brought, .. } => {
            for k in 0..st.people.len() {
                let x = &st.people[k];
                if !x.followed || !x.alive() || x.army == NONE {
                    continue;
                }
                let a = x.army as usize;
                if w.armies.province.get(a) != Some(province) {
                    continue;
                }
                let r = w.armies.realm[a];
                let dead = sides.iter().filter(|s| s.0 == r).map(|s| s.1).sum::<u64>();
                let had = brought.iter().filter(|s| s.0 == r).map(|s| s.1).sum::<u64>();
                if had == 0 {
                    continue;
                }
                let killed = share(dead, had);
                let now = w.armies.size[a] as f64;
                *watch.battle_dead.entry(x.army).or_insert(0.0) += now * killed / (1.0 - killed).max(0.01);
                let won = *winner == r;
                let (frail, bold) = (x.frailty(), x.boldness as f64 / 100.0);
                if u(k, "battle") < (killed * frail).min(1.0) {
                    st.people[k].lines.push(Line { day, what: What::Battle { province: *province, won, wounded: false } });
                    die(st, k, day, Death::Battle, grown_born);
                    continue;
                }
                let wounded = u(k, "wounded") < (killed * roles.wounded * (0.5 + bold)).min(1.0);
                let x = &mut st.people[k];
                x.lines.push(Line { day, what: What::Battle { province: *province, won, wounded } });
                if won && u(k, "praised") < roles.praised * bold {
                    x.lines.push(Line { day, what: What::Praised { province: *province } });
                }
            }
        }
        Event::Uprising { province, .. } => {
            for x in st.people.iter_mut().filter(|x| at_home(x, *province)) {
                x.lines.push(Line { day, what: What::Rising { province: *province } });
            }
        }
        Event::Occupied { province, by } => {
            for x in st.people.iter_mut().filter(|x| at_home(x, *province)) {
                x.lines.push(Line { day, what: What::Occupied { province: *province, by: *by } });
            }
        }
        Event::WarDeclared { attacker, defender, .. } => {
            for k in 0..st.people.len() {
                let r = realm_of(w, &st.people[k]);
                let x = &mut st.people[k];
                if x.followed && x.alive() && r != NONE && (r == *attacker || r == *defender) {
                    let against = if r == *attacker { *defender } else { *attacker };
                    x.lines.push(Line { day, what: What::War { against } });
                }
            }
        }
        Event::Peace { attacker, defender, winner, .. } => {
            for k in 0..st.people.len() {
                let r = realm_of(w, &st.people[k]);
                let x = &mut st.people[k];
                if x.followed && x.alive() && r != NONE && (r == *attacker || r == *defender) {
                    let against = if r == *attacker { *defender } else { *attacker };
                    let won = (*winner != NONE).then_some(*winner == r);
                    x.lines.push(Line { day, what: What::Peace { against, won } });
                }
            }
        }
        Event::Succession { realm, ruler, .. } => {
            for k in 0..st.people.len() {
                let r = realm_of(w, &st.people[k]);
                let x = &mut st.people[k];
                if x.followed && x.alive() && r == *realm {
                    x.lines.push(Line { day, what: What::NewRuler { realm: *realm, ruler: *ruler } });
                }
            }
        }
        Event::Discovery { realm, tech, .. } => {
            for k in 0..st.people.len() {
                let r = realm_of(w, &st.people[k]);
                let x = &mut st.people[k];
                if x.followed && x.alive() && r == *realm && x.job == crate::research::SCHOLAR {
                    x.lines.push(Line { day, what: What::Discovery { realm: *realm, tech: *tech } });
                }
            }
        }
        Event::Converted { province, from, to } | Event::Assimilated { province, from, to } => {
            let faith = matches!(e, Event::Converted { .. });
            let chance = (roles.swayed * share_following(w, *province, *to, faith)).min(1.0);
            for k in 0..st.people.len() {
                let x = &st.people[k];
                let held = if faith { x.religion } else { x.culture };
                if !at_home(x, *province) || held != *from || u(k, if faith { "converted" } else { "assimilated" }) >= chance {
                    continue;
                }
                let x = &mut st.people[k];
                if faith {
                    x.religion = *to;
                    x.lines.push(Line { day, what: What::Faith { to: *to } });
                } else {
                    x.culture = *to;
                    x.lines.push(Line { day, what: What::Culture { to: *to } });
                }
            }
        }
        Event::Outbreak { outbreak, province } | Event::PlagueArrived { outbreak, province, .. } => {
            for x in st.people.iter_mut().filter(|x| at_home(x, *province)) {
                x.lines.push(Line { day, what: What::PlagueCame { outbreak: *outbreak } });
            }
        }
        Event::WoodsCleared { province, .. } => {
            for x in st.people.iter_mut().filter(|x| at_home(x, *province)) {
                x.lines.push(Line { day, what: What::WoodsCleared });
            }
        }
        Event::FisheryCollapsed { province } => {
            for x in st.people.iter_mut().filter(|x| at_home(x, *province)) {
                x.lines.push(Line { day, what: What::FishGone });
            }
        }
        _ => {}
    }
}

/// Names for telling stories: worked out once for many lines.
pub struct Names {
    seed: u64,
    realms: Vec<String>,
    cultures: Vec<String>,
    faiths: Vec<String>,
}

impl Names {
    pub fn new(w: &World) -> Names {
        let seed = w.params.seed;
        Names { seed, realms: crate::realms::names(seed, w.realms.count()), cultures: crate::culture::names(seed, w.cultures.count()), faiths: crate::religion::names(seed, w.religions.count()) }
    }

    fn realm(&self, r: u32) -> String {
        self.realms.get(r as usize).cloned().unwrap_or_else(|| "no realm".into())
    }
}

impl People {
    /// "Merwen Aldric".
    pub fn name(&self, seed: u64, k: usize) -> String {
        let x = &self.people[k];
        format!("{} {}", crate::characters::name(seed, x.name), crate::characters::name(seed, x.family))
    }

    /// What person `k` is now, in a few words: "a master smith, 34", "a soldier of Tholia, 22",
    /// "died of plague aged 51".
    pub fn summary(&self, w: &World, names: &Names, k: usize, day: u64, days_per_year: u32) -> String {
        let x = &self.people[k];
        let age = x.age(day, days_per_year).floor();
        if !x.alive() {
            return format!("died {} aged {age}", x.how.map_or("", Death::describe));
        }
        if x.army != NONE {
            let r = w.armies.realm.get(x.army as usize).copied().unwrap_or(NONE);
            return format!("a soldier of {}, {age}", names.realm(r));
        }
        let job = if x.job.is_empty() { if age < 16.0 { CHILD.to_string() } else { "of no trade yet".into() } } else if x.master { format!("a master {}", x.job) } else { format!("a {}", x.job) };
        format!("{job}, {age}")
    }

    /// Line `line` of person `k`'s story in words, after its year: "1043: married Dorana Wyn".
    pub fn tell(&self, w: &World, names: &Names, k: usize, line: &Line, calendar: &crate::calendar::Calendar) -> String {
        let x = &self.people[k];
        let (he, his) = if x.female { ("she", "her") } else { ("he", "his") };
        let who = |o: u32| if (o as usize) < self.people.len() { self.name(names.seed, o as usize) } else { "someone".into() };
        let place = |p: u32| format!("province {p}");
        let year = calendar.date(line.day).year;
        let text = match &line.what {
            What::Begun { age } => format!("followed from here, aged {:.0}, in {} ({})", age.floor(), place(x.province), names.realm(owner(w, x.province))),
            What::CarriedOn { from } => format!("{his} parent {} died; {his} story goes on", who(*from)),
            What::Married { spouse } => format!("married {}", who(*spouse)),
            What::Child { child } => format!("a child was born: {}", who(*child)),
            What::KinDied { who: o } => {
                let tie = if *o == x.spouse { if x.female { "husband" } else { "wife" } } else if *o == x.parent { "parent" } else { "child" };
                let how = self.people.get(*o as usize).and_then(|p| p.how).map_or("", Death::describe);
                format!("{his} {tie} {} died {how}", who(*o))
            }
            What::Died { how } => format!("{he} died {}", how.describe()),
            What::Year { people_was, people, starved, plague, coins } => {
                // Only what stood out: a quiet year is told as one.
                let mut parts = Vec::new();
                let change = share(*people, (*people_was).max(1)) - 1.0;
                if change > PEOPLE_CHANGE {
                    parts.push(format!("the province grew to {people} people (+{:.0}%)", change * 100.0));
                } else if change < -PEOPLE_CHANGE {
                    parts.push(format!("the province shrank to {people} people ({:.0}%)", change * 100.0));
                }
                if *starved > 0 {
                    parts.push(format!("{starved} starved"));
                }
                if *plague > 0 {
                    parts.push(format!("{plague} died of plague"));
                }
                if let Some((was, now)) = coins {
                    let c = if *was > 0.0 { now / was - 1.0 } else { 0.0 };
                    let mine = now * x.wealth;
                    if c > COINS_CHANGE as f32 {
                        parts.push(format!("a good year: {he} has about {mine:.0} coins (+{:.0}%)", c * 100.0));
                    } else if c < -COINS_CHANGE as f32 {
                        parts.push(format!("a lean year: {he} has about {mine:.0} coins ({:.0}%)", c * 100.0));
                    }
                }
                if parts.is_empty() { "a quiet year".into() } else { parts.join("; ") }
            }
            What::Realm { from, to } if *to == NONE => format!("{} no longer rules {}", names.realm(*from), place(x.province)),
            What::Realm { from, to } if *from == NONE => format!("{} now belongs to {}", place(x.province), names.realm(*to)),
            What::Realm { from, to } => format!("{} passed from {} to {}", place(x.province), names.realm(*from), names.realm(*to)),
            What::Job { job } => format!("became a {job}"),
            What::Master { job } => format!("became a master {job}"),
            What::Taken { realm, guard: true, .. } => format!("taken for the guard of {} at home", names.realm(*realm)),
            What::Taken { realm, guard: false, .. } => format!("taken for a soldier of {}", names.realm(*realm)),
            What::Battle { province, won, wounded } => {
                let side = if *won { "won" } else { "lost" };
                let hurt = if *wounded { ", and was wounded" } else { "" };
                format!("fought at {} and {side}{hurt}", place(*province))
            }
            What::Praised { .. } => "praised for bravery".into(),
            What::Home { deserted: true } => "deserted and came home".into(),
            What::Home { deserted: false } => "sent home from soldiering".into(),
            What::Rising { .. } => format!("{} rose up", place(x.province)),
            What::Occupied { by, .. } => format!("{} was occupied by {}", place(x.province), names.realm(*by)),
            What::War { against } => format!("war with {}", names.realm(*against)),
            What::Peace { against, won: Some(true) } => format!("peace with {}, won", names.realm(*against)),
            What::Peace { against, won: Some(false) } => format!("peace with {}, lost", names.realm(*against)),
            What::Peace { against, won: None } => format!("peace with {}", names.realm(*against)),
            What::NewRuler { realm, ruler } => {
                let r = if (*ruler as usize) < w.characters.count() { crate::characters::title(w, *ruler) } else { format!("character {ruler}") };
                format!("{r} now rules {}", names.realm(*realm))
            }
            What::Discovery { tech, .. } => format!("the realm's scholars learned {}", w.techs.get(*tech as usize).cloned().unwrap_or_else(|| format!("tech {tech}"))),
            What::Faith { to } => format!("took up the {} faith", names.faiths.get(*to as usize).cloned().unwrap_or_else(|| format!("faith {to}"))),
            What::Culture { to } => format!("took up the ways of the {}", names.cultures.get(*to as usize).cloned().unwrap_or_else(|| format!("culture {to}"))),
            What::PlagueCame { outbreak } => format!("{} reached {}", crate::disease::name(names.seed, *outbreak), place(x.province)),
            What::WoodsCleared => format!("the woods of {} were cleared", place(x.province)),
            What::FishGone => format!("the fish of {} gave out", place(x.province)),
        };
        format!("{year}: {text}")
    }
}
