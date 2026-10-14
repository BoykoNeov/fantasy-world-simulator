//! The map view. The simulation lives in the `sim` crate; this crate only reads its fields and draws
//! them with egui + wgpu.

pub mod bench;
pub mod geometry;
pub mod gpu;
pub mod inspector;
pub mod layers;
pub mod playback;
pub mod projection;
pub mod wobble;
