//! Shared, engine-free code for the renderer spike: both `spike_egui` and `spike_bevy` draw exactly
//! this world, through exactly this projection and picking, so the comparison measures the
//! framework glue and nothing else.

pub mod borders;
pub mod cube;
pub mod view;
pub mod world;

/// Shared WGSL: projection, cube-face lookup, terrain colouring and line expansion. Each app
/// prepends its own binding declarations.
pub const MAP_COMMON_WGSL: &str = include_str!("../../shaders/map_common.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    use cube::lonlat_to_dir;
    use view::Camera;

    #[test]
    fn picking_matches_projection() {
        let w = world::generate(&world::GenParams { n: 64, provinces: 50, factions: 5, ..Default::default() });
        let cam = Camera { lon: 2.9, lat: 0.3, scale: 900.0 };
        let (sw, sh) = (1920.0, 1080.0);
        for k in 0..2000 {
            let (x, y) = ((k * 37 % 1920) as f64 + 0.5, (k * 53 % 1080) as f64 + 0.5);
            let (lon, lat) = cam.screen_to_lonlat(x, y, sw, sh);
            if lat.abs() > std::f64::consts::FRAC_PI_2 {
                continue;
            }
            let (x2, y2) = cam.lonlat_to_screen(lon, lat, sw, sh);
            assert!((x - x2).abs() < 1e-6 && (y - y2).abs() < 1e-6);
            assert_eq!(cam.pick(&w.grid, x, y, sw, sh), Some(w.grid.dir_to_cell(lonlat_to_dir(lon, lat))));
        }
    }

    #[test]
    fn world_generates_with_provinces_everywhere_on_land() {
        let w = world::generate(&world::GenParams { n: 64, provinces: 200, factions: 8, ..Default::default() });
        for (c, &e) in w.elevation.iter().enumerate() {
            assert_eq!(e >= 0.0, w.province[c] != world::NO_PROVINCE);
        }
        let segs = borders::extract_borders(&w);
        assert!(!segs.is_empty());
        for s in &segs {
            assert!((s.a[0] - s.b[0]).abs() < 1.6, "segment spans the seam: {s:?}");
        }
    }
}
