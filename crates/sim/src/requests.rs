//! Change requests: how a module changes a value it does not own.
//!
//! A module asks the owner to remove an amount from one entity's field. Requests made on day N are
//! applied in one pass at the end of day N, owner by owner in run order, each owner's queue in a
//! fixed order. When requests ask for more than exists, the owner rations: priority classes are
//! served in order, and within the class that runs out the rest is split in proportion to what was
//! asked, by the largest-remainder rule. Each grant reaches its requester the next day.
//!
//! A request on a per-good field asks for a node of the goods tree in one province ("food in
//! province 12"), not for one stored good: the owner serves it from whatever goods it keeps under
//! that node (see [`serve_goods`]), so the requester works whichever goods the owner keeps.
//! Likewise a request for people names a province, never one pop: the owner takes them from all
//! the province's pops (see [`serve_people`]).
//!
//! A request can also ask the owner to *move* what it grants into a value of another field it
//! keeps, instead of using it up (a province's coins into its realm's treasury): see [`Dest`].
//! Nothing then changes hands between modules, and no flow is booked.

use crate::fields::FieldId;
use crate::module::{Ctx, Slot};
use crate::taxonomy::NodeId;
use serde::{Deserialize, Serialize};

/// Served in this order when there isn't enough for everyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Critical,
    /// Keeping people alive: eating.
    Essential,
    Normal,
    Low,
}

/// Why a conserved quantity changed. Every applied request and every reported flow carries one;
/// the inspector reads them to answer "why did this change".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Cause {
    Harvest,
    Spoilage,
    Eating,
    Births,
    Deaths,
    Starvation,
    /// A field contract clamped a value back into bounds.
    ContractClamp,
    /// The player set a value directly (an edit): a declared source or sink.
    Edit,
    /// Goods made by work, other than harvests (step 6, markets).
    Made,
    /// Goods used up to make other goods.
    MadeInto,
    /// Goods people used up.
    Used,
    /// Tools and cloth worn out.
    WornOut,
    /// Goods and coins that came into a province by trade, and those that left it. Every trade is
    /// both, so the two cancel in each class's total.
    TradeIn,
    TradeOut,
    /// Coins a realm takes from its provinces into its treasury (step 7).
    Taxes,
    /// People taken from the pops into an army, booked out by the pops' owner and in by the
    /// army's (a day apart: the grant arrives the next day).
    Raised,
    /// Soldiers who left an army for home (disbanded, deserted, survivors of a lost army): booked
    /// out by the army's owner and in by the pops' when they arrive, the next day.
    Returned,
    /// Coins a realm pays out of its treasury into a province: its spending, or what a dissolved
    /// realm leaves to the land (step 7).
    Spent,
    /// Soldiers who left an army for home because they went hungry: booked out by the army's owner
    /// and in by the pops' when they arrive, the next day, as [`Cause::Returned`] is.
    Deserted,
    /// Goods used up equipping soldiers when they are raised: tools for heavy infantry, timber for
    /// ships.
    Equipped,
    /// Coins a realm pays from its treasury into a province for each soldier raised there (a move,
    /// so no flow is booked).
    Bounty,
    /// Soldiers killed in battle, or lost at sea when their fleet broke.
    Killed,
    // The first wave of modules built side by side (Oct 17), a block each.
    // Logistics and support.
    /// Coins a realm pays its soldiers from its treasury into their home provinces (a move).
    Wages,
    /// Coins a realm carries off from a province it took by siege into its treasury (a move).
    Plunder,
    /// Food soldiers take from the province they stand in, beyond their own supply.
    Foraging,
    /// Soldiers dead of camp disease and winter on campaign.
    Attrition,
    // Research.
    /// Coins a realm pays its scholars from its treasury into its provinces (a move).
    Scholars,
    // Culture (shifts: nobody is made or lost, so no flow).
    /// People who took up another culture.
    Assimilated,
    /// People whose culture split off as a new one.
    SplitOff,
    /// People given their culture or faith when the world was made.
    Placed,
    // The second wave of modules built side by side (Oct 18), a block each.
    // Religion (shifts: nobody is made or lost, so no flow).
    /// People who took up another faith.
    Converted,
    /// People whose faith broke away as a new one.
    Schism,
    // Disease.
    /// People dead of plague.
    Plague,
}

/// Where a request's grant goes when it is moved rather than used up: one value of a field the
/// owner also keeps. For a per-good field, `entity` is the row (province or realm); the grant keeps
/// its goods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Dest {
    pub field: FieldId,
    pub entity: u32,
}

/// What a shift request asks: people whose value of a pop key is `from` take the value `to`,
/// keeping everything else about them (see [`serve_shifts`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Shift {
    pub from: u16,
    pub to: u16,
}

/// The pop keys people can shift between.
pub const SHIFT_FIELDS: &[&str] = &["pop.culture", "pop.religion", "pop.species"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Day the request was made.
    pub day: u64,
    pub requester: Slot,
    pub field: FieldId,
    /// The province for province and per-good fields and for people (`pop.size`, served from all
    /// the province's pops); the realm for realm fields; the army for army fields.
    pub entity: u32,
    /// For per-good fields, the node of the goods tree asked for; `None` for other fields.
    pub good: Option<NodeId>,
    /// Amount to remove.
    pub take: u64,
    pub priority: Priority,
    pub cause: Cause,
    /// The requester's own reference, returned with the grant (e.g. which pop asked). Also the
    /// final tie-break when rationing.
    pub tag: u64,
    /// Move the grant here instead of using it up (see [`Dest`]).
    pub to: Option<Dest>,
    /// For a request on a pop key (`pop.culture`…): `take` people of the province `entity` move
    /// from one value of it to another (see [`Shift`]).
    pub shift: Option<Shift>,
}

impl Request {
    /// The province it is about, if it names one: requests on province and per-province-good
    /// fields, and for people.
    pub fn province(&self) -> Option<u32> {
        use crate::fields::Tier;
        matches!(self.field.def().tier, Tier::Province | Tier::ProvinceGood | Tier::Pop).then_some(self.entity)
    }

    /// The fixed order an owner applies its queue in: field, entity, good, the value a shift is
    /// from, priority, requester, tag.
    #[allow(clippy::type_complexity)]
    pub fn order_key(&self, requester_rank: usize) -> (FieldId, u32, Option<NodeId>, Option<u16>, Priority, usize, u64) {
        (self.field, self.entity, self.good, self.shift.map(|s| s.from), self.priority, requester_rank, self.tag)
    }
}

/// For owners of a per-good field: serve today's requests on it from `values`, the field's values
/// (one per province per good kept). The requests for one node in one province share the goods kept
/// under that node as one pool, rationed by [`ration`]; each grant is taken from those goods in
/// proportion to how much of each there is, by [`split_proportional`]. With one good kept under
/// the node, that is simply rationing its store.
pub fn serve_goods(values: &mut [u64], requests: &[Request], cx: &mut Ctx) {
    serve_goods_into(values, None, requests, cx);
}

/// [`serve_goods`] for requests some of which move what they are granted into `into`, the
/// values of the per-good field their [`Dest`] names (laid out the same way, by the same goods).
/// Each good taken goes into the same good there.
pub fn serve_goods_into(values: &mut [u64], mut into: Option<&mut [u64]>, requests: &[Request], cx: &mut Ctx) {
    let goods = cx.goods;
    for run in requests.chunk_by(|a, b| (a.field, a.entity, a.good) == (b.field, b.entity, b.good)) {
        let node = run[0].good.expect("requests on per-good fields name a good");
        let columns = goods.columns_under(node).expect("checked at configuration: what is asked for is kept whole or in parts");
        let at: Vec<usize> = columns.iter().map(|&c| goods.index(run[0].entity as usize, c)).collect();
        let wants: Vec<_> = run.iter().map(|r| (r.priority, r.take, r.tag)).collect();
        let grants = ration(at.iter().map(|&i| values[i]).sum(), &wants);
        for (r, g) in run.iter().zip(grants) {
            let parts = if let [_] = at[..] {
                vec![g]
            } else {
                let held: Vec<u64> = at.iter().map(|&i| values[i]).collect();
                let ids: Vec<u64> = columns.iter().map(|&c| c as u64).collect();
                split_proportional(g, &held, &ids)
            };
            for (k, &part) in parts.iter().enumerate() {
                values[at[k]] -= part;
            }
            if let Some(dest) = r.to {
                let into = into.as_deref_mut().expect("the owner passes the values its moves go into");
                for (&c, &part) in columns.iter().zip(&parts) {
                    into[goods.index(dest.entity as usize, c)] += part;
                }
            }
            cx.grant(r, g);
        }
    }
}

/// For the owner of the pops: serve today's requests for people (on `pop.size`, each naming a
/// province) from `sizes`, every pop of that province together as one pool, rationed by [`ration`];
/// each grant is taken from the pops in proportion to their size, by [`split_proportional`].
/// `province` is each pop's province. Requests come in the owner's fixed order, so each
/// province's run together.
pub fn serve_people(sizes: &mut [u64], province: &[u32], requests: &[Request], cx: &mut Ctx) {
    let mut pops_of: std::collections::BTreeMap<u32, Vec<usize>> = requests.iter().map(|r| (r.entity, Vec::new())).collect();
    for (k, p) in province.iter().enumerate() {
        if let Some(v) = pops_of.get_mut(p) {
            v.push(k);
        }
    }
    for run in requests.chunk_by(|a, b| (a.field, a.entity) == (b.field, b.entity)) {
        debug_assert!(run[0].to.is_none(), "people are moved between owners by grant and event, not by Dest");
        let pops = &pops_of[&run[0].entity];
        let wants: Vec<_> = run.iter().map(|r| (r.priority, r.take, r.tag)).collect();
        let grants = ration(pops.iter().map(|&k| sizes[k]).sum(), &wants);
        for (r, g) in run.iter().zip(grants) {
            let held: Vec<u64> = pops.iter().map(|&k| sizes[k]).collect();
            let ids: Vec<u64> = pops.iter().map(|&k| k as u64).collect();
            for (&k, part) in pops.iter().zip(split_proportional(g, &held, &ids)) {
                sizes[k] -= part;
            }
            cx.grant(r, g);
        }
    }
}

/// For the owner of the pops: serve today's shift requests (see [`Shift`]), each naming a province
/// and one of the pop keys in [`SHIFT_FIELDS`]. The requests from one value of one key in one
/// province share that province's pops with that value as one pool, rationed by [`ration`]; each
/// grant is taken from those pops in proportion to their size, by [`split_proportional`]. A shift
/// to the value it is from is granted nothing. People taken from a pop join the province's pop
/// that has the same keys but the new value (the lowest numbered, empty ones too); if there is
/// none, the whole pop takes the new value when all of it moves, or else a new pop is added.
/// Empty pops stay, to be used again: grants and saved edits name pops by number. Requests come in
/// the owner's fixed order, so each pool's run together.
pub fn serve_shifts(pops: &mut crate::pops::Pops, requests: &[Request], cx: &mut Ctx) {
    let mut pops_of: std::collections::BTreeMap<u32, Vec<usize>> = requests.iter().map(|r| (r.entity, Vec::new())).collect();
    for (k, p) in pops.province.iter().enumerate() {
        if let Some(v) = pops_of.get_mut(p) {
            v.push(k);
        }
    }
    let pool = |pops: &crate::pops::Pops, of: &[usize], field: &str, from: u16| -> Vec<usize> { of.iter().copied().filter(|&k| key(pops, field, k) == from && pops.size[k] > 0).collect() };
    for run in requests.chunk_by(|a, b| (a.field, a.entity, a.shift.map(|s| s.from)) == (b.field, b.entity, b.shift.map(|s| s.from))) {
        let field = run[0].field.def().name;
        assert!(SHIFT_FIELDS.contains(&field), "{field} is not a pop key people can shift between");
        let from = run[0].shift.expect("requests on pop keys shift people").from;
        let of = pops_of.get_mut(&run[0].entity).expect("listed above");
        let wants: Vec<_> = run.iter().map(|r| (r.priority, if r.shift.is_some_and(|s| s.to == from) { 0 } else { r.take }, r.tag)).collect();
        let grants = ration(pool(pops, of, field, from).iter().map(|&k| pops.size[k]).sum(), &wants);
        for (r, g) in run.iter().zip(grants) {
            // Looked up again: a pop the request before moved whole has left the pool.
            let source = pool(pops, of, field, from);
            let held: Vec<u64> = source.iter().map(|&k| pops.size[k]).collect();
            let ids: Vec<u64> = source.iter().map(|&k| k as u64).collect();
            let to = r.shift.expect("checked above").to;
            for (&k, part) in source.iter().zip(split_proportional(g, &held, &ids)) {
                if part > 0 {
                    shift_part(pops, of, field, k, part, to);
                }
            }
            cx.grant(r, g);
        }
    }
}

/// Pop `k`'s value of the pop key `field`.
fn key(pops: &crate::pops::Pops, field: &str, k: usize) -> u16 {
    match field {
        "pop.culture" => pops.culture[k],
        "pop.religion" => pops.religion[k],
        "pop.species" => pops.species[k],
        other => unreachable!("{other} is not a pop key people can shift between"),
    }
}

/// Move `n` of pop `k`'s people to the value `to` of `field`; `of` is its province's pops. (The
/// culture module's edits use it too, so an edit moves people exactly as a shift request does.)
pub(crate) fn shift_part(pops: &mut crate::pops::Pops, of: &mut Vec<usize>, field: &str, k: usize, n: u64, to: u16) {
    let same = |pops: &crate::pops::Pops, j: usize| {
        key(pops, field, j) == to
            && ["pop.culture", "pop.religion", "pop.species"].iter().filter(|&&f| f != field).all(|&f| key(pops, f, j) == key(pops, f, k))
            && pops.class[j] == pops.class[k]
    };
    if let Some(j) = of.iter().copied().filter(|&j| j != k && same(pops, j)).min() {
        pops.size[k] -= n;
        pops.size[j] += n;
    } else if n == pops.size[k] {
        set_key(pops, field, k, to);
    } else {
        pops.size[k] -= n;
        let j = pops.size.len();
        pops.push(pops.province[k], n);
        (pops.species[j], pops.culture[j], pops.religion[j], pops.class[j]) = (pops.species[k], pops.culture[k], pops.religion[k], pops.class[k]);
        set_key(pops, field, j, to);
        of.push(j);
    }
}

fn set_key(pops: &mut crate::pops::Pops, field: &str, k: usize, to: u16) {
    match field {
        "pop.culture" => pops.culture[k] = to,
        "pop.religion" => pops.religion[k] = to,
        "pop.species" => pops.species[k] = to,
        other => unreachable!("{other} is not a pop key people can shift between"),
    }
}

/// Split `total` whole units in proportion to `weights`, exactly: everyone gets the whole part of
/// their share, and the units left over go one each to the largest fractional remainders, ties
/// going to the smaller `ids`. The result always sums to `total` (or 0 when every weight is 0).
///
/// This is the one rule for dividing a conserved total: rationing, reallocating people when a
/// border moves, splitting into finer categories.
pub fn split_proportional(total: u64, weights: &[u64], ids: &[u64]) -> Vec<u64> {
    assert_eq!(weights.len(), ids.len());
    let sum: u128 = weights.iter().map(|&w| w as u128).sum();
    if sum == 0 {
        return vec![0; weights.len()];
    }
    let mut out = Vec::with_capacity(weights.len());
    let mut rem = Vec::with_capacity(weights.len());
    for (k, &w) in weights.iter().enumerate() {
        let x = total as u128 * w as u128;
        out.push((x / sum) as u64);
        rem.push((x % sum, k));
    }
    let left = total - out.iter().sum::<u64>();
    rem.sort_by(|a, b| b.0.cmp(&a.0).then(ids[a.1].cmp(&ids[b.1])));
    for &(_, k) in rem.iter().take(left as usize) {
        out[k] += 1;
    }
    out
}

/// What each of several requests on one pool gets, given `available`: `(priority, amount, id)`.
/// Classes are served in priority order; the class that can't be served in full shares what is
/// left by [`split_proportional`]; later classes get nothing.
pub fn ration(available: u64, wants: &[(Priority, u64, u64)]) -> Vec<u64> {
    let mut out = vec![0; wants.len()];
    let mut left = available;
    let mut classes: Vec<Priority> = wants.iter().map(|w| w.0).collect();
    classes.sort();
    classes.dedup();
    for class in classes {
        let members: Vec<usize> = (0..wants.len()).filter(|&k| wants[k].0 == class).collect();
        let asked: u64 = members.iter().map(|&k| wants[k].1).sum();
        if asked <= left {
            for &k in &members {
                out[k] = wants[k].1;
            }
            left -= asked;
        } else {
            let weights: Vec<u64> = members.iter().map(|&k| wants[k].1).collect();
            let ids: Vec<u64> = members.iter().map(|&k| wants[k].2).collect();
            for (share, &k) in split_proportional(left, &weights, &ids).into_iter().zip(&members) {
                out[k] = share;
            }
            left = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Priority::*;

    #[test]
    fn proportional_split_is_exact_and_breaks_ties_by_id() {
        // 10 split 1:1:1 is 3.33 each: one unit left over, and all remainders tie, so the smallest
        // id gets it.
        assert_eq!(split_proportional(10, &[5, 5, 5], &[7, 2, 9]), vec![3, 4, 3]);
        // Largest remainder wins before ids matter: 7 * (3/10, 3/10, 4/10) = 2.1, 2.1, 2.8.
        assert_eq!(split_proportional(7, &[3, 3, 4], &[0, 1, 2]), vec![2, 2, 3]);
        assert_eq!(split_proportional(0, &[1, 2], &[0, 1]), vec![0, 0]);
        assert_eq!(split_proportional(5, &[0, 0], &[0, 1]), vec![0, 0]);
        // Never drifts, whatever the numbers.
        for total in [0u64, 1, 17, 1_000_003, u64::MAX / 3] {
            let w = [3, 0, 1_000_000_007, 12, 12, 999];
            let ids = [5, 4, 3, 2, 1, 0];
            assert_eq!(split_proportional(total, &w, &ids).iter().sum::<u64>(), total);
        }
    }

    #[test]
    fn rationing_serves_classes_in_order_and_shares_within_one() {
        // Enough for everyone: everyone gets what they asked.
        assert_eq!(ration(100, &[(Essential, 30, 0), (Normal, 40, 1)]), vec![30, 40]);
        // The essential class is served in full, the normal class shares the 30 left 2:1, and the
        // low class gets nothing.
        let wants = [(Normal, 40, 0), (Essential, 50, 1), (Low, 5, 2), (Normal, 20, 3)];
        assert_eq!(ration(80, &wants), vec![20, 50, 0, 10]);
        // Two equal requests and an odd unit: the smaller id gets it.
        assert_eq!(ration(5, &[(Essential, 10, 8), (Essential, 10, 3)]), vec![2, 3]);
        // Nothing available.
        assert_eq!(ration(0, &[(Critical, 10, 0)]), vec![0]);
    }

    /// Shift requests, served by [`serve_shifts`]: a culture shift `from` → `to` of `take` people
    /// in `province`, on `field`.
    fn shift(field: &str, province: u32, from: u16, to: u16, take: u64, priority: Priority, tag: u64) -> Request {
        Request {
            day: 0,
            requester: Slot::Culture,
            field: crate::fields::known(field),
            entity: province,
            good: None,
            take,
            priority,
            cause: Cause::Assimilated,
            tag,
            to: None,
            shift: Some(Shift { from, to }),
        }
    }

    /// Serve `requests` (sorted as the engine sorts them) and return the grants.
    fn serve(pops: &mut crate::pops::Pops, mut requests: Vec<Request>) -> Vec<u64> {
        requests.sort_by_key(|r| r.order_key(0));
        let (calendar, goods) = (crate::calendar::Calendar::standard(), crate::fields::Goods::default());
        let mut out = crate::module::Outbox::default();
        let mut cx = Ctx { day: 0, seed: 1, calendar: &calendar, goods: &goods, slot: Slot::Population, out: &mut out, trace: None };
        serve_shifts(pops, &requests, &mut cx);
        assert!(out.flows.is_empty(), "nobody is made or lost");
        out.applied.iter().map(|a| a.1).collect()
    }

    /// People per (province, species, culture, religion, class), the totals a shift on one key must
    /// keep for the others.
    fn totals(pops: &crate::pops::Pops, without: &str) -> std::collections::BTreeMap<(u32, u16, u16, u16, u8), u64> {
        let mut t = std::collections::BTreeMap::new();
        for k in 0..pops.size.len() {
            let c = if without == "pop.culture" { 0 } else { pops.culture[k] };
            let r = if without == "pop.religion" { 0 } else { pops.religion[k] };
            *t.entry((pops.province[k], pops.species[k], c, r, pops.class[k])).or_insert(0) += pops.size[k];
        }
        t
    }

    #[test]
    fn shifts_move_people_between_values_of_one_key_exactly() {
        let mut pops = crate::pops::Pops::default();
        // Province 3: 600 of culture 0 in two pops (religions 0 and 1), 50 of culture 2.
        pops.push(3, 400);
        pops.push(3, 200);
        pops.religion[1] = 1;
        pops.push(3, 50);
        pops.culture[2] = 2;
        // Province 5: 100 of culture 0.
        pops.push(5, 100);
        let (before, people) = (totals(&pops, "pop.culture"), pops.size.iter().sum::<u64>());

        // 300 of culture 0 in province 3 take culture 2: taken from both pops by size (200 and
        // 100); religion 0's join the existing pop of culture 2, religion 1's make a new pop.
        let g = serve(&mut pops, vec![shift("pop.culture", 3, 0, 2, 300, Normal, 0)]);
        assert_eq!(g, vec![300]);
        assert_eq!(pops.size, vec![200, 100, 250, 100, 100]);
        assert_eq!((pops.culture[4], pops.religion[4], pops.province[4]), (2, 1, 3));
        assert_eq!((totals(&pops, "pop.culture"), pops.size.iter().sum::<u64>()), (before.clone(), people));

        // All of province 5 takes culture 7: no pop has it, so the pop takes it in place.
        assert_eq!(serve(&mut pops, vec![shift("pop.culture", 5, 0, 7, 100, Normal, 0)]), vec![100]);
        assert_eq!((pops.size.len(), pops.culture[3], pops.size[3]), (5, 7, 100));

        // Asking for more than there is: the essential request first, the normal ones share the
        // rest by what they asked. A shift to the value it is from gets nothing.
        let mut pops2 = crate::pops::Pops::default();
        pops2.push(1, 100);
        let g = serve(
            &mut pops2,
            vec![shift("pop.culture", 1, 0, 4, 60, Essential, 0), shift("pop.culture", 1, 0, 5, 60, Normal, 1), shift("pop.culture", 1, 0, 6, 20, Normal, 2), shift("pop.culture", 1, 0, 0, 9, Normal, 3)],
        );
        assert_eq!(g, vec![60, 30, 10, 0]);
        assert_eq!(pops2.size.iter().sum::<u64>(), 100);
        // The last 10 of culture 0 took culture 6 in place, so no pop was left empty.
        assert_eq!((pops2.culture[0], pops2.size[0], pops2.size.len()), (6, 10, 3));
        // An empty pop is used again: people taking its values join it.
        let mut pops3 = crate::pops::Pops::default();
        pops3.push(1, 100);
        pops3.push(1, 0);
        pops3.culture[1] = 3;
        let g = serve(&mut pops3, vec![shift("pop.culture", 1, 0, 3, 40, Normal, 0)]);
        assert_eq!((g, pops3.size.clone()), (vec![40], vec![60, 40]));

        // The same on religion keeps every culture's people.
        let before = totals(&pops, "pop.religion");
        serve(&mut pops, vec![shift("pop.religion", 3, 1, 0, 150, Normal, 0)]);
        assert_eq!(totals(&pops, "pop.religion"), before);
    }
}
