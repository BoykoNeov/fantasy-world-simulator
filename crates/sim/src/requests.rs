//! Change requests: how a module changes a value it does not own.
//!
//! A module asks the owner to remove an amount from one entity's field. Requests made on day N are
//! applied in one pass at the end of day N, owner by owner in run order, each owner's queue in a
//! fixed order. When requests ask for more than exists, the owner rations: priority classes are
//! served in order, and within the class that runs out the rest is split in proportion to what was
//! asked, by the largest-remainder rule. Each grant reaches its requester the next day.

use crate::fields::FieldId;
use crate::module::Slot;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Day the request was made.
    pub day: u64,
    pub requester: Slot,
    pub field: FieldId,
    pub entity: u32,
    /// Amount to remove.
    pub take: u64,
    pub priority: Priority,
    pub cause: Cause,
    /// The requester's own reference, returned with the grant (e.g. which pop asked). Also the
    /// final tie-break when rationing.
    pub tag: u64,
}

impl Request {
    /// The fixed order an owner applies its queue in: field, entity, priority, requester, tag.
    pub fn order_key(&self, requester_rank: usize) -> (FieldId, u32, Priority, usize, u64) {
        (self.field, self.entity, self.priority, requester_rank, self.tag)
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
}
