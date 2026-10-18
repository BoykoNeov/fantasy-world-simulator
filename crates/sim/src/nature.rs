//! Natural world "regrowth" (build step 10): each province's wild woodland, fish and game, used up
//! where people crowd and work them and growing back where they don't. Under markets, the woodland
//! and fish left set how much timber and fish the land can give. Being built: the module keeps its
//! fields (all untouched) and does nothing yet.

use crate::fields::Tier;
use crate::module::{Cadence, Ctx, Manifest, Module, Slot};
use crate::world::World;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NatureParams {}

pub struct Regrowth {
    pub params: NatureParams,
}

impl Regrowth {
    pub fn new(params: NatureParams) -> Regrowth {
        Regrowth { params }
    }
}

static V1: Manifest = Manifest {
    slot: Slot::Nature,
    version: "regrowth",
    tier: Tier::Province,
    cadence: Cadence::Monthly,
    reads: &["pop.province", "pop.size"],
    reads_earlier: &[],
    owns: &["province.woods_lost", "province.fish_lost", "province.game_lost"],
    requests: &[],
    asks: &[],
    moves: &[],
    requires: &["pop.size"],
    emits: &[],
    handles: &[],
    state_schema: "none",
    goods: &[],
};

impl Module for Regrowth {
    fn manifest(&self) -> &'static Manifest {
        &V1
    }

    fn init(&mut self, w: &mut World, _cx: &mut Ctx) {
        let n = w.provinces.count;
        (w.woods_lost, w.fish_lost, w.game_lost) = (vec![0; n], vec![0; n], vec![0; n]);
    }
}
