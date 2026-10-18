//! Disease "plagues" (build step 10): plagues break out, spread along the ways between provinces
//! (faster by sea and between big towns), kill a share of the sick and leave survivors immune for
//! years. Being built: the module keeps its fields and does nothing yet.

use crate::fields::Tier;
use crate::grid::NONE;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::world::World;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiseaseParams {}

pub struct Plagues {
    pub params: DiseaseParams,
}

impl Plagues {
    pub fn new(params: DiseaseParams) -> Plagues {
        Plagues { params }
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Disease,
    version: "plagues",
    tier: Tier::Province,
    cadence: Cadence::Weekly,
    reads: &["pop.province", "pop.size"],
    reads_earlier: &[],
    owns: &["province.sick", "province.immune", "province.plague_dead", "province.outbreak"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Plagues {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let n = w.provinces.count;
        (w.sick, w.immune, w.plague_dead, w.outbreak) = (vec![0; n], vec![0; n], vec![0; n], vec![NONE; n]);
    }
}
