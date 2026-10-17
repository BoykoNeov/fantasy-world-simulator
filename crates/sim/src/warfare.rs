//! Warfare (step 7): soldiers taken from the pops, standing or marching, serving a realm.
//!
//! Army numbers are never reused: a disbanded army stays as an entry of no soldiers. Soldiers are
//! people, counted in the people total with the pops (see [`crate::fields::Quantity::fields`]).
//!
//! Warfare v1, "troops" ([`Troops`]), raises them, moves them and fights with them. An army entry
//! is a *company*: soldiers of one kind (a leaf of the troop tree, `data/troops.toml`) from one
//! province, their home, serving the realm that held it when they were raised. A company is a
//! *guard*, the peace-time force that stays home and fights for its own province, or a *field*
//! company, raised in war, which marches.
//!
//! Each month, for every province a realm holds:
//! 1. Companies whose realm no longer holds their home (it changed hands, or the realm was
//!    dissolved) go home, all of them, and so do the guards of a province an enemy occupies.
//! 2. Its guards are kept at a share of the province's people (its soldiers included); while its
//!    realm is at war, field companies make up a bigger share. A tenth or more below either number
//!    it raises up to it, at most a set part of the war-time number a month, shared between the
//!    kinds by how well its land suits each (land with ore for heavy infantry, grazing for cavalry,
//!    a wooded shore for ships). Well over it, the extra go home. A province an enemy occupies, or
//!    in uprising, raises nobody.
//! 3. The companies standing in a province ask it together for a month of food, at the priority
//!    people ask with, so soldiers and civilians share a shortage alike. One request a province, not
//!    one a company, since each costs the engine as much as a company's whole month; the module
//!    keeps who asked for how many until the grant comes (its private state). A company on its way
//!    somewhere eats from the province it left.
//!
//! People are asked of Population by province, and booked into the company the next day, when the
//! grant comes. Then, under an economy with coins (markets), the goods each new soldier needs are
//! asked for (tools, timber), sized to the recruits who actually came; a recruit whose goods the
//! province can't supply joins the province's levies instead. And the realm pays a bounty for each
//! recruit, moved from its treasury into the province: asked at low priority, the month's second
//! day, so it never cuts into the realm's own spending. Under Economics v0, which keeps only food,
//! the land alone decides the kinds. A company waiting for recruits or their goods isn't sent
//! anywhere until they come, so nobody joins a company that has marched off.
//!
//! When a company's food comes, part of its unfed desert home (sent to Population as
//! [`Event::Arrived`]), the unfed who stay die as hungry civilians do, and soldiers die of age and
//! illness at the civilians' rate. Soldiers' hunger is not a famine: it adds nothing to unrest.
//!
//! Every day:
//! - Companies on the move count down the days of the way they are on (the way's days over their
//!   pace), along a route worked out when they were sent: through land their realm holds or
//!   occupies, its enemies' land and land nobody owns, never a neutral realm's. Land troops go by
//!   land ways and ships by sea ways; land troops cross the sea only in a crossing (below). A
//!   company that comes into a province where enemies stand stops there.
//! - Where a realm's forces and its enemy's stand in one province, they fight a day of battle (see
//!   [`TroopParams::battle_rate`]), land soldiers against land soldiers. A side that has lost a set
//!   share of the soldiers it fought with breaks: its field companies fall back to the nearest
//!   province of its own where no enemy stands and rest a while; its guards go back to the people.
//!   When a battle ends Warfare sends [`Event::Battle`], and the realms module adds unrest there.
//! - A realm's field companies standing alone in an enemy province besiege it, and after the
//!   province's siege time Warfare sends [`Event::Occupied`]: the realms module marks it occupied.
//!   They free a province of their own an enemy occupies in the same way, in half the time.
//! - The general of each realm at war plans, on the month's first day and whenever one of its
//!   companies arrives, a battle ends or a province is taken: for each target in turn (its own
//!   land where enemies stand or that an enemy occupies, then enemy provinces next to land it
//!   holds or occupies, nearest its capital first, then enemy coasts across the sea), it sends
//!   free field companies, nearest first, to gather in its provinces next to the target, each
//!   taking soldiers up to a share of its people, until they outmatch the enemies standing there.
//!   Once they have all arrived, the group sets off for the target together. Companies with
//!   nothing to do go home to their province.
//! - A sea crossing goes from a coast the realm holds or occupies to an enemy coast facing it,
//!   with enough of the realm's ships to carry the soldiers. Enemy ships standing where they land
//!   fight the fleet first, worked out on arrival; if the fleet breaks, the soldiers turn back and
//!   those the surviving ships can't carry are lost at sea. After a landing the ships go back.
//!
//! The war is checked on the day: battles and sieges go on only between realms at war that day. At
//! a peace, a realm with no war left sends all its field companies home; one still at war brings
//! them back from the former enemy's land and plans again.
//!
//! Nothing reads the army fields but this module, so the realms and markets modules don't depend
//! on them (they may not: warfare reads theirs). They learn of battles and occupations by events.

use crate::diplomacy;
use crate::edit::Edit;
use crate::events::{Event, EventKind};
use crate::fields::{known, Goods, Quantity, Tier};
use crate::grid::{dot, NONE};
use crate::module::{Cadence, Ctx, Flows, Manifest, Module, Slot};
use crate::requests::{split_proportional, Cause, Dest, Priority, Request};
use crate::rng::{purpose, round_stochastic, unit, Module as Rng};
use crate::taxonomy::{NodeId, Taxonomy, TreeDef};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::sync::Arc;

/// The parameters shipped in `data/troops.toml`.
pub const STANDARD: &str = include_str!("../data/troops.toml");

/// `army.role` of a guard: the peace-time force, which stays home.
pub const GUARD: u8 = 0;
/// `army.role` of a field company: raised in war, it marches.
pub const FIELD: u8 = 1;

/// One entry per army, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Armies {
    /// The realm it serves (field `army.realm`).
    pub realm: Vec<u32>,
    /// The province it stands in, or left while on its way (field `army.province`).
    pub province: Vec<u32>,
    /// Soldiers (field `army.size`).
    pub size: Vec<u64>,
    /// The kind of its soldiers: a node of the troop tree (field `army.kind`).
    pub kind: Vec<u16>,
    /// The province its soldiers came from, where they go home to (field `army.home`).
    pub home: Vec<u32>,
    /// [`GUARD`] or [`FIELD`] (field `army.role`).
    pub role: Vec<u8>,
    /// The number of the group it marches in, [`NONE`] if in none (field `army.group`). Group
    /// numbers are never reused.
    pub group: Vec<u32>,
    /// Where it is going, [`NONE`] when standing (field `army.dest`).
    pub dest: Vec<u32>,
    /// The province it is on its way to next, [`NONE`] when standing (field `army.next`).
    pub next: Vec<u32>,
    /// Days until it reaches `next` (field `army.days_left`).
    pub days_left: Vec<f32>,
    /// Days it has besieged the province it stands in (field `army.siege_days`).
    pub siege_days: Vec<u32>,
}

impl Armies {
    pub fn count(&self) -> usize {
        self.size.len()
    }

    /// A new company of `size` soldiers of `kind`, from `home` and standing there, serving `realm`.
    pub fn push(&mut self, realm: u32, home: u32, kind: u16, role: u8, size: u64) -> u32 {
        let a = self.count() as u32;
        self.realm.push(realm);
        self.province.push(home);
        self.size.push(size);
        self.kind.push(kind);
        self.home.push(home);
        self.role.push(role);
        self.group.push(NONE);
        self.dest.push(NONE);
        self.next.push(NONE);
        self.days_left.push(0.0);
        self.siege_days.push(0);
        a
    }

    /// Whether company `a` is on a way between provinces.
    pub fn moving(&self, a: usize) -> bool {
        self.next[a] != NONE
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

fn one() -> f64 {
    1.0
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
    /// A soldier's strength in battle: what they deal, and what it takes to kill them.
    #[serde(default = "one")]
    pub strength: f64,
    /// How fast they go: a way's days are divided by it. A group goes at its slowest.
    #[serde(default = "one")]
    pub speed: f64,
    /// For ships: the soldiers each crewman takes across the sea.
    #[serde(default)]
    pub carry: f64,
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
    /// The class of the troop tree whose kinds are ships: they move by sea, fight only ships, and
    /// carry land troops across the sea.
    pub sea: String,
    /// A province where a group gathers takes its realm's field soldiers up to this share of its
    /// people.
    pub gather_share: f64,
    /// The general attacks a province when the strength it sends is this many times the enemy's
    /// standing there (counting the defender's edge on its own land).
    pub outmatch: f64,
    /// Each day of battle a side loses this share of the other side's strength, in strength.
    pub battle_rate: f64,
    /// Each side's blows each day are this much stronger or weaker at most, at random.
    pub luck: f64,
    /// The side on its own land deals this much more.
    pub home_edge: f64,
    /// A side breaks once it has lost this share of the soldiers it fought with.
    pub break_share: f64,
    /// Days a company that broke isn't sent to attack.
    pub rest_days: u64,
    /// Days a siege takes: `siege_base_days` × (1 + its people / the people of the world's middle
    /// province), times `capital_siege` for a capital, then at most `siege_max_days`; freeing
    /// one's own province takes `free_siege` of that. Measured against the middle province, as
    /// provinces hold ten times more people on a small world than on a large one.
    pub siege_base_days: f64,
    pub capital_siege: f64,
    pub siege_max_days: f64,
    pub free_siege: f64,
    /// Besiegers fewer than this share of the province's people make no progress.
    pub siege_min_share: f64,
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
    strength: f64,
    speed: f64,
    carry: f64,
    ship: bool,
}

/// The parameters checked and turned into the troop tree and its kinds.
#[derive(Clone, Debug)]
struct Resolved {
    /// Every leaf of the tree, in tree order.
    kinds: Vec<Kind>,
    /// The fallback kind's place in `kinds`.
    fallback: usize,
    /// Each tree node's place in `kinds`, `usize::MAX` for classes.
    of_node: Vec<usize>,
    tree: Taxonomy,
}

impl Resolved {
    fn kind(&self, node: NodeId) -> &Kind {
        &self.kinds[self.of_node[node as usize]]
    }

    /// The kind (a leaf of the troop tree) called `name`.
    fn named(&self, name: &str) -> Result<NodeId, String> {
        match self.tree.find(name) {
            Some(n) if self.of_node[n as usize] != usize::MAX => Ok(n),
            Some(_) => Err(format!("{name} is a class of troops, not a kind: pick one of its kinds")),
            None => Err(format!("there is no troop kind called {name}")),
        }
    }
}

impl TroopParams {
    fn resolve(&self) -> Result<Resolved, String> {
        let tree = Taxonomy::named(&self.tree, "troop tree")?;
        let leaves = tree.leaves_under(0);
        if let Some(name) = self.kinds.keys().find(|n| tree.find(n).is_none_or(|id| !leaves.contains(&id))) {
            return Err(format!("troops: {name} is not a kind (a leaf) of the troop tree"));
        }
        let sea = tree.find(&self.sea).ok_or_else(|| format!("troops: the sea class {} is not in the troop tree", self.sea))?;
        let ships = tree.leaves_under(sea);
        let mut kinds = Vec::new();
        let mut of_node = vec![usize::MAX; tree.nodes.len()];
        for &leaf in &leaves {
            let name = tree.name(leaf);
            let k = self.kinds.get(name).ok_or_else(|| format!("troops: the kind {name} has no table of its own"))?;
            if !(k.weight.is_finite() && k.weight >= 0.0) {
                return Err(format!("troops: {name}'s weight is zero or more"));
            }
            if k.good.is_some() && !(k.per_soldier.is_finite() && k.per_soldier > 0.0) {
                return Err(format!("troops: {name} uses more than none of its good a soldier"));
            }
            if !(k.strength.is_finite() && k.strength > 0.0 && k.speed.is_finite() && k.speed > 0.0) {
                return Err(format!("troops: {name}'s strength and speed are more than zero"));
            }
            if !(k.carry.is_finite() && k.carry >= 0.0) {
                return Err(format!("troops: {name} carries zero soldiers or more"));
            }
            of_node[leaf as usize] = kinds.len();
            kinds.push(Kind {
                node: leaf,
                weight: k.weight,
                land: k.land,
                coast: k.coast,
                good: k.good.clone().map(|g| (g, k.per_soldier)),
                strength: k.strength,
                speed: k.speed,
                carry: k.carry,
                ship: ships.contains(&leaf),
            });
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
        if ![self.gather_share, self.battle_rate, self.luck, self.free_siege, self.siege_min_share].iter().all(|&x| share(x)) || !(self.break_share > 0.0 && self.break_share <= 1.0) {
            return Err("troops: the gathering share, battle rate, luck, breaking share, freeing share and siege share go from 0 to 1".into());
        }
        let positive = |x: f64| x.is_finite() && x > 0.0;
        if !(positive(self.outmatch) && self.home_edge.is_finite() && self.home_edge >= 0.0) {
            return Err("troops: outmatching is more than zero, the home edge zero or more".into());
        }
        if ![self.siege_base_days, self.capital_siege, self.siege_max_days].iter().all(|&x| positive(x)) {
            return Err("troops: siege days and their factors are more than zero".into());
        }
        Ok(Resolved { kinds, fallback, of_node, tree })
    }
}

/// The troop kinds of `params`' troop tree (its leaves, in tree order): each one's node, as
/// `army.kind` holds it, and its name; whether it is a ship. Empty for a tree that doesn't resolve.
pub fn kinds(params: &TroopParams) -> Vec<(NodeId, String, bool)> {
    params.resolve().map_or(Vec::new(), |r| r.kinds.iter().map(|k| (k.node, r.tree.name(k.node).to_string(), k.ship)).collect())
}

/// A group of companies the general sends against one target.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Group {
    realm: u32,
    /// The province it goes for.
    target: u32,
    /// Its companies, in number order.
    members: Vec<u32>,
    /// The provinces where it gathers.
    gather: Vec<u32>,
    /// Set off for the target.
    launched: bool,
    /// Crossing the sea to the target (until it lands).
    sea: bool,
}

/// A realm in a battle going on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Fighter {
    realm: u32,
    /// Soldiers it has fought with: those there when it began, and those who joined since.
    engaged: u64,
    killed: u64,
    broke: bool,
}

/// What the module keeps between days (saved).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct State {
    /// The companies whose food is asked for, and their soldiers then, by the province they stand
    /// in: kept from the month's first day until the grant comes.
    eating: BTreeMap<u32, Vec<(u32, u64)>>,
    /// Groups by number.
    groups: BTreeMap<u32, Group>,
    next_group: u32,
    /// Each moving company's route after `army.next`: provinces, and whether by sea.
    paths: BTreeMap<u32, Vec<(u32, bool)>>,
    /// A company's pace when its group's sets it (the group's slowest), instead of its own.
    pace: BTreeMap<u32, f32>,
    /// Companies that broke, and the day they may attack again.
    rest: BTreeMap<u32, u64>,
    /// Battles going on, by province.
    battles: BTreeMap<u32, Vec<Fighter>>,
    /// Provinces where a battle or a siege may go on: every province where one can is here, and
    /// some where none can (they leave when looked at).
    candidates: BTreeSet<u32>,
    /// Companies on the move.
    moving: BTreeSet<u32>,
    /// Companies with recruits or their goods asked for and not yet come: how many grants.
    awaiting: BTreeMap<u32, u32>,
    /// Realms whose general plans again today.
    dirty: BTreeSet<u32>,
    /// Look over every company today: land changed hands or a war began or ended.
    sweep: bool,
    /// Companies marching under the player's orders ([`Event::March`]): the general leaves them
    /// alone until they arrive.
    ordered: BTreeSet<u32>,
}

/// Where companies stand, worked out from the army fields (not saved: rebuilt after a load).
#[derive(Clone, Debug, Default)]
struct Index {
    /// The companies with soldiers standing in each province (not on a way).
    standing: BTreeMap<u32, BTreeSet<u32>>,
    /// The province each company is filed under in `standing`, [`NONE`] if none.
    filed: Vec<u32>,
    /// Each realm's field companies, in number order.
    field_of: BTreeMap<u32, Vec<u32>>,
}

impl Index {
    fn build(w: &World) -> Index {
        let arm = &w.armies;
        let mut ix = Index { filed: vec![NONE; arm.count()], ..Index::default() };
        for a in 0..arm.count() {
            if arm.role[a] == FIELD {
                ix.field_of.entry(arm.realm[a]).or_default().push(a as u32);
            }
            if arm.size[a] > 0 && !arm.moving(a) {
                ix.standing.entry(arm.province[a]).or_default().insert(a as u32);
                ix.filed[a] = arm.province[a];
            }
        }
        ix
    }
}

/// Companies to send, each with its route: provinces, and whether by sea.
type Sends = Vec<(u32, Vec<(u32, bool)>)>;

/// A route search's results: each province's days from the nearest source (thousandths, `u64::MAX`
/// where not reached), the province before it on the way from that source, and the source.
struct Search {
    dist: Vec<u64>,
    parent: Vec<u32>,
    origin: Vec<u32>,
}

impl Search {
    /// The provinces from `p` back to its source, `p` left out: the route a company at `p` takes
    /// to the source (ways cost the same both ways).
    fn route_back(&self, p: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut q = p;
        while self.parent[q as usize] != NONE {
            q = self.parent[q as usize];
            out.push(q);
        }
        out
    }
}

/// The quickest routes from `sources` out over the provinces `allowed` lets in, by land ways, or
/// by sea ways if `sea`.
fn search(w: &World, sources: &[u32], allowed: &dyn Fn(u32) -> bool, sea: bool) -> Search {
    let n = w.provinces.count;
    let mut s = Search { dist: vec![u64::MAX; n], parent: vec![NONE; n], origin: vec![NONE; n] };
    let mut heap = BinaryHeap::new();
    for &p in sources {
        if (p as usize) < n && s.dist[p as usize] != 0 {
            s.dist[p as usize] = 0;
            s.origin[p as usize] = p;
            heap.push(Reverse((0u64, p)));
        }
    }
    while let Some(Reverse((d, p))) = heap.pop() {
        if d > s.dist[p as usize] {
            continue;
        }
        for l in w.routes.of(p) {
            let Some(days) = (if sea { l.sea_days } else { l.land_days }) else { continue };
            let q = l.other(p);
            if !allowed(q) {
                continue;
            }
            let nd = d + ((days as f64 * 1000.0).round() as u64).max(1);
            if nd < s.dist[q as usize] {
                s.dist[q as usize] = nd;
                s.parent[q as usize] = p;
                s.origin[q as usize] = s.origin[p as usize];
                heap.push(Reverse((nd, q)));
            }
        }
    }
    s
}

/// The route from `from` to the nearest province (not `from`) that `goal` accepts, over the
/// provinces `allowed` lets in: the provinces after `from`, in order.
fn route_to(w: &World, from: u32, allowed: &dyn Fn(u32) -> bool, sea: bool, goal: &dyn Fn(u32) -> bool) -> Option<Vec<u32>> {
    let n = w.provinces.count;
    let mut dist = vec![u64::MAX; n];
    let mut parent = vec![NONE; n];
    let mut heap = BinaryHeap::new();
    dist.get_mut(from as usize).map(|d| *d = 0)?;
    heap.push(Reverse((0u64, from)));
    while let Some(Reverse((d, p))) = heap.pop() {
        if d > dist[p as usize] {
            continue;
        }
        if p != from && goal(p) {
            let mut out = vec![p];
            let mut q = p;
            while parent[q as usize] != from {
                q = parent[q as usize];
                out.push(q);
            }
            out.reverse();
            return Some(out);
        }
        for l in w.routes.of(p) {
            let Some(days) = (if sea { l.sea_days } else { l.land_days }) else { continue };
            let q = l.other(p);
            if !allowed(q) {
                continue;
            }
            let nd = d + ((days as f64 * 1000.0).round() as u64).max(1);
            if nd < dist[q as usize] {
                dist[q as usize] = nd;
                parent[q as usize] = p;
                heap.push(Reverse((nd, q)));
            }
        }
    }
    None
}

pub struct Troops {
    pub params: TroopParams,
    /// Whether the world keeps coins and the other goods (markets): then soldiers use up goods and
    /// realms pay a bounty.
    coins: bool,
    /// The parameters resolved, worked out the first time they are needed.
    resolved: Option<Arc<Resolved>>,
    st: State,
    /// Rebuilt from the army fields when missing (after a load).
    ix: Option<Index>,
}

impl Troops {
    /// Troops that use up goods and are paid for if `coins` (the world keeps coins), and are raised
    /// by the land alone otherwise. The two differ only in their manifests, with the same version
    /// name and the same private state, so saves and scenario files don't depend on which it is;
    /// the configuration picks by the economics chosen, as it does for realms' taxes.
    pub fn new(params: TroopParams, coins: bool) -> Troops {
        Troops { params, coins, resolved: None, st: State::default(), ix: None }
    }

    fn resolved(&mut self) -> Arc<Resolved> {
        if self.resolved.is_none() {
            self.resolved = Some(Arc::new(self.params.resolve().expect("checked when the world was made")));
        }
        self.resolved.clone().expect("just set")
    }

    fn ix(&mut self, w: &World) -> &mut Index {
        self.ix.get_or_insert_with(|| Index::build(w))
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Warfare,
    version: "troops",
    tier: Tier::Army,
    cadence: Cadence::Daily,
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
        "war.target",
        "province.area_km2",
        "province.food_capacity",
        "province.grazing_capacity",
        "province.forest_km2",
        "province.coast_km",
        "province.ore_km2",
    ],
    reads_earlier: &[],
    owns: &[
        "army.realm",
        "army.province",
        "army.size",
        "army.kind",
        "army.home",
        "army.role",
        "army.group",
        "army.dest",
        "army.next",
        "army.days_left",
        "army.siege_days",
    ],
    requests: &["pop.size", "province.stock"],
    asks: &["food"],
    moves: &[],
    // Soldiers serve realms and are taken from the people.
    requires: &["realm.capital", "pop.size"],
    emits: &[EventKind::Arrived, EventKind::Occupied, EventKind::Battle, EventKind::Refused],
    handles: &[EventKind::Granted, EventKind::Peace, EventKind::WarDeclared, EventKind::Rebellion, EventKind::Occupied, EventKind::March],
    state_schema: "armies-2",
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
        self.ix(w);
        if cx.calendar.is_month_start(cx.day) {
            self.sweep(w, cx);
            self.raise(w, cx);
            self.eat(w, cx);
            let enemies = diplomacy::enemies(w);
            self.st.dirty.extend((0..w.realms.count() as u32).filter(|&r| !enemies[r as usize].is_empty()));
        } else if self.st.sweep {
            self.sweep(w, cx);
        }
        self.st.sweep = false;
        if !self.st.moving.is_empty() {
            self.march(w, cx);
        }
        if !self.st.candidates.is_empty() {
            self.fight(w, cx);
        }
        if !self.st.dirty.is_empty() {
            self.command(w, cx);
        }
    }

    /// A day that isn't a month's first, with nobody on the move, no battle or siege that may go on,
    /// no general to plan and nothing to look over, has nothing to do.
    fn idle(&self, _w: &World, calendar: &crate::calendar::Calendar, day: u64) -> bool {
        let s = &self.st;
        !calendar.is_month_start(day) && s.moving.is_empty() && s.candidates.is_empty() && s.dirty.is_empty() && !s.sweep
    }

    fn save_state(&self) -> Vec<u8> {
        crate::sim::to_bytes(&self.st)
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.st = postcard::from_bytes(bytes).map_err(|e| format!("Warfare state: {e}"))?;
        self.ix = None;
        Ok(())
    }

    fn handle(&mut self, w: &mut World, e: &Event, cx: &mut Ctx) {
        if w.realms.count() == 0 {
            return;
        }
        self.ix(w);
        match *e {
            Event::Granted { request: r, granted } => match r.cause {
                Cause::Raised => self.recruited(w, cx, &r, granted),
                Cause::Equipped => self.equipped(w, cx, &r, granted),
                Cause::Eating => self.fed(w, cx, &r, granted),
                // The bounty is paid: nothing follows.
                _ => {}
            },
            Event::Peace { attacker, defender, .. } => {
                let enemies = diplomacy::enemies(w);
                for r in [attacker, defender] {
                    if enemies.get(r as usize).is_some_and(|e| e.is_empty()) {
                        self.stand_down(w, cx, r);
                    } else {
                        self.st.dirty.insert(r);
                    }
                }
                self.st.sweep = true;
            }
            Event::WarDeclared { attacker, defender, .. } => {
                self.st.dirty.extend([attacker, defender]);
                self.st.sweep = true;
            }
            Event::Rebellion { .. } => self.st.sweep = true,
            Event::Occupied { province, .. } => {
                self.st.candidates.insert(province);
                self.st.sweep = true;
            }
            Event::March { realm, from, to } => self.order(w, cx, realm, from, to),
            _ => {}
        }
    }

    fn check_edit(&self, w: &World, edit: &Edit) -> Option<Result<(), String>> {
        let res = || self.resolved.clone().map_or_else(|| self.params.resolve().map(Arc::new), Ok);
        match edit {
            Edit::PlaceArmy { province, realm, kind, .. } => Some(res().and_then(|res| placeable(w, &res, *province, *realm, kind).map(|_| ()))),
            Edit::SetArmy { army, kind, .. } => Some(res().and_then(|res| {
                let node = res.named(kind)?;
                let a = *army as usize;
                let (ship, was) = (res.kind(node).ship, res.kind(w.armies.kind[a]).ship);
                if ship && w.provinces.coast_km.get(w.armies.province[a] as usize).is_none_or(|&c| c <= 0.0) {
                    return Err(format!("{kind} need a coast"));
                }
                // A route by sea or by land, and a crossing's ships, were worked out for its kind.
                if ship != was && (w.armies.moving(a) || w.armies.group[a] != NONE) {
                    return Err(format!("company {army} is on the move or in a group: it can't change between ships and land troops now"));
                }
                Ok(())
            })),
            _ => None,
        }
    }

    /// Soldiers placed, or a company's size or kind set. At home, placed soldiers join the realm's
    /// idle field company of their kind there, so a province keeps one of each.
    fn carry_out(&mut self, w: &mut World, edit: &Edit, _day: u64, _calendar: &crate::calendar::Calendar) -> Flows {
        let res = self.resolved();
        self.ix(w);
        let mut flows = Flows::default();
        match *edit {
            Edit::PlaceArmy { province: p, realm, ref kind, size } => {
                let node = placeable(w, &res, p, realm, kind).expect("checked");
                let a = if w.owner[p as usize] == realm {
                    let mut mine: Vec<u32> = self.ix(w).field_of.get(&realm).map_or(Vec::new(), |v| v.iter().copied().filter(|&b| w.armies.home[b as usize] == p).collect());
                    self.company_at_home(w, &mut mine, realm, p, node, FIELD)
                } else {
                    let a = self.new_company(w, realm, w.realms.capital[realm as usize], node, FIELD);
                    w.armies.province[a as usize] = p;
                    a
                };
                w.armies.size[a as usize] += size;
                flows.add(Quantity::People, Cause::Edit, size as i64);
                self.settle(w, a);
                if at_war_with_anyone(w, realm) {
                    self.st.dirty.insert(realm);
                }
            }
            Edit::SetArmy { army: a, size, ref kind } => {
                let au = a as usize;
                flows.add(Quantity::People, Cause::Edit, size as i64 - w.armies.size[au] as i64);
                w.armies.size[au] = size;
                w.armies.kind[au] = res.named(kind).expect("checked");
                self.settle(w, a);
                let realm = w.armies.realm[au];
                if w.armies.role[au] == FIELD && at_war_with_anyone(w, realm) {
                    self.st.dirty.insert(realm);
                }
            }
            _ => {}
        }
        flows
    }

    /// After an edit that changed the map or who holds land: file every company again where its
    /// fields put it (an edit may have moved it off land that sank), drop the routes of those no
    /// longer on the move, look over every company today and have every realm at war plan again.
    fn edited(&mut self, w: &mut World, edit: &Edit) {
        if w.realms.count() == 0 || !(edit.changes_map() || matches!(edit, Edit::SetOwner { .. } | Edit::NewRealm { .. })) {
            return;
        }
        self.ix = None;
        self.ix(w);
        for a in 0..w.armies.count() as u32 {
            if !w.armies.moving(a as usize) {
                self.st.paths.remove(&a);
                self.st.pace.remove(&a);
                self.st.ordered.remove(&a);
            }
            self.settle(w, a);
        }
        self.st.sweep = true;
        let enemies = diplomacy::enemies(w);
        self.st.dirty.extend((0..w.realms.count() as u32).filter(|&r| !enemies[r as usize].is_empty()));
    }
}

/// The troop kind called `kind`, if realm `realm` may place soldiers of it in province `p`: land it
/// holds or a realm's it is at war with; ships only on a coast.
fn placeable(w: &World, res: &Resolved, p: u32, realm: u32, kind: &str) -> Result<NodeId, String> {
    let node = res.named(kind)?;
    let pu = p as usize;
    if w.provinces.cells.get(pu).is_none_or(|&c| c == 0) {
        return Err(format!("province {p} has no land"));
    }
    let owner = w.owner[pu];
    if owner != realm && !diplomacy::at_war(w, realm, owner) {
        return Err(format!("realm {realm} may place soldiers only on its own land or an enemy's at war with it"));
    }
    if res.kind(node).ship && w.provinces.coast_km[pu] <= 0.0 {
        return Err(format!("{kind} need a coast"));
    }
    Ok(node)
}

impl Troops {
    /// A new company, filed in the index.
    fn new_company(&mut self, w: &mut World, realm: u32, home: u32, kind: NodeId, role: u8) -> u32 {
        let a = w.armies.push(realm, home, kind, role, 0);
        let ix = self.ix(w);
        ix.filed.push(NONE);
        if role == FIELD {
            ix.field_of.entry(realm).or_default().push(a);
        }
        a
    }

    /// File company `a` where its fields say it is, after any change to them. A company emptied
    /// goes back to standing at home in no group.
    fn settle(&mut self, w: &mut World, a: u32) {
        let au = a as usize;
        if w.armies.size[au] == 0 {
            self.leave_group(w, a);
            let arm = &mut w.armies;
            arm.province[au] = arm.home[au];
            arm.dest[au] = NONE;
            arm.next[au] = NONE;
            arm.days_left[au] = 0.0;
            arm.siege_days[au] = 0;
            self.st.paths.remove(&a);
            self.st.pace.remove(&a);
            self.st.rest.remove(&a);
            self.st.ordered.remove(&a);
        }
        let arm = &w.armies;
        let (size, moving, at, realm) = (arm.size[au], arm.moving(au), arm.province[au], arm.realm[au]);
        let want = if size > 0 && !moving { at } else { NONE };
        let ix = self.ix.as_mut().expect("built before use");
        if ix.filed.len() <= au {
            ix.filed.resize(au + 1, NONE);
        }
        let was = ix.filed[au];
        if was != want {
            if was != NONE
                && let Some(s) = ix.standing.get_mut(&was)
            {
                s.remove(&a);
                if s.is_empty() {
                    ix.standing.remove(&was);
                }
            }
            if want != NONE {
                ix.standing.entry(want).or_default().insert(a);
            }
            ix.filed[au] = want;
        }
        if size > 0 && moving {
            self.st.moving.insert(a);
        } else {
            self.st.moving.remove(&a);
        }
        if want != NONE && (w.owner.get(want as usize) != Some(&realm) || w.occupier.get(want as usize).is_some_and(|&o| o != NONE)) {
            self.st.candidates.insert(want);
        }
    }

    fn leave_group(&mut self, w: &mut World, a: u32) {
        let g = w.armies.group[a as usize];
        if g == NONE {
            return;
        }
        w.armies.group[a as usize] = NONE;
        if let Some(grp) = self.st.groups.get_mut(&g) {
            grp.members.retain(|&m| m != a);
            if grp.members.is_empty() {
                self.st.groups.remove(&g);
            }
        }
    }

    /// `people` of company `a`'s soldiers leave it for home, with `cause`.
    fn go_home(&mut self, w: &mut World, cx: &mut Ctx, a: u32, people: u64, cause: Cause) {
        if people == 0 {
            return;
        }
        w.armies.size[a as usize] -= people;
        cx.flow(Quantity::People, cause, -(people as i64));
        cx.emit(Event::Arrived { province: w.armies.home[a as usize], people, cause });
        self.settle(w, a);
    }

    /// Kill `killed` of company `a`'s soldiers (in battle, or lost at sea).
    fn kill(&mut self, w: &mut World, cx: &mut Ctx, a: u32, killed: u64) {
        if killed == 0 {
            return;
        }
        w.armies.size[a as usize] -= killed;
        cx.flow(Quantity::People, Cause::Killed, -(killed as i64));
        self.settle(w, a);
    }

    /// Send company `a` along `path` (provinces after where it stands, and whether by sea), at
    /// `pace` or its own speed. False, and it stays, if the path is empty or its first way is gone.
    fn send(&mut self, w: &mut World, res: &Resolved, a: u32, mut path: Vec<(u32, bool)>, pace: Option<f32>) -> bool {
        let au = a as usize;
        if path.is_empty() || w.armies.size[au] == 0 {
            return false;
        }
        let (q, sea) = path.remove(0);
        let speed = pace.unwrap_or(res.kind(w.armies.kind[au]).speed as f32);
        let Some(days) = way_days(w, w.armies.province[au], q, sea) else { return false };
        let arm = &mut w.armies;
        arm.dest[au] = path.last().map_or(q, |s| s.0);
        arm.next[au] = q;
        arm.days_left[au] = days / speed;
        arm.siege_days[au] = 0;
        self.st.paths.insert(a, path);
        match pace {
            Some(p) => self.st.pace.insert(a, p),
            None => self.st.pace.remove(&a),
        };
        self.settle(w, a);
        true
    }

    /// Company `a` stops where it stands.
    fn halt(&mut self, w: &mut World, a: u32) {
        self.st.ordered.remove(&a);
        let au = a as usize;
        let arm = &mut w.armies;
        arm.next[au] = NONE;
        arm.dest[au] = NONE;
        arm.days_left[au] = 0.0;
        self.st.paths.remove(&a);
        self.st.pace.remove(&a);
        self.settle(w, a);
    }

    /// Look over every company: those whose realm no longer holds their home, and the guards of a
    /// province an enemy occupies, go home; and every province where a battle or siege may go on is
    /// a candidate.
    fn sweep(&mut self, w: &mut World, cx: &mut Ctx) {
        for a in 0..w.armies.count() {
            let (realm, home, size, role) = (w.armies.realm[a], w.armies.home[a], w.armies.size[a], w.armies.role[a]);
            if size == 0 {
                continue;
            }
            let lost = w.owner.get(home as usize) != Some(&realm) || !w.realms.alive(realm);
            let occupied = role == GUARD && w.occupier.get(home as usize).is_some_and(|&o| o != NONE);
            if lost || occupied {
                if cx.traced(home) {
                    let why = if lost { format!("its realm {realm} no longer holds the province") } else { "the province is occupied".to_string() };
                    cx.note(format!("company {a} ({size} soldiers) goes home: {why}"));
                }
                self.go_home(w, cx, a as u32, size, Cause::Returned);
            }
        }
        let ix = self.ix.as_ref().expect("built before use");
        let found: Vec<u32> = ix
            .standing
            .iter()
            .filter(|(p, set)| {
                let (owner, occ) = (w.owner.get(**p as usize).copied().unwrap_or(NONE), w.occupier.get(**p as usize).copied().unwrap_or(NONE));
                occ != NONE || set.iter().any(|&a| w.armies.realm[a as usize] != owner)
            })
            .map(|(p, _)| *p)
            .collect();
        self.st.candidates.extend(found);
    }

    /// The month's raising, and soldiers going home where there are too many.
    fn raise(&mut self, w: &mut World, cx: &mut Ctx) {
        let res = self.resolved();
        let p = self.params.clone();
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
            let sum = |w: &World, role: u8, mine: &[u32]| -> u64 { mine.iter().filter(|&&a| w.armies.role[a as usize] == role).map(|&a| w.armies.size[a as usize]).sum() };
            let (guards, fields) = (sum(w, GUARD, &mine), sum(w, FIELD, &mine));
            let total = people[prov] + guards + fields;
            if total == 0 {
                continue;
            }
            let war = enemies.get(owner as usize).is_some_and(|e| !e.is_empty());
            let peace_wanted = (total as f64 * p.peace_share).round() as u64;
            let field_wanted = if war { ((total as f64 * p.war_share).round() as u64).saturating_sub(peace_wanted) } else { 0 };
            let traced = cx.traced(prov as u32);
            // Too many: the extra go home, guards from the guards; field companies only from those
            // standing at home in no group, and all of them once the realm is at peace.
            let over = |cur: u64, wanted: u64| cur > wanted && cur as f64 > wanted as f64 * (1.0 + p.disband_slack);
            if over(guards, peace_wanted) {
                self.thin(w, cx, &mine, GUARD, guards - peace_wanted);
                if traced {
                    cx.note(format!("{guards} guards, {peace_wanted} wanted: {} go home", guards - peace_wanted));
                }
            }
            if over(fields, field_wanted) {
                self.thin(w, cx, &mine, FIELD, fields - field_wanted);
                if traced {
                    cx.note(format!("{fields} soldiers in field companies, {field_wanted} wanted: up to {} go home", fields - field_wanted));
                }
            }
            if people[prov] == 0 || w.occupier[prov] != NONE || w.uprising[prov] != NONE {
                continue;
            }
            let step = ((total as f64 * p.war_share / p.months_to_raise).ceil() as u64).max(1);
            let low = |cur: u64, wanted: u64| (cur as f64) < wanted as f64 * p.raise_below;
            let raise_guards = if low(guards, peace_wanted) { (peace_wanted - guards).min(step) } else { 0 };
            let raise_fields = if low(fields, field_wanted) { (field_wanted - fields).min(step - raise_guards) } else { 0 };
            let weights = kind_weights(w, prov, &res);
            let order: Vec<u64> = (0..res.kinds.len() as u64).collect();
            for (role, raise) in [(GUARD, raise_guards), (FIELD, raise_fields)] {
                if raise == 0 {
                    continue;
                }
                let parts = split_proportional(raise, &weights, &order);
                for (k, &part) in parts.iter().enumerate() {
                    if part == 0 {
                        continue;
                    }
                    let a = self.company_at_home(w, &mut mine, owner, prov as u32, res.kinds[k].node, role);
                    // Recruits a kind's goods can't equip join the fallback company: chosen now,
                    // so the grant knows where they go.
                    let fallback = if self.coins && res.kinds[k].good.is_some() { self.company_at_home(w, &mut mine, owner, prov as u32, res.kinds[res.fallback].node, role) } else { a };
                    let tag = (a as u64) | (fallback as u64) << 32;
                    cx.request(pop_size, prov as u32, part, Priority::Normal, Cause::Raised, tag);
                    *self.st.awaiting.entry(a).or_default() += 1;
                }
                if traced {
                    let what = if role == GUARD { "guards" } else { "field companies" };
                    cx.note(format!("realm {owner} {}: raising {raise} for its {what}, by kind {parts:?}", if war { "at war" } else { "at peace" }));
                }
            }
        }
    }

    /// The province's company of a kind and role to take recruits: for guards its guard company;
    /// for field companies one standing at home in no group, or an emptied one, made if it has
    /// neither (and then found by the next kind that asks for it).
    fn company_at_home(&mut self, w: &mut World, mine: &mut Vec<u32>, owner: u32, prov: u32, node: NodeId, role: u8) -> u32 {
        let arm = &w.armies;
        let of_kind = |a: u32| arm.kind[a as usize] == node && arm.role[a as usize] == role;
        let free = |a: u32| {
            let au = a as usize;
            role == GUARD || arm.size[au] == 0 || (arm.province[au] == prov && !arm.moving(au) && arm.group[au] == NONE)
        };
        let at_home = mine.iter().copied().find(|&a| of_kind(a) && arm.size[a as usize] > 0 && free(a));
        if let Some(a) = at_home.or_else(|| mine.iter().copied().find(|&a| of_kind(a) && free(a))) {
            return a;
        }
        let a = self.new_company(w, owner, prov, node, role);
        mine.push(a);
        a
    }

    /// `people` of the companies of `role` among `mine` go home, in proportion to their soldiers;
    /// of field companies only those standing at home in no group.
    fn thin(&mut self, w: &mut World, cx: &mut Ctx, mine: &[u32], role: u8, people: u64) {
        let arm = &w.armies;
        let these: Vec<u32> = mine
            .iter()
            .copied()
            .filter(|&a| {
                let au = a as usize;
                arm.role[au] == role && arm.size[au] > 0 && (role == GUARD || (arm.province[au] == arm.home[au] && !arm.moving(au) && arm.group[au] == NONE))
            })
            .collect();
        let sizes: Vec<u64> = these.iter().map(|&a| arm.size[a as usize]).collect();
        let people = people.min(sizes.iter().sum());
        let ids: Vec<u64> = these.iter().map(|&a| a as u64).collect();
        for (&a, part) in these.iter().zip(split_proportional(people, &sizes, &ids)) {
            self.go_home(w, cx, a, part, Cause::Returned);
        }
    }

    /// Soldiers eat where they stand, a ration a day, asked like the people's own food: by the
    /// companies of each province together.
    fn eat(&mut self, w: &mut World, cx: &mut Ctx) {
        let days = cx.calendar.def.months[cx.calendar.date(cx.day).month].days as u64;
        let stock = known("province.stock");
        let food = cx.goods.tree.find("food").expect("checked at configuration: the goods tree has food");
        self.st.eating.clear();
        for a in 0..w.armies.count() {
            let size = w.armies.size[a];
            if size > 0 {
                self.st.eating.entry(w.armies.province[a]).or_default().push((a as u32, size));
            }
        }
        for (&at, companies) in &self.st.eating {
            let soldiers: u64 = companies.iter().map(|c| c.1).sum();
            cx.ask_good(stock, at, food, soldiers * days, Priority::Essential, Cause::Eating, at as u64);
        }
    }

    /// One fewer grant awaited for company `a`; when it awaits none, its realm, if at war, plans
    /// again (it can be sent now).
    fn awaited(&mut self, w: &World, a: u32) {
        if let Some(n) = self.st.awaiting.get_mut(&a) {
            *n -= 1;
            if *n == 0 {
                self.st.awaiting.remove(&a);
                let realm = w.armies.realm[a as usize];
                if w.armies.role[a as usize] == FIELD && at_war_with_anyone(w, realm) {
                    self.st.dirty.insert(realm);
                }
            }
        }
    }

    /// The goods for a company's recruits came, in full or not: those left without join the
    /// fallback company.
    fn equipped(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let (a, fallback) = ((r.tag & 0xFFFF_FFFF) as u32, (r.tag >> 32) as u32);
        self.awaited(w, a);
        if fallback != a {
            self.awaited(w, fallback);
        }
        if granted >= r.take || a == fallback {
            return;
        }
        let res = self.resolved();
        let (au, fu) = (a as usize, fallback as usize);
        let Some(per) = res.kind(w.armies.kind[au]).good.as_ref().map(|g| g.1) else { return };
        // The shortfall leaves that many soldiers without, rounded up.
        let without = (((r.take - granted) as f64 / per).ceil() as u64).min(w.armies.size[au]);
        let home = w.armies.home[au];
        let together = w.armies.province[au] == w.armies.province[fu] && !w.armies.moving(au) && !w.armies.moving(fu);
        if together {
            w.armies.size[au] -= without;
            w.armies.size[fu] += without;
            self.settle(w, a);
            self.settle(w, fallback);
        } else {
            // One of them has gone somewhere since (a battle at home): the unequipped go home.
            self.go_home(w, cx, a, without, Cause::Returned);
        }
        if cx.traced(home) {
            let to = if together { format!("join company {fallback} instead") } else { "go home".to_string() };
            cx.note(format!("{granted} of {} goods came for company {a}'s recruits: {without} {to}", r.take));
        }
    }

    /// Recruits came: book them in, then ask for their goods and pay their bounty, for as many as
    /// came.
    fn recruited(&mut self, w: &mut World, cx: &mut Ctx, r: &Request, granted: u64) {
        let (mut a, fallback) = ((r.tag & 0xFFFF_FFFF) as u32, (r.tag >> 32) as u32);
        self.awaited(w, a);
        let au = a as usize;
        let home = w.armies.home[au];
        let away = w.armies.province[au] != home || w.armies.moving(au) || w.armies.group[au] != NONE;
        if granted > 0 && w.armies.role[au] == FIELD && w.armies.size[au] > 0 && away {
            // It has been sent somewhere since (a battle at home): another company at home takes
            // them.
            let realm = w.armies.realm[au];
            let mut mine: Vec<u32> = self.ix(w).field_of.get(&realm).map_or(Vec::new(), |v| v.iter().copied().filter(|&b| w.armies.home[b as usize] == home).collect());
            a = self.company_at_home(w, &mut mine, realm, home, w.armies.kind[au], FIELD);
        }
        let au = a as usize;
        w.armies.size[au] += granted;
        cx.flow(Quantity::People, Cause::Raised, granted as i64);
        self.settle(w, a);
        if cx.traced(home) {
            cx.note(format!("{granted} recruits joined company {a} (asked for {})", r.take));
        }
        if granted == 0 || !self.coins {
            return;
        }
        let res = self.resolved();
        if let Some((good, per)) = res.kind(w.armies.kind[au]).good.clone() {
            let node = cx.goods.tree.find(&good).expect("checked when the world was made");
            let need = (per * granted as f64).ceil() as u64;
            cx.ask_good(known("province.stock"), home, node, need, Priority::Normal, Cause::Equipped, (a as u64) | (fallback as u64) << 32);
            *self.st.awaiting.entry(a).or_default() += 1;
            if fallback != a {
                *self.st.awaiting.entry(fallback).or_default() += 1;
            }
        }
        let realm = w.armies.realm[au];
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
        let Some(companies) = self.st.eating.remove(&(r.tag as u32)) else { return };
        let fed = if r.take == 0 { 1.0 } else { granted as f64 / r.take as f64 };
        let (cal, seed) = (cx.calendar, cx.seed);
        let (deaths_per_year, desert_share, starvation_deaths) = (self.params.deaths_per_year, self.params.desert_share, self.params.starvation_deaths);
        let days = cal.def.months[cal.date(r.day).month].days as f64;
        for (a, asked) in companies {
            let au = a as usize;
            let size = w.armies.size[au];
            if size == 0 {
                continue;
            }
            let draw = |x: f64, what: &str| round_stochastic(x.max(0.0), seed, Rng::Warfare, r.day, a as u64, purpose(what));
            // The food was asked for the soldiers there were then; recruits who joined since ate
            // with the civilians this month.
            let unfed = asked.min(size) as f64 * (1.0 - fed);
            let natural = draw(size as f64 * deaths_per_year * days / cal.days_per_year() as f64, "deaths").min(size);
            let deserted = draw(unfed * desert_share, "deserted").min(size - natural);
            let starved = draw((unfed - deserted as f64) * starvation_deaths * days / 30.0, "starved").min(size - natural - deserted);
            w.armies.size[au] = size - natural - deserted;
            cx.flow(Quantity::People, Cause::Deaths, -(natural as i64));
            w.armies.size[au] -= starved;
            cx.flow(Quantity::People, Cause::Starvation, -(starved as i64));
            let home = w.armies.home[au];
            if deserted > 0 {
                cx.flow(Quantity::People, Cause::Deserted, -(deserted as i64));
                cx.emit(Event::Arrived { province: home, people: deserted, cause: Cause::Deserted });
            }
            self.settle(w, a);
            let at = w.armies.province[au];
            if cx.traced(at) || cx.traced(home) {
                cx.note(format!(
                    "company {a} ({size} soldiers) got {:.0}% of the month's food it asked for: {natural} died of age and illness, {starved} starved, {deserted} deserted home",
                    100.0 * fed
                ));
            }
        }
    }

    /// Realm `r` has no war left: all its field companies go home, and its groups are no more.
    fn stand_down(&mut self, w: &mut World, cx: &mut Ctx, r: u32) {
        let fields = self.ix(w).field_of.get(&r).cloned().unwrap_or_default();
        for a in fields {
            let size = w.armies.size[a as usize];
            self.go_home(w, cx, a, size, Cause::Returned);
        }
        self.st.groups.retain(|_, g| g.realm != r);
    }

    /// The player's order: realm `realm`'s field companies standing in `from` march to `to`, each
    /// by the ways its realm may use and at its own pace, out of their groups, under orders until
    /// they arrive. Refused for companies fighting a battle, and where no way leads there.
    fn order(&mut self, w: &mut World, cx: &mut Ctx, realm: u32, from: u32, to: u32) {
        let res = self.resolved();
        let mut refuse = |why: String| cx.emit(Event::Refused { why });
        if !w.realms.alive(realm) {
            return refuse(format!("realm {realm} is no more"));
        }
        if w.provinces.cells.get(to as usize).is_none_or(|&c| c == 0) || from == to {
            return refuse(format!("province {to} is not somewhere else with land to march to"));
        }
        if self.st.battles.contains_key(&from) {
            return refuse(format!("the companies in province {from} are fighting a battle"));
        }
        let here: Vec<u32> = self.ix(w).standing.get(&from).map_or(Vec::new(), |s| s.iter().copied().filter(|&a| w.armies.realm[a as usize] == realm && w.armies.role[a as usize] == FIELD).collect());
        if here.is_empty() {
            return refuse(format!("realm {realm} has no field companies standing in province {from}"));
        }
        let enemies = diplomacy::enemies(w);
        let foes = enemies.get(realm as usize).cloned().unwrap_or_default();
        let mut sent = 0;
        for a in here {
            let ship = res.kind(w.armies.kind[a as usize]).ship;
            let allowed = |q: u32| allowed_for(w, realm, &foes, q);
            let Some(path) = route_to(w, from, &allowed, ship, &|q| q == to) else { continue };
            self.leave_group(w, a);
            if self.send(w, &res, a, path.into_iter().map(|q| (q, ship)).collect(), None) {
                self.st.ordered.insert(a);
                sent += 1;
                if cx.traced(from) {
                    cx.note(format!("company {a} ({} soldiers) marches to province {to}, by the player's order", w.armies.size[a as usize]));
                }
            }
        }
        if sent == 0 {
            cx.emit(Event::Refused { why: format!("no way realm {realm} may use leads from province {from} to {to}") });
        }
    }

    /// A day on the move for every moving company.
    fn march(&mut self, w: &mut World, cx: &mut Ctx) {
        let res = self.resolved();
        let enemies = diplomacy::enemies(w);
        let movers: Vec<u32> = self.st.moving.iter().copied().collect();
        let mut landed = BTreeSet::new();
        for a in movers {
            let au = a as usize;
            if w.armies.size[au] == 0 || !w.armies.moving(au) {
                continue;
            }
            let realm = w.armies.realm[au];
            w.armies.days_left[au] -= 1.0;
            while w.armies.days_left[au] <= 0.0 {
                let at = w.armies.next[au];
                w.armies.province[au] = at;
                w.armies.siege_days[au] = 0;
                let rest = self.st.paths.get_mut(&a).map(std::mem::take).unwrap_or_default();
                let foes = enemies.get(realm as usize).map_or(&[][..], |v| &v[..]);
                let blocked = !rest.is_empty() && self.enemies_at(w, at, foes);
                if rest.is_empty() || blocked {
                    // Arrived, or stopped by enemies standing in the way.
                    self.halt(w, a);
                    self.st.dirty.insert(realm);
                    if cx.traced(at) {
                        let why = if blocked { "stopped by enemies standing here" } else { "arrived" };
                        cx.note(format!("company {a} ({} soldiers, realm {realm}) {why}", w.armies.size[au]));
                    }
                    let g = w.armies.group[au];
                    if let Some(grp) = self.st.groups.get(&g)
                        && grp.sea
                        && grp.launched
                        && grp.target == at
                    {
                        landed.insert(g);
                    }
                    break;
                }
                let mut path = rest;
                let (q, sea) = path.remove(0);
                let speed = self.st.pace.get(&a).copied().unwrap_or(res.kind(w.armies.kind[au]).speed as f32);
                match way_days(w, at, q, sea) {
                    Some(days) => {
                        w.armies.next[au] = q;
                        w.armies.days_left[au] += days / speed;
                        self.st.paths.insert(a, path);
                    }
                    None => {
                        // The way is gone (the map changed): stop, and plan again.
                        self.halt(w, a);
                        self.st.dirty.insert(realm);
                        break;
                    }
                }
            }
            self.settle(w, a);
        }
        for g in landed {
            self.land(w, cx, &res, g);
        }
    }

    /// Whether enemies of a realm (`foes`, sorted) stand in province `p`.
    fn enemies_at(&self, w: &World, p: u32, foes: &[u32]) -> bool {
        let ix = self.ix.as_ref().expect("built before use");
        ix.standing.get(&p).is_some_and(|s| s.iter().any(|&a| foes.binary_search(&w.armies.realm[a as usize]).is_ok()))
    }

    /// The companies of each realm with soldiers standing in province `p`, land troops or ships.
    fn forces(&self, w: &World, res: &Resolved, p: u32, ships: bool) -> BTreeMap<u32, Vec<u32>> {
        let ix = self.ix.as_ref().expect("built before use");
        let mut out: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        if let Some(s) = ix.standing.get(&p) {
            for &a in s {
                if res.kind(w.armies.kind[a as usize]).ship == ships {
                    out.entry(w.armies.realm[a as usize]).or_default().push(a);
                }
            }
        }
        out
    }

    /// Battles and sieges in the candidate provinces.
    fn fight(&mut self, w: &mut World, cx: &mut Ctx) {
        let res = self.resolved();
        let cands: Vec<u32> = self.st.candidates.iter().copied().collect();
        let mut people: Option<(Vec<u64>, f64)> = None;
        for p in cands {
            let sides = self.forces(w, &res, p, false);
            let realms: Vec<u32> = sides.keys().copied().collect();
            let war = realms.iter().any(|&x| realms.iter().any(|&y| diplomacy::at_war(w, x, y)));
            let keep = if war {
                self.battle_day(w, cx, &res, p, sides);
                true
            } else {
                if self.st.battles.contains_key(&p) {
                    self.end_battle(cx, p);
                }
                let (people, middle) = people.get_or_insert_with(|| {
                    let people = w.province_population();
                    let middle = middle_province(&people);
                    (people, middle)
                });
                self.siege(w, cx, p, sides, people, *middle)
            };
            if !keep {
                self.st.candidates.remove(&p);
            }
        }
    }

    /// The strength of companies `of`.
    fn strength(w: &World, res: &Resolved, of: &[u32]) -> f64 {
        of.iter().map(|&a| w.armies.size[a as usize] as f64 * res.kind(w.armies.kind[a as usize]).strength).sum()
    }

    /// One day's blows between the realms in `sides` (each realm's companies) that are at war with
    /// each other, in province `p`; `salt` keeps the draws of different exchanges apart. Returns
    /// the soldiers each side lost.
    fn exchange(&mut self, w: &mut World, cx: &mut Ctx, res: &Resolved, p: u32, sides: &[(u32, Vec<u32>)], salt: u64) -> Vec<u64> {
        let pr = &self.params;
        let strength: Vec<f64> = sides.iter().map(|(_, c)| Self::strength(w, res, c)).collect();
        let owner = w.owner.get(p as usize).copied().unwrap_or(NONE);
        let mut loss = vec![0.0; sides.len()];
        for (i, (x, _)) in sides.iter().enumerate() {
            let foes: Vec<usize> = (0..sides.len()).filter(|&j| diplomacy::at_war(w, *x, sides[j].0) && strength[j] > 0.0).collect();
            let total: f64 = foes.iter().map(|&j| strength[j]).sum();
            if total <= 0.0 {
                continue;
            }
            let luck = 1.0 + pr.luck * (2.0 * unit(cx.seed, Rng::Warfare, cx.day, (p as u64) << 32 | *x as u64, purpose("luck") ^ salt) - 1.0);
            let edge = if owner == *x { 1.0 + pr.home_edge } else { 1.0 };
            let dealt = pr.battle_rate * strength[i] * luck * edge;
            for &j in &foes {
                loss[j] += dealt * strength[j] / total;
            }
        }
        let mut lost = vec![0u64; sides.len()];
        for (j, (_, companies)) in sides.iter().enumerate() {
            if loss[j] <= 0.0 {
                continue;
            }
            for &a in companies {
                let size = w.armies.size[a as usize];
                // Its share of the side's strength lost, over its strength a soldier.
                let killed = round_stochastic(loss[j] * size as f64 / strength[j], cx.seed, Rng::Warfare, cx.day, a as u64, purpose("killed") ^ salt).min(size);
                self.kill(w, cx, a, killed);
                lost[j] += killed;
            }
        }
        lost
    }

    /// A day of battle on land in province `p`.
    fn battle_day(&mut self, w: &mut World, cx: &mut Ctx, res: &Resolved, p: u32, sides: BTreeMap<u32, Vec<u32>>) {
        let sides: Vec<(u32, Vec<u32>)> = sides.into_iter().collect();
        let mut rec = self.st.battles.remove(&p).unwrap_or_default();
        let soldiers = |w: &World, c: &[u32]| -> u64 { c.iter().map(|&a| w.armies.size[a as usize]).sum() };
        for (x, c) in &sides {
            let now = soldiers(w, c);
            match rec.iter_mut().find(|f| f.realm == *x) {
                Some(f) => {
                    let left = f.engaged.saturating_sub(f.killed);
                    if f.broke {
                        // Back after breaking: a fight anew.
                        *f = Fighter { realm: *x, engaged: now, killed: 0, broke: false };
                    } else if now > left {
                        f.engaged += now - left;
                    }
                }
                None => rec.push(Fighter { realm: *x, engaged: now, killed: 0, broke: false }),
            }
        }
        let lost = self.exchange(w, cx, res, p, &sides, 0);
        let mut broke = Vec::new();
        for (j, (x, c)) in sides.iter().enumerate() {
            let f = rec.iter_mut().find(|f| f.realm == *x).expect("just added");
            f.killed += lost[j];
            let left = soldiers(w, c);
            if lost[j] > 0 && (f.killed as f64 >= self.params.break_share * f.engaged as f64 || left == 0) {
                f.broke = true;
                broke.push((*x, c.clone()));
            }
        }
        if cx.traced(p) {
            let what: Vec<String> = sides.iter().zip(&lost).map(|((x, c), l)| format!("realm {x}: {} soldiers, lost {l}", soldiers(w, c))).collect();
            cx.note(format!("a day of battle: {}", what.join("; ")));
        }
        self.st.battles.insert(p, rec);
        for (x, c) in broke {
            if cx.traced(p) {
                cx.note(format!("realm {x}'s side breaks"));
            }
            self.break_side(w, cx, res, p, x, &c, false);
        }
        // Over if no two realms at war still stand here.
        let left: Vec<u32> = self.forces(w, res, p, false).keys().copied().collect();
        if !left.iter().any(|&x| left.iter().any(|&y| diplomacy::at_war(w, x, y))) {
            self.end_battle(cx, p);
        }
    }

    /// Realm `x`'s companies `of` in province `p` broke: guards go back to the people, field
    /// companies fall back to the nearest province of their realm's own where no enemy stands
    /// (by sea for ships), and rest.
    #[allow(clippy::too_many_arguments)]
    fn break_side(&mut self, w: &mut World, cx: &mut Ctx, res: &Resolved, p: u32, x: u32, of: &[u32], ships: bool) {
        let enemies = diplomacy::enemies(w);
        let foes = enemies.get(x as usize).cloned().unwrap_or_default();
        let mut route: Option<Option<Vec<u32>>> = None;
        for &a in of {
            let au = a as usize;
            let size = w.armies.size[au];
            if size == 0 {
                continue;
            }
            if w.armies.role[au] == GUARD {
                self.go_home(w, cx, a, size, Cause::Returned);
                continue;
            }
            let path = route.get_or_insert_with(|| {
                let allowed = |q: u32| allowed_for(w, x, &foes, q);
                let goal = |q: u32| w.owner[q as usize] == x && w.occupier[q as usize] == NONE && !self.enemies_at(w, q, &foes);
                route_to(w, p, &allowed, ships, &goal)
            });
            self.leave_group(w, a);
            match path.clone() {
                Some(path) => {
                    self.send(w, res, a, path.into_iter().map(|q| (q, ships)).collect(), None);
                    self.st.rest.insert(a, cx.day + self.params.rest_days);
                }
                None => self.go_home(w, cx, a, size, Cause::Returned),
            }
        }
    }

    /// The battle in province `p` is over: the realm whose land it is gains unrest (the realms
    /// module's), and the realms in it plan again.
    fn end_battle(&mut self, cx: &mut Ctx, p: u32) {
        let Some(rec) = self.st.battles.remove(&p) else { return };
        let winner = rec.iter().filter(|f| !f.broke).max_by_key(|f| (f.engaged.saturating_sub(f.killed), Reverse(f.realm))).map_or(NONE, |f| f.realm);
        let loser = rec.iter().find(|f| f.broke).map_or(NONE, |f| f.realm);
        let killed = rec.iter().map(|f| f.killed).sum();
        let sides = rec.iter().map(|f| (f.realm, f.killed)).collect();
        cx.emit(Event::Battle { province: p, winner, loser, killed, sides });
        self.st.dirty.extend(rec.iter().map(|f| f.realm));
        if cx.traced(p) {
            cx.note(format!("the battle is over: realm {winner} holds the field, {killed} killed"));
        }
    }

    /// Sieges in province `p`, where no battle goes on: `sides` are the companies of each realm
    /// standing there. Returns whether one may go on there.
    #[allow(clippy::too_many_arguments)]
    fn siege(&mut self, w: &mut World, cx: &mut Ctx, p: u32, sides: BTreeMap<u32, Vec<u32>>, people: &[u64], middle: f64) -> bool {
        let pr = &self.params;
        let (min_share, base, capital_siege, max_days, free_siege) = (pr.siege_min_share, pr.siege_base_days, pr.capital_siege, pr.siege_max_days, pr.free_siege);
        let pu = p as usize;
        let (owner, occ) = (w.owner.get(pu).copied().unwrap_or(NONE), w.occupier.get(pu).copied().unwrap_or(NONE));
        let mut keep = false;
        for (x, companies) in sides {
            let field: Vec<u32> = companies.into_iter().filter(|&a| w.armies.role[a as usize] == FIELD).collect();
            if field.is_empty() {
                continue;
            }
            let take = owner != NONE && owner != x && occ != x && diplomacy::at_war(w, x, owner);
            let free = owner == x && occ != NONE;
            if !(take || free) {
                continue;
            }
            keep = true;
            let soldiers: u64 = field.iter().map(|&a| w.armies.size[a as usize]).sum();
            let least = ((min_share * people[pu] as f64).ceil() as u64).max(1);
            if soldiers < least {
                if cx.traced(p) {
                    cx.note(format!("realm {x}'s {soldiers} soldiers are too few to besiege it (at least {least})"));
                }
                continue;
            }
            for &a in &field {
                w.armies.siege_days[a as usize] += 1;
            }
            let done = field.iter().map(|&a| w.armies.siege_days[a as usize]).max().unwrap_or(0);
            let capital = owner != NONE && w.realms.capital.get(owner as usize) == Some(&p);
            let mut need = base * (1.0 + people[pu] as f64 / middle);
            if capital {
                need *= capital_siege;
            }
            need = need.min(max_days);
            if free {
                need *= free_siege;
            }
            let need = (need.round() as u32).max(1);
            if cx.traced(p) {
                cx.note(format!("realm {x} {} it: day {done} of {need}", if free { "frees" } else { "besieges" }));
            }
            if done >= need {
                cx.emit(Event::Occupied { province: p, by: x });
                for &a in &field {
                    w.armies.siege_days[a as usize] = 0;
                }
                self.st.dirty.insert(x);
                break;
            }
        }
        keep
    }

    /// A sea-crossing group arrived at its target: enemy ships standing there fight its fleet, day
    /// by day, worked out now. If the fleet breaks, everyone turns back and the soldiers the
    /// surviving ships can't carry are lost at sea; otherwise the soldiers land and the ships go
    /// back.
    fn land(&mut self, w: &mut World, cx: &mut Ctx, res: &Resolved, g: u32) {
        let Some(grp) = self.st.groups.get(&g).cloned() else { return };
        let (r, t) = (grp.realm, grp.target);
        let from = grp.gather.first().copied().unwrap_or(NONE);
        // Wait for every member that is still on its way.
        if grp.members.iter().any(|&a| w.armies.moving(a as usize) && w.armies.next[a as usize] == t) {
            return;
        }
        let here: Vec<u32> = grp.members.iter().copied().filter(|&a| w.armies.province[a as usize] == t && !w.armies.moving(a as usize)).collect();
        let (ships, troops): (Vec<u32>, Vec<u32>) = here.iter().partition(|&&a| res.kind(w.armies.kind[a as usize]).ship);
        let foes: Vec<(u32, Vec<u32>)> = self.forces(w, res, t, true).into_iter().filter(|(x, _)| diplomacy::at_war(w, r, *x)).collect();
        let mut fleet_broke = false;
        if !foes.is_empty() && !ships.is_empty() {
            let mut sides = vec![(r, ships.clone())];
            sides.extend(foes.iter().cloned());
            let size = |w: &World, c: &[u32]| -> u64 { c.iter().map(|&a| w.armies.size[a as usize]).sum() };
            let engaged: Vec<u64> = sides.iter().map(|(_, c)| size(w, c)).collect();
            let mut killed = vec![0u64; sides.len()];
            let mut enemy_broke = vec![false; sides.len()];
            for round in 1..=200u64 {
                let lost = self.exchange(w, cx, res, t, &sides, round);
                for j in 0..sides.len() {
                    killed[j] += lost[j];
                }
                let broken = |j: usize, w: &World| killed[j] as f64 >= self.params.break_share * engaged[j] as f64 || size(w, &sides[j].1) == 0;
                if broken(0, w) {
                    fleet_broke = true;
                    break;
                }
                for (j, b) in enemy_broke.iter_mut().enumerate().skip(1) {
                    *b = broken(j, w);
                }
                if enemy_broke.iter().skip(1).all(|&b| b) {
                    break;
                }
            }
            if !fleet_broke {
                for (j, (x, c)) in sides.iter().enumerate().skip(1) {
                    self.break_side(w, cx, res, t, *x, c, true);
                    let _ = j;
                }
            }
            let winner = if fleet_broke { foes[0].0 } else { r };
            let loser = if fleet_broke { r } else { foes[0].0 };
            let fought = sides.iter().zip(&killed).map(|((x, _), &k)| (*x, k)).collect();
            cx.emit(Event::Battle { province: t, winner, loser, killed: killed.iter().sum(), sides: fought });
            if cx.traced(t) {
                cx.note(format!("realm {r}'s fleet fought at sea here: {}", if fleet_broke { "it broke" } else { "it won" }));
            }
        }
        let back = || vec![(from, true)];
        if fleet_broke {
            let carry: f64 = ships.iter().map(|&a| w.armies.size[a as usize] as f64 * res.kind(w.armies.kind[a as usize]).carry).sum();
            let soldiers: Vec<u64> = troops.iter().map(|&a| w.armies.size[a as usize]).collect();
            let total: u64 = soldiers.iter().sum();
            let lost = total.saturating_sub(carry.floor() as u64);
            let ids: Vec<u64> = troops.iter().map(|&a| a as u64).collect();
            for (&a, part) in troops.iter().zip(split_proportional(lost, &soldiers, &ids)) {
                self.kill(w, cx, a, part);
            }
            for &a in ships.iter().chain(&troops) {
                self.leave_group(w, a);
                if !self.send(w, res, a, back(), None) {
                    let size = w.armies.size[a as usize];
                    self.go_home(w, cx, a, size, Cause::Returned);
                }
                self.st.rest.insert(a, cx.day + self.params.rest_days);
            }
            self.st.groups.remove(&g);
        } else {
            for &a in &ships {
                self.leave_group(w, a);
                self.send(w, res, a, back(), None);
            }
            if let Some(grp) = self.st.groups.get_mut(&g) {
                grp.sea = false;
            }
        }
        self.st.dirty.insert(r);
    }

    /// The generals of the realms that plan again today.
    fn command(&mut self, w: &mut World, cx: &mut Ctx) {
        let res = self.resolved();
        let dirty = std::mem::take(&mut self.st.dirty);
        let day = cx.day;
        self.st.rest.retain(|_, d| *d > day);
        let enemies = diplomacy::enemies(w);
        let people = w.province_population();
        for r in dirty {
            if !w.realms.alive(r) {
                continue;
            }
            let foes = enemies.get(r as usize).cloned().unwrap_or_default();
            if foes.is_empty() {
                let any = self.ix(w).field_of.get(&r).is_some_and(|v| v.iter().any(|&a| w.armies.size[a as usize] > 0));
                if any {
                    self.stand_down(w, cx, r);
                }
                continue;
            }
            self.plan(w, cx, &res, r, &foes, &people);
        }
    }

    /// Strength of the enemies of a realm (`foes`) standing in province `p`, land troops or ships.
    fn enemy_strength(&self, w: &World, res: &Resolved, p: u32, foes: &[u32], ships: bool) -> f64 {
        let ix = self.ix.as_ref().expect("built before use");
        ix.standing.get(&p).map_or(0.0, |s| {
            s.iter()
                .filter(|&&a| foes.binary_search(&w.armies.realm[a as usize]).is_ok() && res.kind(w.armies.kind[a as usize]).ship == ships)
                .map(|&a| w.armies.size[a as usize] as f64 * res.kind(w.armies.kind[a as usize]).strength)
                .sum()
        })
    }

    /// Realm `r`'s general, at war with `foes` (sorted).
    #[allow(clippy::too_many_arguments)]
    fn plan(&mut self, w: &mut World, cx: &mut Ctx, res: &Resolved, r: u32, foes: &[u32], people: &[u64]) {
        // The settings it needs, copied out (the parameters hold the troop tree).
        struct Knobs {
            outmatch: f64,
            home_edge: f64,
            siege_min_share: f64,
            gather_share: f64,
        }
        let p = &self.params;
        let pr = Knobs { outmatch: p.outmatch, home_edge: p.home_edge, siege_min_share: p.siege_min_share, gather_share: p.gather_share };
        let n = w.provinces.count;
        let day = cx.day;
        // Companies under the player's orders are left alone until they arrive.
        let ordered = std::mem::take(&mut self.st.ordered);
        let fields: Vec<u32> = self.ix(w).field_of.get(&r).map_or(Vec::new(), |v| v.iter().copied().filter(|a| !ordered.contains(a)).collect());
        self.st.ordered = ordered;
        let ok = |w: &World, q: u32| allowed_for(w, r, foes, q);

        // Companies standing on a neutral's land (a former enemy's) go back to their own.
        for &a in &fields {
            let au = a as usize;
            let at = w.armies.province[au];
            if w.armies.size[au] == 0 || w.armies.moving(au) || ok(w, at) {
                continue;
            }
            self.leave_group(w, a);
            let neutral = w.owner[at as usize];
            let ship = res.kind(w.armies.kind[au]).ship;
            let allowed = |q: u32| ok(w, q) || w.owner[q as usize] == neutral;
            let goal = |q: u32| w.owner[q as usize] == r && w.occupier[q as usize] == NONE;
            match route_to(w, at, &allowed, ship, &goal) {
                Some(path) => {
                    self.send(w, res, a, path.into_iter().map(|q| (q, ship)).collect(), None);
                }
                None => {
                    let size = w.armies.size[au];
                    self.go_home(w, cx, a, size, Cause::Returned);
                }
            }
        }

        // Groups whose target is taken, freed or no longer an enemy's are done.
        let valid = |s: &Self, w: &World, t: u32| {
            let (o, occ) = (w.owner[t as usize], w.occupier[t as usize]);
            if o == r {
                occ != NONE || s.enemies_at(w, t, foes)
            } else {
                o != NONE && foes.binary_search(&o).is_ok() && occ != r
            }
        };
        let mine: Vec<u32> = self.st.groups.iter().filter(|(_, g)| g.realm == r).map(|(&k, _)| k).collect();
        for k in &mine {
            let t = self.st.groups[k].target;
            if !valid(self, w, t) {
                self.dissolve(w, *k);
            }
        }
        // Members of a group still gathering that were stopped on their way (by enemies standing
        // in it) go on to a province where it gathers, once no battle holds them.
        for k in &mine {
            let Some(g) = self.st.groups.get(k).cloned() else { continue };
            if g.launched {
                continue;
            }
            for &a in &g.members {
                let au = a as usize;
                let at = w.armies.province[au];
                if w.armies.moving(au) || g.gather.contains(&at) || self.st.battles.contains_key(&at) {
                    continue;
                }
                let ship = res.kind(w.armies.kind[au]).ship;
                let allowed = |q: u32| ok(w, q);
                let goal = |q: u32| g.gather.contains(&q);
                match route_to(w, at, &allowed, ship, &goal) {
                    Some(path) => {
                        self.send(w, res, a, path.into_iter().map(|q| (q, ship)).collect(), None);
                    }
                    None => self.leave_group(w, a),
                }
            }
        }

        // Targets: its own land where enemies stand or that an enemy occupies, then enemy land
        // next to land it holds or occupies, nearest its capital first; then enemy coasts across
        // the sea.
        let capital = w.realms.capital[r as usize];
        let staging = |w: &World, q: u32| (w.owner[q as usize] == r && w.occupier[q as usize] == NONE) || w.occupier[q as usize] == r;
        let near = search(w, &[capital], &|q| ok(w, q), false);
        let mut defend = Vec::new();
        let mut attack = Vec::new();
        let mut sea = Vec::new();
        for q in 0..n as u32 {
            if w.provinces.cells[q as usize] == 0 {
                continue;
            }
            let o = w.owner[q as usize];
            if o == r {
                if w.occupier[q as usize] != NONE || self.enemies_at(w, q, foes) {
                    defend.push(q);
                }
            } else if o != NONE && foes.binary_search(&o).is_ok() && w.occupier[q as usize] != r {
                if w.routes.of(q).any(|l| l.land_days.is_some() && staging(w, l.other(q))) {
                    attack.push(q);
                } else if w.routes.of(q).any(|l| l.sea_days.is_some() && staging(w, l.other(q))) {
                    sea.push(q);
                }
            }
        }
        let by_near = |v: &mut Vec<u32>| v.sort_by_key(|&q| (near.dist[q as usize], q));
        by_near(&mut defend);
        by_near(&mut attack);
        let cap_center = w.provinces.center[capital as usize];
        sea.sort_by(|&a, &b| {
            let d = |q: u32| -dot(w.provinces.center[q as usize], cap_center);
            d(a).total_cmp(&d(b)).then(a.cmp(&b))
        });
        // The provinces its wars were declared over come first after its own land, while it can
        // reach them.
        let wanted: BTreeSet<u32> = (0..w.wars.count())
            .filter(|&k| w.wars.ended[k] == diplomacy::NEVER && w.wars.attacker[k] == r && w.wars.target.get(k).is_some_and(|&t| t != NONE))
            .map(|k| w.wars.target[k])
            .collect();
        let (attack_wanted, attack): (Vec<u32>, Vec<u32>) = attack.into_iter().partition(|q| wanted.contains(q));
        let (sea_wanted, sea): (Vec<u32>, Vec<u32>) = sea.into_iter().partition(|q| wanted.contains(q));
        let targets: Vec<(u32, bool)> = defend
            .into_iter()
            .map(|q| (q, false))
            .chain(attack_wanted.into_iter().map(|q| (q, false)))
            .chain(sea_wanted.into_iter().map(|q| (q, true)))
            .chain(attack.into_iter().map(|q| (q, false)))
            .chain(sea.into_iter().map(|q| (q, true)))
            .collect();

        // Free companies, and how many of the realm's field soldiers each province already holds
        // or expects.
        // Free: in no group, standing, rested, awaiting nothing, and not in a battle.
        let free = |s: &Self, w: &World, a: u32| {
            let au = a as usize;
            w.armies.size[au] > 0
                && w.armies.group[au] == NONE
                && !w.armies.moving(au)
                && s.st.rest.get(&a).is_none_or(|&d| d <= day)
                && !s.st.awaiting.contains_key(&a)
                && !s.st.battles.contains_key(&w.armies.province[au])
        };
        let mut pool: BTreeSet<u32> = fields.iter().copied().filter(|&a| free(self, w, a)).collect();
        let mut load: BTreeMap<u32, u64> = BTreeMap::new();
        for &a in &fields {
            let au = a as usize;
            if w.armies.size[au] > 0 && !res.kind(w.armies.kind[au]).ship {
                let at = if w.armies.moving(au) { w.armies.dest[au] } else { w.armies.province[au] };
                *load.entry(at).or_default() += w.armies.size[au];
            }
        }

        for (t, by_sea) in targets {
            let group = self.st.groups.iter().find(|(_, g)| g.realm == r && g.target == t).map(|(&k, _)| k);
            if group.is_none() && !pool.iter().any(|&a| !res.kind(w.armies.kind[a as usize]).ship) {
                continue;
            }
            if group.is_some_and(|k| self.st.groups[&k].launched) {
                continue;
            }
            let defending = w.owner[t as usize] == r;
            let edge = if defending { 1.0 } else { 1.0 + pr.home_edge };
            let need = pr.outmatch * edge * self.enemy_strength(w, res, t, foes, false);
            let least = ((pr.siege_min_share * people[t as usize] as f64).ceil() as u64).max(1);
            let need_ships = pr.outmatch * self.enemy_strength(w, res, t, foes, true);
            // Where it gathers: provinces it holds or occupies next to the target (across the sea,
            // facing it), with no enemy standing in them.
            let room = |q: u32| (pr.gather_share * people[q as usize] as f64) as i64 - *load.get(&q).unwrap_or(&0) as i64;
            let gather: Vec<u32> = w
                .routes
                .of(t)
                .filter(|l| if by_sea { l.sea_days.is_some() } else { l.land_days.is_some() })
                .map(|l| l.other(t))
                .filter(|&q| staging(w, q) && !self.enemies_at(w, q, foes))
                .collect();
            if gather.is_empty() {
                if let Some(k) = group {
                    pool.extend(self.dissolve(w, k).into_iter().filter(|&a| free(self, w, a)));
                }
                continue;
            }
            let members: Vec<u32> = group.map_or(Vec::new(), |k| self.st.groups[&k].members.clone());
            let (troops, ships): (Vec<u32>, Vec<u32>) = members.iter().partition(|&&a| !res.kind(w.armies.kind[a as usize]).ship);
            let have = Self::strength(w, res, &troops);
            let have_n: u64 = troops.iter().map(|&a| w.armies.size[a as usize]).sum();
            let mut adds: Sends = Vec::new();
            if have < need || have_n < least || members.is_empty() {
                let land_pool: Vec<u32> = pool.iter().copied().filter(|&a| !res.kind(w.armies.kind[a as usize]).ship).collect();
                match self.recruit(w, res, r, foes, &gather, &land_pool, &room, need - have, least.saturating_sub(have_n), false) {
                    Some(chosen) => adds.extend(chosen),
                    None => {
                        if let Some(k) = group {
                            pool.extend(self.dissolve(w, k).into_iter().filter(|&a| free(self, w, a)));
                        }
                        continue;
                    }
                }
            }
            if by_sea {
                // Ships at each province where soldiers gather, enough to carry them across, and in
                // all enough to beat the enemy's ships there.
                let mut short: BTreeMap<u32, f64> = BTreeMap::new();
                let goes = |a: u32, path: Option<&Vec<(u32, bool)>>| match path {
                    Some(p) => p.last().map_or(w.armies.province[a as usize], |s| s.0),
                    None if w.armies.moving(a as usize) => w.armies.dest[a as usize],
                    None => w.armies.province[a as usize],
                };
                for (a, path) in members.iter().map(|&a| (a, None)).chain(adds.iter().map(|(a, p)| (*a, Some(p)))) {
                    let k = res.kind(w.armies.kind[a as usize]);
                    let size = w.armies.size[a as usize] as f64;
                    *short.entry(goes(a, path)).or_default() += if k.ship { -size * k.carry } else { size };
                }
                let mut fleet = Self::strength(w, res, &ships);
                let mut ship_pool: Vec<u32> = pool.iter().copied().filter(|&a| res.kind(w.armies.kind[a as usize]).ship).collect();
                let mut failed = false;
                let mut biggest = (f64::MIN, gather[0]);
                for (&q, &lack) in &short {
                    if lack > biggest.0 {
                        biggest = (lack, q);
                    }
                    if lack <= 0.0 {
                        continue;
                    }
                    match self.recruit_ships(w, res, r, foes, q, &ship_pool, lack, 0.0) {
                        Some(chosen) => {
                            fleet += chosen.iter().map(|(a, _)| Self::strength(w, res, &[*a])).sum::<f64>();
                            ship_pool.retain(|b| !chosen.iter().any(|(a, _)| a == b));
                            adds.extend(chosen);
                        }
                        None => {
                            failed = true;
                            break;
                        }
                    }
                }
                if !failed && fleet < need_ships {
                    match self.recruit_ships(w, res, r, foes, biggest.1, &ship_pool, 0.0, need_ships - fleet) {
                        Some(chosen) => adds.extend(chosen),
                        None => failed = true,
                    }
                }
                if failed {
                    if let Some(k) = group {
                        pool.extend(self.dissolve(w, k).into_iter().filter(|&a| free(self, w, a)));
                    }
                    continue;
                }
            }
            // Commit: make the group if new, and send the new members to gather.
            let k = group.unwrap_or_else(|| {
                let k = self.st.next_group;
                self.st.next_group += 1;
                self.st.groups.insert(k, Group { realm: r, target: t, members: Vec::new(), gather: gather.clone(), launched: false, sea: by_sea });
                k
            });
            if let Some(g) = self.st.groups.get_mut(&k) {
                g.gather = gather.clone();
            }
            for (a, path) in adds {
                pool.remove(&a);
                let au = a as usize;
                if !res.kind(w.armies.kind[au]).ship {
                    let to = path.last().map_or(w.armies.province[au], |s| s.0);
                    *load.entry(to).or_default() += w.armies.size[au];
                }
                w.armies.group[au] = k;
                let g = self.st.groups.get_mut(&k).expect("just made");
                g.members.push(a);
                g.members.sort_unstable();
                self.send(w, res, a, path, None);
            }
            if cx.traced(t) {
                cx.note(format!("realm {r}'s general sends group {k} against it, gathering in {gather:?}: strength needed {need:.0}"));
            }
            self.try_launch(w, res, k, need, least);
        }

        // Free companies with nothing to do go home.
        for a in pool {
            let au = a as usize;
            let (at, home) = (w.armies.province[au], w.armies.home[au]);
            if at == home || w.armies.moving(au) {
                continue;
            }
            let ship = res.kind(w.armies.kind[au]).ship;
            let allowed = |q: u32| ok(w, q);
            let goal = |q: u32| q == home;
            if let Some(path) = route_to(w, at, &allowed, ship, &goal) {
                self.send(w, res, a, path.into_iter().map(|q| (q, ship)).collect(), None);
            }
        }
    }

    /// Group `k` is no more: returns its members, now in no group.
    fn dissolve(&mut self, w: &mut World, k: u32) -> Vec<u32> {
        let Some(g) = self.st.groups.remove(&k) else { return Vec::new() };
        for &a in &g.members {
            w.armies.group[a as usize] = NONE;
            self.st.pace.remove(&a);
        }
        g.members
    }

    /// Free land companies (`pool`) to send to the gathering provinces `gather`, nearest first,
    /// each province taking soldiers while it has room (`room`, less what is sent there now), until
    /// they bring `strength` and `soldiers`. None if they can't.
    #[allow(clippy::too_many_arguments)]
    fn recruit(
        &self,
        w: &World,
        res: &Resolved,
        r: u32,
        foes: &[u32],
        gather: &[u32],
        pool: &[u32],
        room: &dyn Fn(u32) -> i64,
        strength: f64,
        soldiers: u64,
        sea: bool,
    ) -> Option<Sends> {
        let mut left: BTreeMap<u32, i64> = gather.iter().map(|&q| (q, room(q))).collect();
        let mut chosen = Vec::new();
        let (mut got_s, mut got_n) = (0.0, 0u64);
        let mut rest: Vec<u32> = pool.to_vec();
        loop {
            let sources: Vec<u32> = left.iter().filter(|(_, v)| **v > 0).map(|(&q, _)| q).collect();
            if sources.is_empty() || rest.is_empty() {
                return None;
            }
            let s = search(w, &sources, &|q| allowed_for(w, r, foes, q), sea);
            let mut order: Vec<(u64, u32)> = rest.iter().map(|&a| (s.dist[w.armies.province[a as usize] as usize], a)).filter(|(d, _)| *d != u64::MAX).collect();
            order.sort_unstable();
            let mut filled = false;
            for (_, a) in order {
                let at = w.armies.province[a as usize];
                let o = s.origin[at as usize];
                let room = left.get_mut(&o).expect("a source");
                if *room <= 0 {
                    // Filled by an earlier company this round: look again without it.
                    filled = true;
                    continue;
                }
                let size = w.armies.size[a as usize];
                *room -= size as i64;
                got_s += size as f64 * res.kind(w.armies.kind[a as usize]).strength;
                got_n += size;
                chosen.push((a, s.route_back(at).into_iter().map(|q| (q, sea)).collect()));
                rest.retain(|&b| b != a);
                if got_s >= strength && got_n >= soldiers {
                    return Some(chosen);
                }
            }
            if !filled {
                return None;
            }
        }
    }

    /// Free ships (`pool`) to send by sea to `gather`, nearest first, until they carry `carry`
    /// more soldiers and bring `strength` more. Each with its route. None if they can't.
    #[allow(clippy::too_many_arguments)]
    fn recruit_ships(&self, w: &World, res: &Resolved, r: u32, foes: &[u32], gather: u32, pool: &[u32], carry: f64, strength: f64) -> Option<Sends> {
        let s = search(w, &[gather], &|q| allowed_for(w, r, foes, q), true);
        let mut order: Vec<(u64, u32)> = pool.iter().map(|&a| (s.dist[w.armies.province[a as usize] as usize], a)).filter(|(d, _)| *d != u64::MAX).collect();
        order.sort_unstable();
        let (mut got_c, mut got_s) = (0.0, 0.0);
        let mut chosen = Vec::new();
        for (_, a) in order {
            if got_c >= carry && got_s >= strength && !chosen.is_empty() {
                break;
            }
            let k = res.kind(w.armies.kind[a as usize]);
            let size = w.armies.size[a as usize] as f64;
            got_c += size * k.carry;
            got_s += size * k.strength;
            let at = w.armies.province[a as usize];
            chosen.push((a, s.route_back(at).into_iter().map(|q| (q, true)).collect()));
        }
        (got_c >= carry && got_s >= strength && !chosen.is_empty()).then_some(chosen)
    }

    /// Group `k` sets off for its target once all its members have gathered and they still
    /// outmatch the enemies there: everyone at the pace of its slowest.
    fn try_launch(&mut self, w: &mut World, res: &Resolved, k: u32, need: f64, least: u64) {
        let Some(g) = self.st.groups.get(&k).cloned() else { return };
        if g.launched || g.members.is_empty() {
            return;
        }
        let arm = &w.armies;
        let gathered = g.members.iter().all(|&a| !arm.moving(a as usize) && g.gather.contains(&arm.province[a as usize]));
        if !gathered {
            return;
        }
        let troops: Vec<u32> = g.members.iter().copied().filter(|&a| !res.kind(arm.kind[a as usize]).ship).collect();
        let soldiers: u64 = troops.iter().map(|&a| arm.size[a as usize]).sum();
        if troops.is_empty() || Self::strength(w, res, &troops) < need || soldiers < least {
            return;
        }
        if g.sea {
            // Across the sea, the ships at each province must carry the soldiers there.
            let mut lack: BTreeMap<u32, f64> = BTreeMap::new();
            for &a in &g.members {
                let k = res.kind(arm.kind[a as usize]);
                let size = arm.size[a as usize] as f64;
                *lack.entry(arm.province[a as usize]).or_default() += if k.ship { -size * k.carry } else { size };
            }
            if lack.values().any(|&x| x > 0.0) {
                return;
            }
        }
        let pace = g.members.iter().map(|&a| res.kind(arm.kind[a as usize]).speed).fold(f64::INFINITY, f64::min) as f32;
        for &a in &g.members {
            self.send(w, res, a, vec![(g.target, g.sea)], Some(pace));
        }
        if let Some(g) = self.st.groups.get_mut(&k) {
            g.launched = true;
        }
    }
}

/// Whether realm `r` is at war with anyone today (as [`diplomacy::enemies`] has it, without
/// working out everyone's).
fn at_war_with_anyone(w: &World, r: u32) -> bool {
    let wars = &w.wars;
    w.realms.alive(r)
        && (0..wars.count()).any(|k| {
            wars.ended[k] == diplomacy::NEVER && (wars.attacker[k] == r || wars.defender[k] == r) && w.realms.alive(wars.attacker[k]) && w.realms.alive(wars.defender[k])
        })
}

/// The people of the world's middle province, of those with people (at least 1).
fn middle_province(people: &[u64]) -> f64 {
    let mut v: Vec<u64> = people.iter().copied().filter(|&x| x > 0).collect();
    if v.is_empty() {
        return 1.0;
    }
    let k = v.len() / 2;
    (*v.select_nth_unstable(k).1).max(1) as f64
}

/// Whether realm `r`, at war with `foes` (sorted), may pass through province `q`: land it holds or
/// occupies, its enemies' land, or land nobody owns.
fn allowed_for(w: &World, r: u32, foes: &[u32], q: u32) -> bool {
    let o = w.owner.get(q as usize).copied().unwrap_or(NONE);
    o == r || o == NONE || w.occupier.get(q as usize) == Some(&r) || foes.binary_search(&o).is_ok()
}

/// Days of the way between provinces `p` and `q`, by sea or by land, if there is one.
fn way_days(w: &World, p: u32, q: u32, sea: bool) -> Option<f32> {
    let l = w.routes.between(p, q)?;
    if sea { l.sea_days } else { l.land_days }
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
