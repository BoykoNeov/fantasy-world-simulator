//! The settings panel: every setting of a configuration, group by group, each with its value, its
//! range and unit, and its explanation in words. New world uses it for every setting; the running
//! world's panel for those that may change while it runs (see [`sim::settings`]).

use eframe::egui;
use sim::config::{Config, ModuleConfig};
use sim::diplomacy::TreatyParams;
use sim::settings::{self, Item, When, GROUPS, SECTIONS};

/// Which settings a panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// Every setting, and the parts that switch on and off: for a world about to be made.
    All,
    /// Only those that may change while the world runs.
    Live,
}

/// Draw the settings of `c`, letting each be changed, against `defaults` (the same modules with
/// their default settings), which a changed value shows and can go back to. Returns why a value
/// typed was refused, if one was.
pub fn panel(ui: &mut egui::Ui, id: &str, c: &mut Config, defaults: &Config, show: Show, open: bool) -> Option<String> {
    let items = settings::items(c);
    let base = settings::items(defaults);
    let mut refused = None;
    let mut set: Option<(String, f64)> = None;
    let mut section: Option<(&str, bool)> = None;
    for g in GROUPS {
        let mine: Vec<&Item> = items.iter().filter(|i| i.key.starts_with(g.at) && settings::group_of(&i.key).is_some_and(|x| std::ptr::eq(x, g))).filter(|i| show == Show::All || i.spec.when == When::Live).collect();
        let parts: Vec<_> = SECTIONS.iter().filter(|s| show == Show::All && s.key.starts_with(g.at) && module_on(&items, s.key)).collect();
        if mine.is_empty() && parts.is_empty() {
            continue;
        }
        let changed = mine.iter().filter(|i| base.iter().find(|b| b.key == i.key).is_none_or(|b| b.value != i.value)).count();
        let title = if changed > 0 { format!("{} ({changed} changed)", g.title) } else { g.title.to_string() };
        egui::CollapsingHeader::new(title).id_salt((id, g.at)).default_open(open).show(ui, |ui| {
            ui.small(g.help);
            for s in &parts {
                let mut on = settings::section_on(c, s.key);
                if ui.checkbox(&mut on, s.label).changed() {
                    section = Some((s.key, on));
                }
                ui.small(s.help);
            }
            for i in mine {
                let default = base.iter().find(|b| b.key == i.key).map(|b| b.value);
                if let Some(v) = row(ui, i, default) {
                    set = Some((i.key.clone(), v));
                }
            }
        });
    }
    if let Some((key, on)) = section {
        refused = settings::set_section(c, key, on).err();
    }
    if let Some((key, v)) = set {
        refused = settings::set(c, &key, v).err();
    }
    refused
}

/// Whether the module a part belongs to is on, by the settings it has (a part of a module that is
/// off isn't offered).
fn module_on(items: &[Item], part: &str) -> bool {
    let module = format!("{}.", part.rsplit_once('.').map_or(part, |p| p.0));
    items.iter().any(|i| i.key.starts_with(&module))
}

/// One setting's line, then its explanation. Returns a new value typed or reset.
fn row(ui: &mut egui::Ui, i: &Item, default: Option<f64>) -> Option<f64> {
    let s = i.spec;
    let mut v = i.value;
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(i.label());
        let speed = ((s.max - s.min) / 2000.0).min(v.abs().max(1e-3) / 50.0);
        let mut drag = egui::DragValue::new(&mut v).range(s.min..=s.max).speed(speed);
        if i.whole {
            drag = drag.fixed_decimals(0).speed(speed.max(0.05));
        } else {
            drag = drag.max_decimals(6);
        }
        if !s.unit.is_empty() {
            drag = drag.suffix(format!(" {}", s.unit));
        }
        if ui.add(drag).changed() && v != i.value {
            out = Some(if i.whole { v.round() } else { v });
        }
        if s.when == When::NewWorld {
            ui.small("(fixed once made)").on_hover_text("shapes the map or the start: it can't change while the world runs");
        }
        if let Some(d) = default.filter(|&d| d != i.value)
            && ui.small_button("↺").on_hover_text(format!("back to the default, {}", number(d, i.whole))).clicked()
        {
            out = Some(d);
        }
    });
    ui.small(egui::RichText::new(s.help).weak());
    out
}

/// The slots of step 7, not among the combination tester's choices yet, with their versions.
pub const STEP7: &[(&str, &[&str])] = &[
    ("Realms", &["off", "basic"]),
    ("Characters", &["off", "rulers"]),
    ("Diplomacy", &["off", "wars", "treaties"]),
    ("Warfare", &["off", "troops"]),
];

/// The version of a step-7 slot (by its name in [`STEP7`]) in `m`.
pub fn step7_choice(m: &ModuleConfig, slot: &str) -> &'static str {
    match slot {
        "Realms" if m.realms.is_some() => "basic",
        "Characters" if m.characters.is_some() => "rulers",
        "Diplomacy" => match &m.diplomacy {
            None => "off",
            Some(d) if d.treaties.is_some() => "treaties",
            Some(_) => "wars",
        },
        "Warfare" if m.warfare.is_some() => "troops",
        _ => "off",
    }
}

/// Pick a version of a step-7 slot, with default settings, switching on what it needs (realms;
/// armies, for treaties) and off what needs what is switched off.
pub fn set_step7(m: &mut ModuleConfig, slot: &str, version: &str) {
    let on = version != "off";
    match slot {
        "Realms" => {
            if on {
                m.realms.get_or_insert_with(Default::default);
            } else {
                (m.realms, m.characters, m.diplomacy, m.warfare) = (None, None, None, None);
            }
        }
        "Characters" => m.characters = on.then(|| m.characters.take().unwrap_or_default()),
        "Diplomacy" => {
            m.diplomacy = on.then(|| m.diplomacy.take().unwrap_or_default());
            if let Some(d) = &mut m.diplomacy {
                d.treaties = (version == "treaties").then(|| d.treaties.take().unwrap_or_else(TreatyParams::default));
                if d.treaties.is_some() {
                    m.warfare.get_or_insert_with(Default::default);
                }
            }
        }
        "Warfare" => {
            m.warfare = on.then(|| m.warfare.take().unwrap_or_default());
            if let Some(d) = m.diplomacy.as_mut().filter(|_| !on) {
                d.treaties = None;
            }
        }
        _ => unreachable!("a slot of STEP7"),
    }
    if on && slot != "Realms" {
        m.realms.get_or_insert_with(Default::default);
    }
}

fn number(x: f64, whole: bool) -> String {
    if whole { format!("{x:.0}") } else { format!("{x}") }
}
