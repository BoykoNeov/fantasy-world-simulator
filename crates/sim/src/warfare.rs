//! Armies (step 7): soldiers taken from the pops, standing in a province, serving a realm.
//!
//! Army numbers are never reused: a disbanded army stays as an entry of no soldiers. Soldiers are
//! people, counted in the people total with the pops (see [`crate::fields::Quantity::fields`]).

/// One entry per army, as parallel arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Armies {
    /// The realm it serves (field `army.realm`).
    pub realm: Vec<u32>,
    /// The province it stands in (field `army.province`).
    pub province: Vec<u32>,
    /// Soldiers (field `army.size`).
    pub size: Vec<u64>,
}

impl Armies {
    pub fn count(&self) -> usize {
        self.size.len()
    }

    pub fn push(&mut self, realm: u32, province: u32, size: u64) {
        self.realm.push(realm);
        self.province.push(province);
        self.size.push(size);
    }
}
