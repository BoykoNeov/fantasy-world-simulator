//! The settings panel: every setting of a configuration, group by group, each with its value, its
//! range and unit, and its explanation in words. New world uses it for every setting; the running
//! world's panel for those that may change while it runs (see [`sim::settings`]).

use eframe::egui;
use sim::config::{Config, ModuleConfig};
use sim::module::Slot;
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

/// Pick `version` for `slot` (as [`sim::config::CHOICES`] names them), switching on what it needs (realms need
/// people; rulers, diplomacy and armies need realms; realms choosing war and peace need armies)
/// and off what needs what is switched off. A version switched on gets its default settings, or
/// keeps those it had.
pub fn choose(m: &mut ModuleConfig, slot: Slot, version: &str) {
    m.set_choice(slot, version).expect("a version offered for the slot");
    let on = version != "off";
    match slot {
        Slot::Population | Slot::Realms if !on => {
            m.without_realms();
        }
        Slot::Characters | Slot::Diplomacy | Slot::Warfare if on => {
            m.set_choice(Slot::Realms, "basic").expect("a version");
            m.set_choice(Slot::Population, "basic").expect("a version");
        }
        _ => {}
    }
    if slot == Slot::Realms && on {
        m.set_choice(Slot::Population, "basic").expect("a version");
    }
    if slot == Slot::Diplomacy && version == "treaties" {
        m.set_choice(Slot::Warfare, "troops").expect("a version");
    }
    if slot == Slot::Warfare && !on && m.choice(Slot::Diplomacy) == Some("treaties") {
        m.set_choice(Slot::Diplomacy, "wars").expect("a version");
    }
}

fn number(x: f64, whole: bool) -> String {
    if whole { format!("{x:.0}") } else { format!("{x}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::combos::Mix;

    #[test]
    fn choosing_a_version_switches_on_what_it_needs_and_off_what_needs_it() {
        let mut m = ModuleConfig::default();
        choose(&mut m, Slot::Population, "off");
        assert!(m.realms.is_none() && m.characters.is_none() && m.diplomacy.is_none() && m.warfare.is_none());
        choose(&mut m, Slot::Diplomacy, "treaties");
        assert!(m.population.is_some() && m.realms.is_some() && m.warfare.is_some());
        choose(&mut m, Slot::Warfare, "off");
        assert_eq!(m.choice(Slot::Diplomacy), Some("wars"));
        choose(&mut m, Slot::Realms, "off");
        assert!(m.diplomacy.is_none());
        // Whatever is picked, the mix is one the engine takes (with an economy for the people).
        for &(slot, versions) in sim::config::CHOICES {
            for &v in versions {
                let mut m = ModuleConfig::default();
                choose(&mut m, slot, v);
                let refused = Mix::of(&m).check().err();
                assert!(refused.is_none() || slot == Slot::Economics, "{slot:?}={v}: {refused:?}");
            }
        }
    }
}
