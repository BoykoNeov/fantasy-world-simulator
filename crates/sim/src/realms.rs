//! Realms (step 7): who owns each province, and each realm's own values.
//!
//! Realm numbers are never reused. A dissolved realm stays as an entry whose capital is
//! [`NONE`], as a province merged away stays as an empty placeholder, so requests and events in
//! flight, checkpoints, undo and branching all still find what they name.

use crate::grid::NONE;

/// One entry per realm, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Realms {
    /// The realm's capital province (field `realm.capital`); [`NONE`] for a dissolved realm.
    pub capital: Vec<u32>,
}

impl Realms {
    pub fn count(&self) -> usize {
        self.capital.len()
    }

    /// Whether realm `r` exists and has not been dissolved.
    pub fn alive(&self, r: u32) -> bool {
        self.capital.get(r as usize).is_some_and(|&c| c != NONE)
    }
}
