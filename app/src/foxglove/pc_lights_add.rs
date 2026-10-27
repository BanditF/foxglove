//! "Add a device…" on the Lights page (#423 follow-up): bespoke lighting gear the catalog
//! doesn't know, added by hand.
//!
//! 1. **Pick:** the HID devices nothing recognises (name, VID:PID, usage pages).
//! 2. **Set up:** a name, and a known protocol to try or "unknown".
//! 3. **Zones** (a known protocol only): "Light zone N" blinks that zone and puts it back; the
//!    user says what lit up. Nothing is written without that click.
//! 4. **Review:** the catalog entry as TOML; Save puts it in the user's
//!    `lighting-catalog.local.toml`, where "Copy entry" can share it later.

use std::collections::{BTreeMap, BTreeSet};

use egui::RichText;
use foxglove_devices::pc_lighting::{Catalog, CatalogDevice, Protocol, Unknown, ZoneSpec};

use super::pc_lights::Snapshot;
use super::pc_lights_ui::{chips, Intent};
use super::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Closed,
    Pick,
    Setup,
    Zones,
    Review,
}

/// The wizard's state, kept by the page between frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub step: Step,
    pub device: Option<Unknown>,
    pub name: String,
    pub protocol: Protocol,
    /// Index into the protocol's zone numbers.
    pub zone: usize,
    /// Zone number to what the user said lit up.
    pub answers: BTreeMap<u8, String>,
    /// Zones the user has lit.
    pub tested: BTreeSet<u8>,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            step: Step::Closed,
            device: None,
            name: String::new(),
            protocol: Protocol::None,
            zone: 0,
            answers: BTreeMap::new(),
            tested: BTreeSet::new(),
        }
    }
}

impl Wizard {
    /// Starts on a device the user picked.
    pub fn pick(&mut self, device: Unknown) {
        *self = Wizard {
            step: Step::Setup,
            name: device.name.clone(),
            device: Some(device),
            ..Wizard::default()
        };
    }

    /// The named zones, in zone order; "nothing" or an empty answer leaves a zone out.
    pub fn zones(&self) -> Vec<ZoneSpec> {
        let valid = self.protocol.zones();
        self.answers
            .iter()
            .filter(|(id, _)| valid.contains(id))
            .map(|(id, name)| (*id, name.trim()))
            .filter(|(_, name)| !name.is_empty() && !name.eq_ignore_ascii_case("nothing"))
            .map(|(id, name)| ZoneSpec {
                id,
                name: name.to_string(),
                position: None,
                role: Default::default(),
            })
            .collect()
    }

    /// The entry Save would write, or why it can't yet.
    pub fn entry(&self) -> Result<CatalogDevice, &'static str> {
        let device = self.device.as_ref().ok_or("Pick a device first")?;
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Give it a name");
        }
        let zones = self.zones();
        if self.protocol != Protocol::None && zones.is_empty() {
            return Err("Name at least one zone, or choose Unknown");
        }
        Ok(CatalogDevice::new(
            name.to_string(),
            device.vid,
            device.pid,
            self.protocol,
            zones,
        ))
    }
}

/// The first entry of a catalog TOML (a row's "Copy entry" text).
pub fn parse_entry(toml: &str) -> Option<CatalogDevice> {
    Catalog::parse(toml).ok()?.devices.into_iter().next()
}

/// A device's usage pages in words: "vendor FF00 · 0001".
pub fn pages_text(pages: &[u16]) -> String {
    pages
        .iter()
        .map(|p| {
            if *p >= 0xFF00 {
                format!("vendor {p:04X}")
            } else {
                format!("{p:04X}")
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Paints the wizard (or its button) at the foot of the card.
pub fn draw(ui: &mut egui::Ui, s: &Snapshot, w: &mut Wizard, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    match w.step {
        Step::Closed => {
            ui.horizontal(|ui| {
                if ui.button("Add a device…").clicked() {
                    *w = Wizard {
                        step: Step::Pick,
                        ..Wizard::default()
                    };
                }
                ui.weak("For lighting gear Foxglove doesn't recognise yet.");
            });
        }
        Step::Pick => {
            ui.label(
                RichText::new("Add a device: which one?")
                    .color(p.text_strong)
                    .strong(),
            );
            if s.unknown.is_empty() {
                ui.weak("Every HID device on this PC is recognised already.");
            }
            egui::Grid::new("pc-lights-unknown")
                .num_columns(4)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for u in &s.unknown {
                        ui.label(&u.name);
                        ui.label(
                            RichText::new(format!("{:04X}:{:04X}", u.vid, u.pid))
                                .font(theme::mono(12.0)),
                        );
                        ui.label(RichText::new(pages_text(&u.usage_pages)).color(p.muted));
                        if ui.small_button("Choose").clicked() {
                            w.pick(u.clone());
                        }
                        ui.end_row();
                    }
                });
            if ui.button("Cancel").clicked() {
                *w = Wizard::default();
            }
        }
        Step::Setup => setup(ui, w),
        Step::Zones => zones(ui, s, w, out),
        Step::Review => review(ui, s, w, out),
    }
}

fn setup(ui: &mut egui::Ui, w: &mut Wizard) {
    let p = theme::palette(ui);
    let Some(device) = w.device.clone() else {
        *w = Wizard::default();
        return;
    };
    ui.label(
        RichText::new(format!(
            "Add a device: {:04X}:{:04X}",
            device.vid, device.pid
        ))
        .color(p.text_strong)
        .strong(),
    );
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut w.name);
    });
    ui.horizontal(|ui| {
        ui.label("Protocol");
        egui::ComboBox::from_id_salt("pc-lights-protocol")
            .selected_text(w.protocol.label())
            .show_ui(ui, |ui| {
                for proto in Protocol::ALL {
                    ui.selectable_value(&mut w.protocol, proto, proto.label());
                }
            });
    });
    if w.protocol != Protocol::None {
        ui.label(
            RichText::new(
                "A zone test sends this protocol's reports to the device you picked. Only try a \
                 protocol your board speaks.",
            )
            .color(p.warn),
        );
    } else {
        ui.weak("Foxglove will list it and remember its name, but can't light it.");
    }
    ui.horizontal(|ui| {
        if ui.button("Back").clicked() {
            w.step = Step::Pick;
        }
        let next = if w.protocol == Protocol::None {
            "Review"
        } else {
            "Name the zones"
        };
        if ui.add(theme::primary_button(ui, next)).clicked() {
            w.zone = 0;
            w.step = if w.protocol == Protocol::None {
                Step::Review
            } else {
                Step::Zones
            };
        }
    });
}

fn zones(ui: &mut egui::Ui, s: &Snapshot, w: &mut Wizard, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let ids = w.protocol.zones();
    let (Some(device), Some(&id)) = (w.device.clone(), ids.get(w.zone)) else {
        w.step = Step::Review;
        return;
    };
    ui.label(
        RichText::new(format!(
            "Name the zones: zone {id} ({} of {})",
            w.zone + 1,
            ids.len()
        ))
        .color(p.text_strong)
        .strong(),
    );
    ui.weak(
        "Light the zone, look at the device, then say what lit up. It blinks for a moment and \
         goes back to how it was (dark if Foxglove never set it). Leave it empty if nothing lit.",
    );
    ui.horizontal(|ui| {
        let lit = w.tested.contains(&id);
        let label = if lit {
            format!("Light zone {id} again")
        } else {
            format!("Light zone {id}")
        };
        if ui
            .add_enabled(!s.busy, theme::primary_button(ui, &label))
            .clicked()
        {
            w.tested.insert(id);
            out.push(Intent::TestZone {
                vid: device.vid,
                pid: device.pid,
                path: device.path.clone(),
                protocol: w.protocol,
                zone: id,
            });
        }
        ui.label("What lit up?");
        let answer = w.answers.entry(id).or_default();
        ui.add(egui::TextEdit::singleline(answer).hint_text("e.g. Logo, or leave empty"));
    });
    let named: Vec<String> = w
        .zones()
        .iter()
        .map(|z| format!("{} · {}", z.id, z.name))
        .collect();
    if !named.is_empty() {
        chips(ui, &named);
    }
    ui.horizontal(|ui| {
        if ui.button("Previous").clicked() {
            if w.zone == 0 {
                w.step = Step::Setup;
            } else {
                w.zone -= 1;
            }
        }
        if w.zone + 1 < ids.len() && ui.button("Next zone").clicked() {
            w.zone += 1;
        }
        if ui.add(theme::primary_button(ui, "Done naming")).clicked() {
            w.step = Step::Review;
        }
        if ui.button("Cancel").clicked() {
            *w = Wizard::default();
        }
    });
}

fn review(ui: &mut egui::Ui, s: &Snapshot, w: &mut Wizard, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    ui.label(
        RichText::new("Add a device: review")
            .color(p.text_strong)
            .strong(),
    );
    let entry = w.entry();
    match &entry {
        Ok(device) => {
            egui::Frame::new()
                .fill(p.well)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.label(RichText::new(device.to_toml()).font(theme::mono(12.0)));
                });
            if let Some(file) = &s.local_file {
                ui.weak(format!("Saved to {file}. Copy entry shares it later."));
            }
        }
        Err(why) => {
            ui.label(RichText::new(*why).color(p.warn));
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Back").clicked() {
            w.step = if w.protocol == Protocol::None {
                Step::Setup
            } else {
                Step::Zones
            };
        }
        if ui
            .add_enabled(entry.is_ok(), theme::primary_button(ui, "Save"))
            .clicked()
        {
            if let Ok(device) = entry.clone() {
                out.push(Intent::Save(Box::new(device)));
                *w = Wizard::default();
            }
        }
        if ui.button("Cancel").clicked() {
            *w = Wizard::default();
        }
    });
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::foxglove::pc_lights_ui::tests::unknown_strip;

    /// The previews' wizard: naming zone 3 of the desk strip, two zones named.
    pub fn sample_zones() -> Wizard {
        let mut w = Wizard::default();
        w.pick(unknown_strip());
        w.protocol = Protocol::OmenTracer;
        w.step = Step::Zones;
        w.zone = 2;
        w.answers.insert(1, "Shelf left".into());
        w.answers.insert(2, "nothing".into());
        w.answers.insert(3, "Shelf right".into());
        w.tested.extend([1, 2, 3]);
        w
    }

    #[test]
    fn the_entry_takes_named_zones_only() {
        let mut w = sample_zones();
        let e = w.entry().unwrap();
        assert_eq!(e.vid, 0x1209);
        assert_eq!(
            e.zones,
            vec![
                ZoneSpec {
                    id: 1,
                    name: "Shelf left".into(),
                    position: None,
                    role: Default::default(),
                },
                ZoneSpec {
                    id: 3,
                    name: "Shelf right".into(),
                    position: None,
                    role: Default::default(),
                },
            ]
        );
        assert!(e.local);
        w.answers.clear();
        assert_eq!(
            w.entry().unwrap_err(),
            "Name at least one zone, or choose Unknown"
        );
        w.protocol = Protocol::None;
        let e = w.entry().unwrap();
        assert!(e.zones.is_empty(), "an unknown protocol has no zones");
        w.name = "  ".into();
        assert_eq!(w.entry().unwrap_err(), "Give it a name");
        assert_eq!(pages_text(&[0xff00, 1]), "vendor FF00 · 0001");
        let toml = sample_zones().entry().unwrap().to_toml();
        assert_eq!(parse_entry(&toml).unwrap().zones.len(), 2);
    }

    #[test]
    fn nothing_is_written_until_light_zone_is_clicked() {
        // Drawing every step with no clicks asks for no zone test.
        let s = crate::foxglove::pc_lights_ui::tests::sample().pc;
        for step in [Step::Pick, Step::Setup, Step::Zones, Step::Review] {
            let mut w = sample_zones();
            w.step = step;
            let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
            let mut out = Vec::new();
            for _ in 0..2 {
                let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                    draw(ui, &s, &mut w, &mut out);
                });
                frame.textures_delta.clear();
            }
            assert!(out.is_empty(), "{step:?}: {out:?}");
        }
    }
}
