//! The editor's controls on the inspector's pages: changing a realm (its treasury, opinions, war
//! and peace, merging and dissolving it), its ruler and heir, and the armies in a province. Each
//! control gives back the change asked for ([`Action`]); the window sends it.

use crate::editor::Action;
use crate::inspector::{group, Names};
use eframe::egui;
use sim::characters::NEVER as NOT_YET;
use sim::edit::Edit;
use sim::events::Event;
use sim::grid::NONE;
use sim::world::World;

/// What the world keeps, so only changes it can take are offered.
#[derive(Clone, Copy, Debug, Default)]
pub struct Kept {
    /// The column of the per-good fields that holds coins, where realms keep treasuries.
    pub treasury: Option<usize>,
    pub opinions: bool,
    pub wars: bool,
    pub rulers: bool,
    pub armies: bool,
}

/// What is being typed into the controls, kept between frames.
#[derive(Clone, Debug)]
pub struct RealmTools {
    /// The realm the realm controls were filled for: another realm fills them again.
    realm: u32,
    treasury: u64,
    opinion_of: u32,
    opinion: (f32, f32),
    war_on: u32,
    merge_into: u32,
    /// Traits typed for the ruler and the heir (warlike, greed, ability), and who they are for.
    ruler_traits: (u32, [u8; 3]),
    heir_traits: (u32, [u8; 3]),
    new_ruler: (f64, [u8; 3]),
    /// Soldiers to place: realm, kind (its place in the kinds), size.
    place: (u32, usize, u64),
    /// A company being changed: its number, size and kind (its place in the kinds).
    company: Option<(u32, u64, usize)>,
}

impl Default for RealmTools {
    fn default() -> Self {
        RealmTools {
            realm: NONE,
            treasury: 0,
            opinion_of: NONE,
            opinion: (0.0, 0.0),
            war_on: NONE,
            merge_into: NONE,
            ruler_traits: (NONE, [50; 3]),
            heir_traits: (NONE, [50; 3]),
            new_ruler: (30.0, [50; 3]),
            place: (NONE, 0, 1000),
            company: None,
        }
    }
}

fn traits_of(w: &World, c: u32) -> [u8; 3] {
    let ch = &w.characters;
    let k = c as usize;
    [ch.martial.get(k), ch.greed.get(k), ch.ability.get(k)].map(|t| t.copied().unwrap_or(50))
}

fn trait_sliders(ui: &mut egui::Ui, t: &mut [u8; 3]) {
    ui.add(egui::Slider::new(&mut t[0], 0..=100).text("cautious to warlike"));
    ui.add(egui::Slider::new(&mut t[1], 0..=100).text("open-handed to greedy"));
    ui.add(egui::Slider::new(&mut t[2], 0..=100).text("inept to able"));
}

/// A list to pick a realm from: the living realms but `not`.
fn pick_realm(ui: &mut egui::Ui, id: &str, w: &World, names: &Names, not: u32, to: &mut u32) {
    if *to != NONE && !w.realms.alive(*to) {
        *to = NONE;
    }
    let text = if *to == NONE { "pick a realm".to_string() } else { names.realm(*to) };
    egui::ComboBox::from_id_salt(id).selected_text(text).show_ui(ui, |ui| {
        for r in (0..w.realms.count() as u32).filter(|&r| r != not && w.realms.alive(r)) {
            ui.selectable_value(to, r, names.realm(r));
        }
    });
}

impl RealmTools {
    /// The controls for realm `r`'s page, shown open if `open`.
    pub fn realm(&mut self, ui: &mut egui::Ui, w: &World, names: &Names, r: u32, kept: &Kept, open: bool) -> Option<Action> {
        if !w.realms.alive(r) {
            return None;
        }
        let ru = r as usize;
        if self.realm != r {
            let ruler = w.realms.ruler.get(ru).copied().unwrap_or(NONE);
            let heir = w.realms.heir.get(ru).copied().unwrap_or(NONE);
            *self = RealmTools {
                realm: r,
                treasury: kept.treasury.and_then(|c| w.realm_stock.get(ru * w.goods.stride() + c)).copied().unwrap_or(0),
                ruler_traits: (ruler, traits_of(w, ruler)),
                heir_traits: (heir, traits_of(w, heir)),
                place: self.place,
                ..RealmTools::default()
            };
        }
        let land: Vec<u32> = (0..w.provinces.count as u32).filter(|&p| w.owner[p as usize] == r && w.provinces.cells[p as usize] > 0).collect();
        let mut action = None;
        egui::CollapsingHeader::new(format!("Change {}", names.realm(r))).id_salt(("change realm", r)).default_open(open).show(ui, |ui| {
            if let Some(c) = kept.treasury {
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::DragValue::new(&mut self.treasury).range(0..=1_000_000_000_000u64).speed(1000.0).suffix(" coins"));
                    if ui.small_button("set the treasury").clicked() {
                        let good = w.goods.tree.name(w.goods.columns[c]).to_string();
                        action = Some(Action::Edit(Edit::SetRealmStock { realm: r, good, amount: self.treasury }));
                    }
                });
            }
            if kept.opinions {
                ui.horizontal_wrapped(|ui| {
                    ui.label("opinions with");
                    let before = self.opinion_of;
                    pick_realm(ui, "opinion of", w, names, r, &mut self.opinion_of);
                    let o = self.opinion_of;
                    if o != before && o != NONE {
                        self.opinion = (w.relations.opinion(r, o), w.relations.opinion(o, r));
                    }
                    if o != NONE {
                        ui.add(egui::Slider::new(&mut self.opinion.0, -100.0..=100.0).text(format!("what {} thinks of {}", names.realm(r), names.realm(o))).integer());
                        ui.add(egui::Slider::new(&mut self.opinion.1, -100.0..=100.0).text(format!("what {} thinks of {}", names.realm(o), names.realm(r))).integer());
                        if ui.small_button("set what they think").clicked() {
                            action = Some(Action::Edit(Edit::SetOpinion { a: r, b: o, a_of_b: self.opinion.0, b_of_a: self.opinion.1 }));
                        }
                    }
                });
            }
            if kept.wars {
                ui.horizontal_wrapped(|ui| {
                    ui.label("war on");
                    pick_realm(ui, "war on", w, names, r, &mut self.war_on);
                    if self.war_on != NONE && ui.small_button("declare war on it").on_hover_text("a poke: the war begins tomorrow, breaking any truce").clicked() {
                        action = Some(Action::Poke(Event::DeclareWar { attacker: r, defender: self.war_on }));
                    }
                });
                let wars = &w.wars;
                for k in 0..wars.count() as u32 {
                    let (a, b) = (wars.attacker[k as usize], wars.defender[k as usize]);
                    if !wars.ongoing(k) || (a != r && b != r) || !w.realms.alive(a) || !w.realms.alive(b) {
                        continue;
                    }
                    let other = if a == r { b } else { a };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("make peace with {}:", names.realm(other)));
                        for (winner, text) in [(r, format!("{} wins", names.realm(r))), (other, format!("{} wins", names.realm(other))), (NONE, "nobody wins".to_string())] {
                            if ui.small_button(text).on_hover_text("a poke: the peace is made tomorrow; the winner keeps the land it holds, or with nobody winning all goes back").clicked() {
                                action = Some(Action::Poke(Event::ForcePeace { a: r, b: other, winner }));
                            }
                        }
                    });
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("merge into");
                pick_realm(ui, "merge into", w, names, r, &mut self.merge_into);
                if self.merge_into != NONE && ui.small_button("merge into it").on_hover_text("an edit: all its land goes to that realm, and it is no more").clicked() {
                    action = Some(Action::Edit(Edit::SetOwner { provinces: land.clone(), realm: self.merge_into }));
                }
                if ui.small_button("dissolve it").on_hover_text("an edit: all its land goes to nobody").clicked() {
                    action = Some(Action::Edit(Edit::SetOwner { provinces: land.clone(), realm: NONE }));
                }
            });
            if kept.rulers {
                ui.separator();
                for (title, who, traits) in [("ruler", w.realms.ruler.get(ru).copied().unwrap_or(NONE), &mut self.ruler_traits), ("heir", w.realms.heir.get(ru).copied().unwrap_or(NONE), &mut self.heir_traits)] {
                    if who == NONE || w.characters.died.get(who as usize).is_none_or(|&d| d != NOT_YET) {
                        continue;
                    }
                    if traits.0 != who {
                        *traits = (who, traits_of(w, who));
                    }
                    ui.label(format!("The {title}, {}:", names.person(who)));
                    trait_sliders(ui, &mut traits.1);
                    ui.horizontal(|ui| {
                        if ui.small_button("set the traits").clicked() {
                            let [martial, greed, ability] = traits.1;
                            action = Some(Action::Edit(Edit::SetTraits { character: who, martial, greed, ability }));
                        }
                        let hint = if title == "ruler" { "a poke: dies tomorrow, and the heir (or a claimant) takes the throne at once" } else { "a poke: dies tomorrow, and the realm has no heir" };
                        if ui.small_button(format!("kill the {title}")).on_hover_text(hint).clicked() {
                            action = Some(Action::Poke(Event::KillRuler { character: who }));
                        }
                    });
                }
                ui.label("A new ruler, put on the throne now (the one before is set aside; the heir stays):");
                ui.add(egui::Slider::new(&mut self.new_ruler.0, 0.0..=90.0).text("years old").integer());
                trait_sliders(ui, &mut self.new_ruler.1);
                if ui.small_button("crown a new ruler").clicked() {
                    let (age, [martial, greed, ability]) = self.new_ruler;
                    action = Some(Action::Edit(Edit::NewRuler { realm: r, age, martial, greed, ability }));
                }
            }
        });
        action
    }

    /// The armies in province `p`: the companies standing there and passing through, with controls
    /// to change them, to place soldiers and to send the field companies elsewhere. `kinds`: the
    /// troop kinds (node, name, ship). Gives back the change asked for, or an order waiting for a
    /// click on where to go (realm, from).
    pub fn armies(&mut self, ui: &mut egui::Ui, w: &World, names: &Names, p: u32, kinds: &[(u16, String, bool)]) -> (Option<Action>, Option<(u32, u32)>) {
        let arm = &w.armies;
        let kind_name = |node: u16| kinds.iter().find(|k| k.0 == node).map_or(format!("kind {node}"), |k| k.1.clone());
        let here: Vec<usize> = (0..arm.count()).filter(|&a| arm.size[a] > 0 && (arm.province[a] == p || arm.next[a] == p)).collect();
        let (mut action, mut march) = (None, None);
        let title = format!("Armies here ({} compan{})", here.len(), if here.len() == 1 { "y" } else { "ies" });
        egui::CollapsingHeader::new(egui::RichText::new(title).strong()).id_salt(("armies", p)).default_open(!here.is_empty()).show(ui, |ui| {
            for &a in &here {
                let role = if arm.role[a] == sim::warfare::GUARD { "guards" } else { "field company" };
                let place = if arm.next[a] == p {
                    format!("coming in from province {}", arm.province[a])
                } else if arm.next[a] != NONE {
                    format!("on its way to province {}", arm.dest[a])
                } else {
                    "standing here".into()
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("company {a}: {} {}, {} ({role}) from province {}, {place}", group(arm.size[a]), kind_name(arm.kind[a]), names.realm(arm.realm[a]), arm.home[a]));
                    match &mut self.company {
                        Some((k, size, kind)) if *k == a as u32 => {
                            ui.add(egui::DragValue::new(size).range(0..=100_000_000u64).speed(10.0).suffix(" soldiers"));
                            egui::ComboBox::from_id_salt(("company kind", a)).selected_text(kinds.get(*kind).map_or("?", |k| k.1.as_str())).show_ui(ui, |ui| {
                                for (i, k) in kinds.iter().enumerate() {
                                    ui.selectable_value(kind, i, &k.1);
                                }
                            });
                            if ui.small_button("set").clicked() {
                                action = Some(Action::Edit(Edit::SetArmy { army: a as u32, size: *size, kind: kinds[*kind].1.clone() }));
                            }
                            if ui.small_button("cancel").clicked() {
                                self.company = None;
                            }
                        }
                        _ => {
                            if ui.small_button("change").on_hover_text("set its soldiers (0 disbands it: they are gone) and their kind").clicked() {
                                let kind = kinds.iter().position(|k| k.0 == arm.kind[a]).unwrap_or(0);
                                self.company = Some((a as u32, arm.size[a], kind));
                            }
                        }
                    }
                });
            }
            if action.is_some() {
                self.company = None;
            }
            // Orders: the field companies of each realm standing here.
            let mut realms: Vec<u32> = here.iter().filter(|&&a| arm.province[a] == p && arm.next[a] == NONE && arm.role[a] == sim::warfare::FIELD).map(|&a| arm.realm[a]).collect();
            realms.sort_unstable();
            realms.dedup();
            for r in realms {
                if ui.small_button(format!("send {}'s field companies here to…", names.realm(r))).on_hover_text("then click where they are to go: they march there, and the general takes them over once they arrive").clicked() {
                    march = Some((r, p));
                }
            }
            if kinds.is_empty() {
                return;
            }
            ui.separator();
            let owner = w.owner.get(p as usize).copied().unwrap_or(NONE);
            if self.place.0 == NONE || !w.realms.alive(self.place.0) {
                self.place.0 = owner;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Place");
                ui.add(egui::DragValue::new(&mut self.place.2).range(1..=100_000_000u64).speed(10.0));
                egui::ComboBox::from_id_salt("place kind").selected_text(kinds.get(self.place.1).map_or("?", |k| k.1.as_str())).show_ui(ui, |ui| {
                    for (i, k) in kinds.iter().enumerate() {
                        ui.selectable_value(&mut self.place.1, i, &k.1);
                    }
                });
                ui.label("of");
                pick_realm(ui, "place realm", w, names, NONE, &mut self.place.0);
                if self.place.0 != NONE && ui.small_button("place them here").on_hover_text("an edit: soldiers made from nothing, as a field company, on the realm's own land or an enemy's at war with it. In peace, soldiers standing at home go home at the next month's start").clicked() {
                    let (realm, kind, size) = self.place;
                    action = Some(Action::Edit(Edit::PlaceArmy { province: p, realm, kind: kinds[kind.min(kinds.len() - 1)].1.clone(), size }));
                }
            });
        });
        (action, march)
    }
}
