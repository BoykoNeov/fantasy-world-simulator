//! Research "tree" (build step 9): realms study and learn techs from a tree. Being built: the
//! module keeps its fields and does nothing yet.

use crate::config::ModuleConfig;
use crate::fields::Tier;
use crate::module::{Cadence, Manifest, Module, Slot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResearchParams {}

/// The techs of the research tree, in the order of `realm.tech`'s columns (see
/// [`crate::world::World::techs`]); none without Research.
pub fn tech_names(config: &ModuleConfig) -> Vec<String> {
    let _ = &config.research;
    Vec::new()
}

pub struct Research {
    pub params: ResearchParams,
    /// Whether the world keeps coins (scholars are then paid from the treasury).
    pub coins: bool,
}

impl Research {
    pub fn new(params: ResearchParams, coins: bool) -> Research {
        Research { params, coins }
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Research,
    version: "tree",
    tier: Tier::Realm,
    cadence: Cadence::Monthly,
    reads: &["pop.province", "pop.size"],
    reads_earlier: &[],
    owns: &["realm.knowledge", "realm.studying", "realm.tech"],
    requests: &[],
    asks: &[],
    moves: &[],
    // Knowledge belongs to realms and grows with their people.
    requires: &["realm.capital", "pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Research {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }
}
