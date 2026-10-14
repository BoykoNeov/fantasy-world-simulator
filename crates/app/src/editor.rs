//! The editor's tools: what each one is set to, the stroke being drawn, and the edit a finished
//! stroke makes. The window draws the tools' panel and the brush, and sends the edits.
//!
//! Brush tools draw with the left mouse button; the right button still drags the map. A click is a
//! stroke of one point.

use eframe::egui;
use sim::biome::{Biome, ALL};
use sim::edit::{brush_cells, Brush, Edit, Override};
use sim::grid::NONE;
use sim::world::{World, RIVER_MIN_FLOW};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Click a province to inspect it; drag to move the map.
    Inspect,
    Height,
    Climate,
    River,
    Provinces,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeightKind {
    Raise,
    Lower,
    Smooth,
    Roughen,
    Flatten,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClimateKind {
    Temperature,
    Rainfall,
    Biome,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvinceKind {
    /// Land the brush covers joins the selected province.
    MoveInto,
    /// Land the brush covers becomes a new province.
    Split,
    /// A clicked province joins the selected one.
    Merge,
}

pub struct Editor {
    pub tool: Tool,
    pub height: HeightKind,
    pub metres: f32,
    pub smooth: f32,
    pub flatten_to: f32,
    pub radius_km: f64,
    pub climate: ClimateKind,
    pub temperature: f32,
    pub rainfall: f32,
    pub biome: Biome,
    /// A drawn river's spring, in big rivers' worth of water (0 only redirects water).
    pub spring: f64,
    pub provinces: ProvinceKind,
    pub sea_rise: f32,
    /// The stroke being drawn: longitude and latitude, degrees.
    pub stroke: Vec<[f64; 2]>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            tool: Tool::Inspect,
            height: HeightKind::Raise,
            metres: 500.0,
            smooth: 0.5,
            flatten_to: 200.0,
            radius_km: 250.0,
            climate: ClimateKind::Biome,
            temperature: 15.0,
            rainfall: 800.0,
            biome: Biome::Grassland,
            spring: 1.5,
            provinces: ProvinceKind::MoveInto,
            sea_rise: 20.0,
            stroke: Vec::new(),
        }
    }
}

impl Editor {
    /// Whether dragging with the left button draws (rather than moving the map).
    pub fn draws(&self) -> bool {
        self.tool != Tool::Inspect && !(self.tool == Tool::Provinces && self.provinces == ProvinceKind::Merge)
    }

    /// The brush's radius to show around the pointer, if the tool has one.
    pub fn brush_radius_km(&self) -> Option<f64> {
        matches!(self.tool, Tool::Height | Tool::Climate | Tool::Provinces).then_some(self.radius_km).filter(|_| self.draws())
    }

    /// Add a point to the stroke being drawn, keeping points at most a quarter of the brush apart
    /// (the simulation wants at most half) by filling gaps.
    pub fn add_point(&mut self, lon: f64, lat: f64, planet_km: f64) {
        let p = [lon.to_degrees(), lat.to_degrees()];
        let Some(&last) = self.stroke.last() else {
            self.stroke.push(p);
            return;
        };
        let to = |q: [f64; 2]| sim::grid::lonlat_to_dir(q[0].to_radians(), q[1].to_radians());
        let angle = sim::grid::dot(to(last), to(p)).clamp(-1.0, 1.0).acos();
        let spacing = if self.tool == Tool::River { 0.25 * 100.0 } else { 0.25 * self.radius_km } / planet_km;
        if angle < spacing * 0.5 {
            return;
        }
        let steps = (angle / spacing).ceil().max(1.0) as usize;
        for k in 1..=steps {
            let t = k as f64 / steps as f64;
            // Longitude the short way round.
            let dlon = (p[0] - last[0] + 540.0).rem_euclid(360.0) - 180.0;
            self.stroke.push([last[0] + dlon * t, last[1] + (p[1] - last[1]) * t]);
        }
    }

    /// The edit the finished stroke makes, or why there is none. `selected` is the province being
    /// inspected; `clicked` the province under a click.
    pub fn finish(&mut self, w: &World, selected: Option<u32>) -> Option<Result<Edit, String>> {
        let path = std::mem::take(&mut self.stroke);
        if path.is_empty() {
            return None;
        }
        let radius_km = self.radius_km;
        Some(match self.tool {
            Tool::Inspect => return None,
            Tool::Height => Ok(Edit::Paint {
                path,
                radius_km,
                brush: match self.height {
                    HeightKind::Raise => Brush::Raise(self.metres),
                    HeightKind::Lower => Brush::Lower(self.metres),
                    HeightKind::Smooth => Brush::Smooth(self.smooth),
                    HeightKind::Roughen => Brush::Roughen(self.metres),
                    HeightKind::Flatten => Brush::Flatten(self.flatten_to),
                },
            }),
            Tool::Climate => Ok(Edit::Override {
                path,
                radius_km,
                set: match self.climate {
                    ClimateKind::Temperature => Override::Temperature(self.temperature),
                    ClimateKind::Rainfall => Override::Rainfall(self.rainfall),
                    ClimateKind::Biome => Override::Biome(self.biome as u8),
                    ClimateKind::Clear => Override::Clear,
                },
            }),
            Tool::River if path.len() < 2 => Err("drag from the river's source to its mouth".into()),
            Tool::River => Ok(Edit::River { path, spring: (self.spring * RIVER_MIN_FLOW as f64).round() as u64 }),
            Tool::Provinces => {
                let land: Vec<u32> = brush_cells(w, &path, radius_km).into_iter().filter(|&c| w.provinces.of_cell[c as usize] != NONE).collect();
                if land.is_empty() {
                    return Some(Err("the brush covered no land".into()));
                }
                match (self.provinces, selected) {
                    (ProvinceKind::Split, _) => Ok(Edit::Split { cells: land }),
                    (ProvinceKind::MoveInto, Some(to)) => Ok(Edit::MoveCells { cells: land, to }),
                    (ProvinceKind::MoveInto, None) => Err("select the province to move land into first (inspect tool)".into()),
                    (ProvinceKind::Merge, _) => return None,
                }
            }
        })
    }

    /// The edit a click on province `clicked` makes, for tools that act on clicks.
    pub fn click(&self, clicked: Option<u32>, selected: Option<u32>) -> Option<Result<Edit, String>> {
        if self.tool != Tool::Provinces || self.provinces != ProvinceKind::Merge {
            return None;
        }
        Some(match (clicked, selected) {
            (_, None) => Err("select the province to merge into first (inspect tool)".into()),
            (None, _) => Err("click a province".into()),
            (Some(from), Some(into)) => Ok(Edit::Merge { from, into }),
        })
    }

    /// The tools' panel.
    pub fn panel(&mut self, ui: &mut egui::Ui, selected: Option<u32>) -> Option<Edit> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for (tool, name) in [(Tool::Inspect, "inspect"), (Tool::Height, "height"), (Tool::Climate, "climate"), (Tool::River, "rivers"), (Tool::Provinces, "provinces")] {
                if ui.selectable_label(self.tool == tool, name).clicked() {
                    self.tool = tool;
                    self.stroke.clear();
                }
            }
        });
        let radius = |ui: &mut egui::Ui, r: &mut f64| {
            ui.add(egui::Slider::new(r, 30.0..=2000.0).logarithmic(true).text("brush radius, km").integer());
        };
        match self.tool {
            Tool::Inspect => {
                ui.small("Click a province to inspect it. Drag to move the map.");
            }
            Tool::Height => {
                ui.horizontal_wrapped(|ui| {
                    for (k, name) in [(HeightKind::Raise, "raise"), (HeightKind::Lower, "lower"), (HeightKind::Smooth, "smooth"), (HeightKind::Roughen, "roughen"), (HeightKind::Flatten, "flatten")] {
                        ui.radio_value(&mut self.height, k, name);
                    }
                });
                match self.height {
                    HeightKind::Smooth => {
                        ui.add(egui::Slider::new(&mut self.smooth, 0.05..=1.0).text("how much"));
                    }
                    HeightKind::Flatten => {
                        ui.add(egui::Slider::new(&mut self.flatten_to, -3000.0..=5000.0).text("to height, m (below 0 is sea)").integer());
                    }
                    _ => {
                        ui.add(egui::Slider::new(&mut self.metres, 10.0..=3000.0).logarithmic(true).text("metres").integer());
                    }
                }
                radius(ui, &mut self.radius_km);
                ui.small("Drag on the map to paint; right-drag moves the map. Lakes: flatten below 0 m.");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.sea_rise).range(-500.0..=500.0).suffix(" m"));
                    if ui.button("raise the sea everywhere").on_hover_text("negative lowers it").clicked() {
                        action = Some(Edit::SeaLevel { rise_m: self.sea_rise });
                    }
                });
            }
            Tool::Climate => {
                ui.horizontal_wrapped(|ui| {
                    for (k, name) in [(ClimateKind::Biome, "biome"), (ClimateKind::Temperature, "temperature"), (ClimateKind::Rainfall, "rainfall"), (ClimateKind::Clear, "clear")] {
                        ui.radio_value(&mut self.climate, k, name);
                    }
                });
                match self.climate {
                    ClimateKind::Temperature => {
                        ui.add(egui::Slider::new(&mut self.temperature, -40.0..=40.0).text("°C, yearly mean"));
                    }
                    ClimateKind::Rainfall => {
                        ui.add(egui::Slider::new(&mut self.rainfall, 0.0..=4000.0).text("mm of rain a year").integer());
                    }
                    ClimateKind::Biome => {
                        egui::ComboBox::from_label("biome").selected_text(self.biome.name()).show_ui(ui, |ui| {
                            for b in ALL.into_iter().filter(|&b| b != Biome::Ocean) {
                                ui.selectable_value(&mut self.biome, b, b.name());
                            }
                        });
                    }
                    ClimateKind::Clear => {
                        ui.small("Takes away painted climate and biomes.");
                    }
                }
                radius(ui, &mut self.radius_km);
                ui.small("What you paint stays, even when the land changes, until cleared.");
            }
            Tool::River => {
                ui.add(egui::Slider::new(&mut self.spring, 0.0..=10.0).text("spring, in big rivers' worth"));
                ui.small("Drag from the source to the mouth. Starting on a river redirects it; with no spring, only the water already there flows.");
            }
            Tool::Provinces => {
                ui.radio_value(&mut self.provinces, ProvinceKind::MoveInto, "move land into the selected province");
                ui.radio_value(&mut self.provinces, ProvinceKind::Split, "make the land a new province");
                ui.radio_value(&mut self.provinces, ProvinceKind::Merge, "merge a clicked province into the selected one");
                if self.provinces != ProvinceKind::Merge {
                    radius(ui, &mut self.radius_km);
                }
                match selected {
                    Some(p) => ui.small(format!("Selected: province {p}. People and food go with the land.")),
                    None => ui.small("Select a province with the inspect tool first."),
                };
            }
        }
        action
    }
}
