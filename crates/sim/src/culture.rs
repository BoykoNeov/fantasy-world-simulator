//! Culture "assimilation" (build step 9): peoples spread over regions, and people take up the
//! culture that pulls hardest on them. Being built: the module keeps its fields and does nothing
//! yet.

use crate::fields::Tier;
use crate::module::{Cadence, Manifest, Module, Slot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CultureParams {}

/// One entry per culture, as parallel arrays. Numbers are never reused: a culture with nobody left
/// keeps its entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cultures {
    /// The culture it split from, [`crate::grid::NONE`] for a starting culture (field
    /// `culture.parent`).
    pub parent: Vec<u32>,
    /// The day it began (field `culture.born`).
    pub born: Vec<u64>,
    /// The province where it began (field `culture.hearth`).
    pub hearth: Vec<u32>,
    /// The day its last person was gone, `u64::MAX` while it lives (field `culture.died`).
    pub died: Vec<u64>,
}

impl Cultures {
    pub fn count(&self) -> usize {
        self.parent.len()
    }
}

pub struct Assimilation {
    pub params: CultureParams,
}

static V1: Manifest = Manifest {
    slot: Slot::Culture,
    version: "assimilation",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["pop.province", "pop.culture", "pop.size"],
    reads_earlier: &[],
    owns: &["culture.parent", "culture.born", "culture.hearth", "culture.died", "realm.culture"],
    // People take up another culture by a shift request to the pops' owner.
    requests: &["pop.culture"],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Assimilation {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }
}
