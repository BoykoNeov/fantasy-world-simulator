//! Flat map projections, the map camera and picking. The same maths runs per pixel in `map.wgsl`;
//! keep the two in sync (the GPU picking test compares them).
//!
//! Projected coordinates are in planet radii: x grows east, y grows north. Longitudes are taken
//! relative to the central meridian, so turning the world under the map is just a change of that
//! meridian.

use crate::wobble::ProvinceLookup;
use sim::grid::{lonlat_to_dir, wrap_lon, Grid, NONE};
use std::f64::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// Equal Earth (Šavrič, Patterson & Jenny, 2018): an oval world map that keeps true areas.
    /// Panning sideways turns the world under the map.
    EqualEarth,
    /// Equirectangular: longitude and latitude straight onto x and y. It repeats sideways like tiles.
    Rectangle,
}

// Equal Earth polynomial coefficients.
const A1: f64 = 1.340264;
const A2: f64 = -0.081106;
const A3: f64 = 0.000893;
const A4: f64 = 0.003796;
/// √3 / 2: the parametric latitude is θ = asin(M · sin φ).
const M: f64 = 0.866_025_403_784_438_6;

fn ee_y(t: f64) -> f64 {
    let t2 = t * t;
    t * (A1 + A2 * t2 + t2 * t2 * t2 * (A3 + A4 * t2))
}

fn ee_dy(t: f64) -> f64 {
    let t2 = t * t;
    A1 + 3.0 * A2 * t2 + t2 * t2 * t2 * (7.0 * A3 + 9.0 * A4 * t2)
}

/// Parametric latitude θ for a projected y (clamped to the map).
fn ee_theta(y: f64) -> f64 {
    let y = y.clamp(-Projection::EqualEarth.half_height(), Projection::EqualEarth.half_height());
    let mut t = y / A1;
    for _ in 0..8 {
        t -= (ee_y(t) - y) / ee_dy(t);
    }
    t
}

impl Projection {
    pub fn index(self) -> u32 {
        match self {
            Projection::EqualEarth => 0,
            Projection::Rectangle => 1,
        }
    }

    /// Whether the map repeats sideways instead of having an outline.
    pub fn wraps(self) -> bool {
        self == Projection::Rectangle
    }

    /// Projected y of the poles.
    pub fn half_height(self) -> f64 {
        match self {
            Projection::EqualEarth => 1.317_362_759_157_4,
            Projection::Rectangle => FRAC_PI_2,
        }
    }

    /// Projected x of the outline on the equator (for the rectangle: one copy of the world).
    pub fn half_width(self) -> f64 {
        match self {
            Projection::EqualEarth => PI / (M * A1),
            Projection::Rectangle => PI,
        }
    }

    /// Projected x per radian of longitude at a latitude. x is linear in longitude for both.
    pub fn x_per_lon(self, lat: f64) -> f64 {
        match self {
            Projection::EqualEarth => {
                let t = (M * lat.sin()).asin();
                t.cos() / (M * ee_dy(t))
            }
            Projection::Rectangle => 1.0,
        }
    }

    /// (x, y) for a longitude relative to the central meridian, and a latitude.
    pub fn forward(self, dlon: f64, lat: f64) -> (f64, f64) {
        match self {
            Projection::EqualEarth => {
                let t = (M * lat.sin()).asin();
                (dlon * t.cos() / (M * ee_dy(t)), ee_y(t))
            }
            Projection::Rectangle => (dlon, lat),
        }
    }

    /// Latitude at a projected y (clamped to the poles).
    pub fn lat_at(self, y: f64) -> f64 {
        match self {
            Projection::EqualEarth => (ee_theta(y).sin() / M).clamp(-1.0, 1.0).asin(),
            Projection::Rectangle => y.clamp(-FRAC_PI_2, FRAC_PI_2),
        }
    }

    /// (longitude relative to the central meridian, latitude), or `None` outside the map. For the
    /// rectangle the longitude is not wrapped.
    pub fn inverse(self, x: f64, y: f64) -> Option<(f64, f64)> {
        // A little slack, so points exactly on the outline survive rounding.
        const SLACK: f64 = 1e-9;
        if y.abs() > self.half_height() + SLACK {
            return None;
        }
        let lat = self.lat_at(y);
        let dlon = x / self.x_per_lon(lat);
        (self.wraps() || dlon.abs() <= PI + SLACK).then_some((dlon, lat))
    }
}

/// What the map view is looking at. Screen coordinates are physical pixels from the map's top-left.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub projection: Projection,
    /// Central meridian, radians.
    pub lon0: f64,
    /// Projected y at the centre of the view.
    pub cy: f64,
    /// Physical pixels per projected unit.
    pub scale: f64,
}

/// Zoom limits, relative to the scale at which the whole planet's height fills the view.
const MIN_ZOOM: f64 = 0.6;
const MAX_ZOOM: f64 = 100.0;

impl Camera {
    /// The whole world in view, centred.
    pub fn fit(projection: Projection, w: f64, h: f64) -> Self {
        let tall = h * 0.94 / (2.0 * projection.half_height());
        let scale = if projection.wraps() { tall } else { tall.min(w * 0.96 / (2.0 * projection.half_width())) };
        Camera { projection, lon0: 0.0, cy: 0.0, scale }
    }

    /// Scale at which the planet's full height fills a view `h` pixels tall.
    pub fn reference_scale(&self, h: f64) -> f64 {
        h / (2.0 * self.projection.half_height())
    }

    /// Zoom relative to [`reference_scale`](Self::reference_scale).
    pub fn zoom(&self, h: f64) -> f64 {
        self.scale / self.reference_scale(h)
    }

    pub fn screen_to_projected(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        ((x - w * 0.5) / self.scale, self.cy - (y - h * 0.5) / self.scale)
    }

    /// (longitude, latitude) under a screen point, or `None` off the map.
    pub fn screen_to_lonlat(&self, x: f64, y: f64, w: f64, h: f64) -> Option<(f64, f64)> {
        let (px, py) = self.screen_to_projected(x, y, w, h);
        self.projection.inverse(px, py).map(|(dl, lat)| (wrap_lon(self.lon0 + dl), lat))
    }

    /// Screen position of a point (for the rectangle: the copy nearest the view centre).
    pub fn lonlat_to_screen(&self, lon: f64, lat: f64, w: f64, h: f64) -> (f64, f64) {
        let (px, py) = self.projection.forward(wrap_lon(lon - self.lon0), lat);
        (px * self.scale + w * 0.5, (self.cy - py) * self.scale + h * 0.5)
    }

    pub fn pick(&self, grid: &Grid, x: f64, y: f64, w: f64, h: f64) -> Option<usize> {
        self.screen_to_lonlat(x, y, w, h).map(|(lon, lat)| grid.dir_to_cell(lonlat_to_dir(lon, lat)))
    }

    /// The province drawn under a pixel, by the shader's rule (`province_at` after `warped` in
    /// `map.wgsl`): `None` off the map or where the point is mostly sea.
    pub fn pick_province(&self, look: &ProvinceLookup, x: f64, y: f64, w: f64, h: f64) -> Option<u32> {
        let (lon, lat) = self.screen_to_lonlat(x, y, w, h)?;
        Some(look.at(lonlat_to_dir(lon, lat))).filter(|&p| p != NONE)
    }

    /// Drag by (dx, dy) pixels. The point grabbed at screen row `grab_y` follows the cursor exactly:
    /// sideways drags turn the world under the map, up and down drags slide it.
    pub fn pan(&mut self, dx: f64, dy: f64, grab_y: f64, h: f64) {
        let lat = self.projection.lat_at(self.cy - (grab_y - h * 0.5) / self.scale);
        self.lon0 = wrap_lon(self.lon0 - dx / (self.scale * self.projection.x_per_lon(lat)));
        self.cy = self.clamp_cy(self.cy + dy / self.scale);
    }

    /// Zoom by `factor`, keeping the point under the cursor in place.
    pub fn zoom_at(&mut self, factor: f64, x: f64, y: f64, w: f64, h: f64) {
        let (px, py) = self.screen_to_projected(x, y, w, h);
        let reference = self.reference_scale(h);
        let old = self.scale;
        self.scale = (self.scale * factor).clamp(reference * MIN_ZOOM, reference * MAX_ZOOM);
        // y is untouched by the meridian, so the cursor row maps back exactly; x is linear in
        // longitude at a fixed latitude, so turning by the right amount keeps the cursor column.
        self.cy = self.clamp_cy(py + (y - h * 0.5) / self.scale);
        let dlon = px / self.projection.x_per_lon(self.projection.lat_at(py));
        self.lon0 = wrap_lon(self.lon0 + dlon * (1.0 - old / self.scale));
    }

    fn clamp_cy(&self, cy: f64) -> f64 {
        let hh = self.projection.half_height();
        cy.clamp(-hh, hh)
    }

    /// Switch projection, keeping the view centre's latitude and the zoom.
    pub fn with_projection(&self, projection: Projection, h: f64) -> Camera {
        let lat = self.projection.lat_at(self.cy);
        let zoom = self.zoom(h);
        let mut cam = Camera { projection, lon0: self.lon0, cy: 0.0, scale: 0.0 };
        cam.scale = cam.reference_scale(h) * zoom;
        cam.cy = projection.forward(0.0, lat).1;
        cam
    }

    /// Province borders fade in as you zoom in.
    pub fn province_line_alpha(&self, h: f64) -> f64 {
        ((self.zoom(h) - 1.6) / 2.0).clamp(0.0, 1.0)
    }

    /// For the repeating rectangle: the first and last copy of the world that reach the screen.
    /// Copy k shows longitudes lon0 + [-π, π) + 2πk.
    pub fn visible_copies(&self, w: f64) -> (i32, i32) {
        if !self.projection.wraps() {
            return (0, 0);
        }
        let half = w * 0.5 / self.scale; // projected units from the centre to the edge
        let first = ((-half + PI) / (2.0 * PI)).floor() as i32;
        let last = ((half + PI) / (2.0 * PI)).floor() as i32;
        (first, last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Projection; 2] = [Projection::EqualEarth, Projection::Rectangle];

    #[test]
    fn equal_earth_known_values() {
        // The published extent of the Equal Earth map on a unit sphere.
        assert!((ee_y(PI / 3.0) - Projection::EqualEarth.half_height()).abs() < 1e-9);
        assert!((Projection::EqualEarth.half_width() - 2.706_629_6).abs() < 1e-6);
        let (x, y) = Projection::EqualEarth.forward(PI, 0.0);
        assert!((x - Projection::EqualEarth.half_width()).abs() < 1e-12 && y == 0.0);
        let (_, y) = Projection::EqualEarth.forward(0.0, FRAC_PI_2);
        assert!((y - Projection::EqualEarth.half_height()).abs() < 1e-9);
    }

    #[test]
    fn forward_then_inverse_returns_the_start() {
        for p in BOTH {
            for k in 0..=40 {
                let lat = -FRAC_PI_2 + PI * k as f64 / 40.0;
                for &dl in &[-PI, -2.0, -0.3, 0.0, 1e-4, 1.7, PI - 1e-9] {
                    let (x, y) = p.forward(dl, lat);
                    let (dl2, lat2) = p.inverse(x, y).unwrap_or_else(|| panic!("{p:?} ({dl}, {lat}) fell off the map"));
                    // Latitude comes back through an arcsine, which loses precision right at the
                    // poles (1e-7 rad is under a kilometre).
                    let tol = if lat.abs() > FRAC_PI_2 - 1e-3 { 1e-6 } else { 1e-9 };
                    assert!((lat - lat2).abs() < tol, "{p:?} lat {lat} -> {lat2}");
                    // Longitude is meaningless at the exact poles.
                    if lat.abs() < FRAC_PI_2 - 1e-9 {
                        assert!((dl - dl2).abs() < 1e-9, "{p:?} dlon {dl} -> {dl2} at lat {lat}");
                    }
                }
            }
        }
    }

    #[test]
    fn outside_the_oval_is_off_the_map() {
        let p = Projection::EqualEarth;
        assert!(p.inverse(p.half_width() + 1e-6, 0.0).is_none());
        assert!(p.inverse(0.0, p.half_height() + 1e-6).is_none());
        assert!(p.inverse(p.half_width() * 0.99, p.half_height() * 0.9).is_none(), "the outline curves in toward the poles");
        assert!(Projection::Rectangle.inverse(50.0, 0.0).is_some(), "the rectangle repeats sideways");
    }

    #[test]
    fn zoom_keeps_the_cursor_point_fixed() {
        let (w, h) = (1600.0, 900.0);
        for p in BOTH {
            for &(x, y) in &[(800.0, 450.0), (100.0, 200.0), (1500.0, 800.0), (950.0, 120.0)] {
                for &f in &[1.7, 0.6] {
                    let mut cam = Camera::fit(p, w, h);
                    cam.zoom_at(3.0, 700.0, 400.0, w, h);
                    cam.lon0 = 2.5;
                    let before = cam.screen_to_lonlat(x, y, w, h);
                    cam.zoom_at(f, x, y, w, h);
                    let (Some((lon, lat)), Some((lon2, lat2))) = (before, cam.screen_to_lonlat(x, y, w, h)) else { continue };
                    assert!((lat - lat2).abs() < 1e-9 && wrap_lon(lon - lon2).abs() < 1e-9, "{p:?} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn dragging_keeps_the_grabbed_point_under_the_cursor() {
        let (w, h) = (1600.0, 900.0);
        for p in BOTH {
            let mut cam = Camera::fit(p, w, h);
            cam.zoom_at(4.0, 800.0, 300.0, w, h);
            let (x, y) = (900.0, 380.0);
            let (lon, lat) = cam.screen_to_lonlat(x, y, w, h).unwrap();
            cam.pan(-37.0, 22.0, y, h);
            let (x2, y2) = cam.lonlat_to_screen(lon, lat, w, h);
            assert!((x2 - (x - 37.0)).abs() < 1e-6 && (y2 - (y + 22.0)).abs() < 1e-6, "{p:?}: ({x2}, {y2})");
        }
    }

    #[test]
    fn switching_projection_keeps_centre_latitude_and_zoom() {
        let (w, h) = (1600.0, 900.0);
        let mut cam = Camera::fit(Projection::EqualEarth, w, h);
        cam.zoom_at(5.0, 800.0, 200.0, w, h);
        let rect = cam.with_projection(Projection::Rectangle, h);
        let back = rect.with_projection(Projection::EqualEarth, h);
        assert!((back.cy - cam.cy).abs() < 1e-9 && (back.scale - cam.scale).abs() < 1e-9);
        assert!((rect.zoom(h) - cam.zoom(h)).abs() < 1e-12);
    }

    #[test]
    fn visible_copies_cover_the_screen() {
        let mut cam = Camera::fit(Projection::Rectangle, 4000.0, 600.0);
        cam.scale = 300.0; // one copy is ~1885 px wide, the screen 4000
        let (a, b) = cam.visible_copies(4000.0);
        assert!(a <= -1 && b >= 1, "{a}..{b}");
        cam.scale = 3000.0;
        assert_eq!(cam.visible_copies(4000.0), (0, 0));
        assert_eq!(Camera::fit(Projection::EqualEarth, 4000.0, 600.0).visible_copies(4000.0), (0, 0));
    }
}
