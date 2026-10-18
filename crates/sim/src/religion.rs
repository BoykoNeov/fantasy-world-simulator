//! Religion "conversion" (build step 9): faiths spread over regions, and people take up the faith
//! that pulls hardest on them. Being built: the module keeps its fields and does nothing yet.

use crate::fields::Tier;
use crate::module::{Cadence, Manifest, Module, Slot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReligionParams {}

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
}

pub struct Conversion {
    pub params: ReligionParams,
}

impl Conversion {
    pub fn new(params: ReligionParams) -> Conversion {
        Conversion { params }
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Religion,
    version: "conversion",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["pop.province", "pop.religion", "pop.size"],
    reads_earlier: &[],
    owns: &["religion.parent", "religion.born", "religion.hearth", "religion.died", "realm.religion"],
    // People take up another faith by a shift request to the pops' owner.
    requests: &["pop.religion"],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Conversion {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }
}
