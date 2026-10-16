//! Warfare (step 7): soldiers taken from the pops, standing in a province, serving a realm.
//!
//! Army numbers are never reused: a disbanded army stays as an entry of no soldiers. Soldiers are
//! people, counted in the people total with the pops (see [`crate::fields::Quantity::fields`]).
//!
//! Warfare v1, "troops" ([`Troops`]), raises them. An army entry is a *company*: soldiers of one
//! kind (a leaf of the troop tree, `data/troops.toml`) from one province, their home, serving the
//! realm that held it when they were raised. Companies stand at home for now; moving and fighting
//! is the next piece's, which will group companies that march together.
//!
//! Each month, for every province a realm holds:
//! 1. Companies whose realm no longer holds their home (it changed hands, or the realm was
//!    dissolved) go home, all of them.
//! 2. The soldiers wanted are a share of the province's people (its soldiers included): a small one
//!    in peace, a bigger one while its realm is at war with anyone. A tenth or more below that it
//!    raises up to it, at most a set part of the war-time number a month, shared between the kinds by how well its land suits
//!    each (land with ore for heavy infantry, grazing for cavalry, a wooded shore for ships). Well
//!    over it (after a peace), the extra go home. A province an enemy occupies, or in uprising,
//!    raises nobody.
//! 3. The companies standing in a province ask it together for a month of food, at the priority
//!    people ask with, so soldiers and civilians share a shortage alike. One request a province, not
//!    one a company, since each costs the engine as much as a company's whole month; the module
//!    keeps who asked for how many until the grant comes (its private state).
//!
//! People are asked of Population by province, and booked into the company the next day, when the
//! grant comes. Then, under an economy with coins (markets), the goods each new soldier needs are
//! asked for (tools, timber), sized to the recruits who actually came; a recruit whose goods the
//! province can't supply joins the province's levies instead. And the realm pays a bounty for each
//! recruit, moved from its treasury into the province: asked at low priority, the month's second
//! day, so it never cuts into the realm's own spending. Under Economics v0, which keeps only food,
//! the land alone decides the kinds.
//!
//! When a company's food comes, part of its unfed desert home (sent to Population as
//! [`Event::Arrived`]), the unfed who stay die as hungry civilians do, and soldiers die of age and
//! illness at the civilians' rate. Soldiers' hunger is not a famine: it adds nothing to unrest.
//!
//! Nothing reads the army fields yet, so the realms and markets modules don't depend on them (they
//! may not: warfare reads theirs).

use crate::diplomacy;
use crate::events::{Event, EventKind};
use crate::fields::{known, Goods, Quantity, Tier};
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::requests::{split_proportional, Cause, Dest, Priority, Request};
use crate::rng::{purpose, round_stochastic, Module as Rng};
use crate::taxonomy::{NodeId, Taxonomy, TreeDef};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The parameters shipped in `data/troops.toml`.
pub const STANDARD: &str = include_str!("../data/troops.toml");

/// One entry per army, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Armies {
    /// The realm it serves (field `army.realm`).
    pub realm: Vec<u32>,
    /// The province it stands in (field `army.province`).
    pub province: Vec<u32>,
    /// Soldiers (field `army.size`).
    pub size: Vec<u64>,
    /// The kind of its soldiers: a node of the troop tree (field `army.kind`).
    pub kind: Vec<u16>,
    /// The province its soldiers came from, where they go home to (field `army.home`).
    pub home: Vec<u32>,
}

impl Armies {
    pub fn count(&self) -> usize {
        self.size.len()
    }

    /// A new army of `size` soldiers of `kind`, from `home` and standing there, serving `realm`.
    pub fn push(&mut self, realm: u32, home: u32, kind: u16, size: u64) -> u32 {
        let a = self.count() as u32;
        self.realm.push(realm);
        self.province.push(home);
        self.size.push(size);
        self.kind.push(kind);
        self.home.push(home);
        a
    }
}

/// What part of a province's land suits a kind of troops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Land {
    /// Any land: every province alike.
    #[default]
    Any,
    /// The share of its land with ore in reach.
    Ore,
    /// The share of its food better from herds than fields.
    Grazing,
    /// The share of its land under woods.
    Woodland,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KindParams {
    /// How much of a province's raising goes to this kind, times the share of its land that suits it.
    pub weight: f64,
    #[serde(default)]
    pub land: Land,
    /// Only provinces with a shore raise it.
    #[serde(default)]
    pub coast: bool,
    /// Under an economy that keeps goods, what each soldier raised uses up, and how much.
    #[serde(default)]
    pub good: Option<String>,
    #[serde(default)]
    pub per_soldier: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TroopParams {
    /// Share of a province's people (its soldiers included) under arms in peace.
    pub peace_share: f64,
    /// The same while its realm is at war with anyone.
    pub war_share: f64,
    /// A province raises at most the war-time number divided by this a month.
    pub months_to_raise: f64,
    /// Soldiers go home only once a province has this share more than it wants, and then down to
    /// what it wants.
    pub disband_slack: f64,
    /// A province raises only once it has less than this share of what it wants, and then up to
    /// what it wants.
    pub raise_below: f64,
    /// Share of a month's unfed soldiers who desert home.
    pub desert_share: f64,
    /// Soldiers' deaths of age and illness, a year.
    pub deaths_per_year: f64,
    /// Share of the unfed who stay that die, per 30 days of hunger.
    pub starvation_deaths: f64,
    /// Coins a realm pays a province for each soldier raised there, where the world keeps coins.
    pub bounty: f64,
    /// The class of the goods tree whose goods are coins.
    pub money: String,
    /// The kind raised in place of one whose goods the province can't supply. It needs none.
    pub fallback: String,
    /// The troop tree: its leaves are the kinds.
    pub tree: TreeDef,
    /// Each kind, by its name in the tree.
    pub kinds: BTreeMap<String, KindParams>,
}

impl Default for TroopParams {
    fn default() -> Self {
        toml::from_str(STANDARD).expect("bundled troop parameters are valid")
    }
}

/// A kind, ready to use.
#[derive(Clone, Debug)]
struct Kind {
    node: NodeId,
    weight: f64,
    land: Land,
    coast: bool,
    good: Option<(String, f64)>,
}

/// The parameters checked and turned into the troop tree and its kinds.
#[derive(Clone, Debug)]
struct Resolved {
    /// Every leaf of the tree, in tree order.
    kinds: Vec<Kind>,
    /// The fallback kind's place in `kinds`.
    fallback: usize,
}

impl TroopParams {
    fn resolve(&self) -> Result<Resolved, String> {
        let tree = Taxonomy::named(&self.tree, "troop tree")?;
        let leaves = tree.leaves_under(0);
        if let Some(name) = self.kinds.keys().find(|n| tree.find(n).is_none_or(|id| !leaves.contains(&id))) {
            return Err(format!("troops: {name} is not a kind (a leaf) of the troop tree"));
        }
        let mut kinds = Vec::new();
        for &leaf in &leaves {
            let name = tree.name(leaf);
            let k = self.kinds.get(name).ok_or_else(|| format!("troops: the kind {name} has no table of its own"))?;
            if !(k.weight.is_finite() && k.weight >= 0.0) {
                return Err(format!("troops: {name}'s weight is zero or more"));
            }
            if k.good.is_some() && !(k.per_soldier.is_finite() && k.per_soldier > 0.0) {
                return Err(format!("troops: {name} uses more than none of its good a soldier"));
            }
            kinds.push(Kind { node: leaf, weight: k.weight, land: k.land, coast: k.coast, good: k.good.clone().map(|g| (g, k.per_soldier)) });
        }
        let fallback = kinds.iter().position(|k| tree.name(k.node) == self.fallback).ok_or_else(|| format!("troops: the fallback {} is not a kind", self.fallback))?;
        if kinds[fallback].good.is_some() {
            return Err(format!("troops: the fallback {} must need no goods", self.fallback));
        }
        let share = |x: f64| (0.0..=1.0).contains(&x);
        if !(share(self.peace_share) && share(self.war_share) && self.peace_share <= self.war_share) {
            return Err("troops: the shares under arms go from 0 to 1, the war-time one no smaller".into());
        }
        if !(self.months_to_raise.is_finite() && self.months_to_raise >= 1.0) {
            return Err("troops: raising takes a month or more".into());
        }
        if !(self.disband_slack.is_finite() && self.disband_slack >= 0.0) {
            return Err("troops: the slack before soldiers go home is zero or more".into());
        }
        if !(0.0..=1.0).contains(&self.raise_below) {
            return Err("troops: a province raises below a share of what it wants from 0 to 1".into());
        }
        if ![self.desert_share, self.deaths_per_year, self.starvation_deaths].iter().all(|&x| share(x)) {
            return Err("troops: desertion and deaths go from 0 to 1".into());
        }
        if !(self.bounty.is_finite() && self.bounty >= 0.0) {
            return Err("troops: the bounty is zero coins or more".into());
        }
        Ok(Resolved { kinds, fallback })
    }
}

pub struct Troops {
    pub params: TroopParams,
    /// Whether the world keeps coins and the other goods (markets): then soldiers use up goods and
    /// realms pay a bounty.
    coins: bool,
    /// The parameters resolved, worked out the first time they are needed.
    resolved: Option<Resolved>,
    /// The companies whose food is asked for, and their soldiers then, by the province they stand
    /// in: kept from the month's first day until the grant comes (saved).
    eating: BTreeMap<u32, Vec<(u32, u64)>>,
}

impl Troops {
    /// Troops that use up goods and are paid for if `coins` (the world keeps coins), and are raised
    /// by the land alone otherwise. The two differ only in their manifests, with the same version
    /// name and no private state, so saves and scenario files don't depend on which it is; the
    /// configuration picks by the economics chosen, as it does for realms' taxes.
    pub fn new(params: TroopParams, coins: bool) -> Troops {
        Troops { params, coins, resolved: None, eating: BTreeMap::new() }
    }

    fn resolved(&mut self) -> Resolved {
        if self.resolved.is_none() {
            self.resolved = Some(self.params.resolve().expect("checked when the world was made"));
        }
        self.resolved.clone().expect("just set")
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Warfare,
    version: "troops",
    tier: Tier::Army,
    cadence: Cadence::Monthly,
    reads: &[
        "pop.province",
        "pop.size",
        "province.owner",
        "province.occupier",
        "province.uprising",
        "realm.capital",
        "war.attacker",
        "war.defender",
        "war.ended",
        "province.area_km2",
        "province.food_capacity",
        "province.grazing_capacity",
        "province.forest_km2",
        "province.coast_km",
        "province.ore_km2",
    ],
    owns: &["army.realm", "army.province", "army.size", "army.kind", "army.home"],
    requests: &["pop.size", "province.stock"],
    asks: &["food"],
    moves: &[],
    // Soldiers serve realms and are taken from the people.
    requires: &["realm.capital", "pop.size"],
    emits: &[EventKind::Arrived],
    handles: &[EventKind::Granted],
    state_schema: "eating-1",
    goods: &[],
};

/// The same version where the world keeps coins and goods: soldiers use up goods, and realms pay a
/// bounty from their treasuries into the provinces.
static V1_COINS: Manifest = Manifest {
    requests: &["pop.size", "province.stock", "realm.stock"],
    asks: &["food", "tools", "timber", "money"],
    moves: &[("realm.stock", "province.stock")],
    ..V1
};

impl Module for Troops {
    fn manifest(&self) -> &'static Manifest {
        if self.coins { &V1_COINS } else { &V1 }
    }

    fn check(&self, goods: &Goods) -> Result<(), String> {
        let r = self.params.resolve()?;
        if self.coins {
            let wanted = r.kinds.iter().filter_map(|k| k.good.as_ref().map(|g| g.0.as_str())).chain([self.params.money.as_str()]);
            for name in wanted {
                let kept = goods.tree.find(name).and_then(|n| goods.columns_under(n)).is_some_and(|c| !c.is_empty());
                if !kept {
                    return Err(format!("troops: {name} is not a good this world keeps on its own"));
                }
            }
        }
        Ok(())
    }

    fn tick(&mut self, w: &mut World, cx: &mut Ctx) {
        if w.realms.count() == 0 {
            return;
        }
        let res = self.resolved();
        let p = &self.params;
        // Companies whose realm no longer holds their home go home.
        for a in 0..w.armies.count() {
            let (realm, home, size) = (w.armies.realm[a], w.armies.home[a], w.armies.size[a]);
            if size > 0 && (w.owner.get(home as usize) != Some(&realm) || !w.realms.alive(realm)) {
                if cx.traced(home) {
                    cx.note(format!("company {a} ({size} soldiers) goes home: its realm {realm} no longer holds the province"));
                }
                go_home(w, cx, a, size, Cause::Returned);
            }
        }

        let n = w.provinces.count;
        let people = w.province_population();
        let enemies = diplomacy::enemies(w);
        let mut companies: Vec<Vec<u32>> = vec![Vec::new(); n];
        for a in 0..w.armies.count() {
            if let Some(v) = companies.get_mut(w.armies.home[a] as usize) {
                v.push(a as u32);
            }
        }
        let pop_size = known("pop.size");
        for prov in 0..n {
            let owner = w.owner[prov];
            if owner == NONE || !w.realms.alive(owner) {
                continue;
            }
            let mut mine: Vec<u32> = companies[prov].iter().copied().filter(|&a| w.armies.realm[a as usize] == owner).collect();
            let current: u64 = mine.iter().map(|&a| w.armies.size[a as usize]).sum();
            let total = people[prov] + current;
            if total == 0 {
                continue;
            }
            let war = enemies.get(owner as usize).is_some_and(|e| !e.is_empty());
            let wanted = (total as f64 * if war { p.war_share } else { p.peace_share }).round() as u64;
            if current > wanted && current as f64 > wanted as f64 * (1.0 + p.disband_slack) {
                let sizes: Vec<u64> = mine.iter().map(|&a| w.armies.size[a as usize]).collect();
                let ids: Vec<u64> = mine.iter().map(|&a| a as u64).collect();
                for (&a, part) in mine.iter().zip(split_proportional(current - wanted, &sizes, &ids)) {
                    if part > 0 {
                        go_home(w, cx, a as usize, part, Cause::Returned);
                    }
                }
                if cx.traced(prov as u32) {
                    cx.note(format!("{current} soldiers, {wanted} wanted (realm {owner} {}): {} go home", if war { "at war" } else { "at peace" }, current - wanted));
                }
            } else if (current as f64) < wanted as f64 * p.raise_below && people[prov] > 0 && w.occupier[prov] == NONE && w.uprising[prov] == NONE {
                let step = ((total as f64 * p.war_share / p.months_to_raise).ceil() as u64).max(1);
                let raise = (wanted - current).min(step);
                let weights = kind_weights(w, prov, &res);
                let order: Vec<u64> = (0..res.kinds.len() as u64).collect();
                let parts = split_proportional(raise, &weights, &order);
                // The province's company of a kind, made if it has none (and then found by the next
                // kind that asks for it).
                let company = |w: &mut World, mine: &mut Vec<u32>, node: NodeId| -> u32 {
                    if let Some(a) = mine.iter().copied().find(|&a| w.armies.kind[a as usize] == node) {
                        return a;
                    }
                    let a = w.armies.push(owner, prov as u32, node, 0);
                    mine.push(a);
                    a
                };
                for (k, &part) in parts.iter().enumerate() {
                    if part == 0 {
                        continue;
                    }
                    let a = company(w, &mut mine, res.kinds[k].node);
                    // Recruits a kind's goods can't equip join the fallback company: made now, so the
                    // grant knows where they go.
                    let fallback = if self.coins && res.kinds[k].good.is_some() { company(w, &mut mine, res.kinds[res.fallback].node) } else { a };
                    let tag = (a as u64) | (fallback as u64) << 32;
                    cx.request(pop_size, prov as u32, part, Priority::Normal, Cause::Raised, tag);
                }
                if cx.traced(prov as u32) {
                    cx.note(format!(
                        "{current} soldiers, {wanted} wanted (realm {owner} {}): raising {raise}, by kind {parts:?}",
                        if war { "at war" } else { "at peace" }
                    ));
                }
            }
        }

        // Soldiers eat where they stand, a ration a day, asked like the people's own food: by the
        // companies of each province together.
        let days = cx.calendar.def.months[cx.calendar.date(cx.day).month].days as u64;
        let stock = known("province.stock");
        let food = cx.goods.tree.find("food").expect("checked at configuration: the goods tree has food");
        self.eating.clear();
        for a in 0..w.armies.count() {
            let size = w.armies.size[a];
            if size > 0 {
                self.eating.entry(w.armies.province[a]).or_default().push((a as u32, size));
            }
        }
        for (&at, companies) in &self.eating {
            let soldiers: u64 = companies.iter().map(|c| c.1).sum();
            cx.ask_good(stock, at, food, soldiers * days, Priority::Essential, Cause::Eating, at as u64);
        }
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.eating)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.eating = postcard::from_bytes(bytes).map_err(|e| format!("Warfare state: {e}"))?;
        Ok(())
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        let Event::Granted { request: r, granted } = *e else { return };
        match r.cause {
            Cause::Raised => self.recruited(w, cx, &r, granted),
            Cause::Equipped => self.equipped(w, cx, &r, granted),
            Cause::Eating => self.fed(w, cx, &r, granted),
            // The bounty is paid: nothing follows.
            _ => {}
        }
    }
}

impl Troops {
    /// The goods for a company's recruits came, in full or not: those left without join the
    /// fallback company.
    fn equipped(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let (a, fallback) = ((r.tag & 0xFFFF_FFFF) as usize, (r.tag >> 32) as usize);
        if granted >= r.take || a == fallback {
            return;
        }
        let res = self.resolved();
        let Some(per) = res.kinds.iter().find(|k| k.node == w.armies.kind[a]).and_then(|k| k.good.as_ref().map(|g| g.1)) else { return };
        // The shortfall leaves that many soldiers without, rounded up.
        let without = (((r.take - granted) as f64 / per).ceil() as u64).min(w.armies.size[a]);
        w.armies.size[a] -= without;
        w.armies.size[fallback] += without;
        let home = w.armies.home[a];
        if cx.traced(home) {
            cx.note(format!("{granted} of {} goods came for company {a}'s recruits: {without} join company {fallback} instead", r.take));
        }
    }

    /// Recruits came: book them in, then ask for their goods and pay their bounty, for as many as
    /// came.
    fn recruited(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let (a, fallback) = ((r.tag & 0xFFFF_FFFF) as usize, (r.tag >> 32) as u32);
        w.armies.size[a] += granted;
        cx.flow(Quantity::People, Cause::Raised, granted as i64);
        let home = w.armies.home[a];
        if cx.traced(home) {
            cx.note(format!("{granted} recruits joined company {a} (asked for {})", r.take));
        }
        if granted == 0 || !self.coins {
            return;
        }
        let res = self.resolved();
        let kind = res.kinds.iter().find(|k| k.node == w.armies.kind[a]);
        if let Some((good, per)) = kind.and_then(|k| k.good.clone()) {
            let node = cx.goods.tree.find(&good).expect("checked when the world was made");
            let need = (per * granted as f64).ceil() as u64;
            cx.ask_good(known("province.stock"), home, node, need, Priority::Normal, Cause::Equipped, (a as u64) | (fallback as u64) << 32);
        }
        let realm = w.armies.realm[a];
        let pay = (self.params.bounty * granted as f64).round() as u64;
        if pay > 0 && w.realms.alive(realm) {
            let money = cx.goods.tree.find(&self.params.money).expect("checked when the world was made");
            let to = Dest { field: known("province.stock"), entity: home };
            cx.ask_move(known("realm.stock"), realm, Some(money), pay, to, Priority::Low, Cause::Bounty, a as u64);
        }
    }

    /// A month's food came for the companies standing in a province, in full or not: each gets the
    /// same share of what it asked for. Soldiers die of age, and of hunger, and the hungry desert.
    fn fed(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let Some(companies) = self.eating.remove(&(r.tag as u32)) else { return };
        let fed = if r.take == 0 { 1.0 } else { granted as f64 / r.take as f64 };
        let (p, cal, seed) = (&self.params, cx.calendar, cx.seed);
        let days = cal.def.months[cal.date(r.day).month].days as f64;
        for (a, asked) in companies {
            let a = a as usize;
            let size = w.armies.size[a];
            if size == 0 {
                continue;
            }
            let draw = |x: f64, what: &str| round_stochastic(x.max(0.0), seed, Rng::Warfare, r.day, a as u64, purpose(what));
            // The food was asked for the soldiers there were then; recruits who joined since ate
            // with the civilians this month.
            let unfed = asked.min(size) as f64 * (1.0 - fed);
            let natural = draw(size as f64 * p.deaths_per_year * days / cal.days_per_year() as f64, "deaths").min(size);
            let deserted = draw(unfed * p.desert_share, "deserted").min(size - natural);
            let starved = draw((unfed - deserted as f64) * p.starvation_deaths * days / 30.0, "starved").min(size - natural - deserted);
            w.armies.size[a] = size - natural - deserted - starved;
            cx.flow(Quantity::People, Cause::Deaths, -(natural as i64));
            cx.flow(Quantity::People, Cause::Starvation, -(starved as i64));
            let home = w.armies.home[a];
            if deserted > 0 {
                cx.flow(Quantity::People, Cause::Deserted, -(deserted as i64));
                cx.emit(Event::Arrived { province: home, people: deserted, cause: Cause::Deserted });
            }
            let at = w.armies.province[a];
            if cx.traced(at) || cx.traced(home) {
                cx.note(format!(
                    "company {a} ({size} soldiers) got {:.0}% of the month's food it asked for: {natural} died of age and illness, {starved} starved, {deserted} deserted home",
                    100.0 * fed
                ));
            }
        }
    }
}

/// What part of province `prov`'s raising goes to each kind (in `res.kinds` order), as whole
/// weights: each kind's weight times the share of the land that suits it.
fn kind_weights(w: &World, prov: usize, res: &Resolved) -> Vec<u64> {
    let p = &w.provinces;
    let of = |x: f64, whole: f64| if whole > 0.0 { (x / whole).clamp(0.0, 1.0) } else { 0.0 };
    res.kinds
        .iter()
        .map(|k| {
            let land = match k.land {
                Land::Any => 1.0,
                Land::Ore => of(p.ore_km2[prov], p.area_km2[prov]),
                Land::Grazing => of(p.grazing_capacity[prov] as f64, p.food_capacity[prov] as f64),
                Land::Woodland => of(p.forest_km2[prov], p.area_km2[prov]),
            };
            let shore = if k.coast && p.coast_km[prov] <= 0.0 { 0.0 } else { 1.0 };
            (k.weight * land * shore * 1e6).round() as u64
        })
        .collect()
}

/// `people` of company `a`'s soldiers leave it for home, with `cause`.
fn go_home(w: &mut World, cx: &mut Ctx, a: usize, people: u64, cause: Cause) {
    w.armies.size[a] -= people;
    cx.flow(Quantity::People, cause, -(people as i64));
    cx.emit(Event::Arrived { province: w.armies.home[a], people, cause });
}
