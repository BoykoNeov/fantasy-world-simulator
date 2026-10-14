//! Fantasy planet simulation core. Plain Rust: no graphics, no engine, runs headless.

// Fields are parallel arrays indexed by cell, province or pop id; indexing several of them by one
// id reads better than zipped iterators.
#![allow(clippy::needless_range_loop)]

pub mod biome;
pub mod branch;
pub mod calendar;
pub mod climate;
pub mod config;
pub mod economy;
pub mod edit;
pub mod events;
pub mod export;
pub mod fields;
pub mod grid;
pub mod hydrology;
pub mod module;
pub mod pops;
pub mod provinces;
pub mod requests;
pub mod rng;
pub mod save;
pub mod scenario;
pub mod sim;
pub mod terrain;
pub mod weather;
pub mod world;
