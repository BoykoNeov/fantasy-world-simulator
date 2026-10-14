//! The map's GPU setup and the per-province table upload that runs when the simulation advances
//! (the "+1 year" button). wgpu panics on validation errors, so a wrong texture size, layout or
//! binding fails here instead of on the user's first click. Skips (with a message) when there is
//! no GPU.

use app::gpu::{MapData, MapGpu};
use app::layers;
use eframe::egui_wgpu::wgpu;
use sim::calendar::Calendar;
use sim::pops::PopParams;
use sim::sim::Sim;
use sim::world::WorldParams;

#[test]
fn build_the_map_then_advance_and_upload_the_tables() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("no GPU adapter: skipping the GPU upload test");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");

    // More than 256 provinces, so the tables have several rows.
    let params = WorldParams { n: 48, provinces: 400, seed: 3, land_fraction: 0.4, radius_km: 6371.0 };
    let mut sim = Sim::new(&params, Calendar::standard(), PopParams::default());
    sim.run_to_year(sim.calendar.def.first_year + 2);
    assert!(sim.world.provinces.count > layers::TABLE_WIDTH);
    let data = MapData::build(&sim);
    let gpu = MapGpu::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, &data);

    let before = layers::province_tables(&sim);
    sim.run_to_year(sim.calendar.def.first_year + 3);
    let after = layers::province_tables(&sim);
    assert_eq!(before.len(), after.len(), "the table size never changes while the world runs");
    assert_ne!(before, after, "a year of growth and famine should change the tables");
    gpu.upload_tables(&queue, &after);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
}
