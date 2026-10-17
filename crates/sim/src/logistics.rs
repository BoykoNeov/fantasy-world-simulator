//! Logistics and support (build step 8): supply lines and foraging, winter, soldiers' pay and
//! sieges in depth, as four parts of Warfare that can each be switched on or off (Warfare
//! "campaigns" is "troops" with all four). Being built: the parts do nothing yet.

use serde::{Deserialize, Serialize};

/// Supply lines and foraging (`modules.warfare.supply`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SupplyParams {}

/// Winter (`modules.warfare.seasons`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeasonParams {}

/// Soldiers' pay, where the world keeps coins (`modules.warfare.pay`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PayParams {}

/// Sieges in depth (`modules.warfare.sieges`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiegeParams {}
