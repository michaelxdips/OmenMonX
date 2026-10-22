//! System: the BIOS settings table.
//!
//! A table, not a grid of cards: these are 60-odd name/value rows and the job is
//! scanning and comparing, which a table does and cards do not.

use eframe::egui;

use crate::sampler::{BiosRow, ControlRequest, SharedState};
use crate::theme::{self, elevation, radius, space, Palette, BODY, LABEL};
use crate::ui::components as c;
use crate::widgets as w;
use crate::App;

/// Name column width. Fixed so the value column starts at the same x on every
/// row, which is what makes a table scannable.
const NAME_COL_W: f32 = 300.0;

impl App {
    pub fn page_system(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        if st.bios_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                c::body_text(ui, p, "reading BIOS settings\u{2026}");
            });
            return;
        }
        let Some(rows) = st.bios_settings.as_ref() else {
            w::card(ui, p, "BIOS SETTINGS", |ui| {
                w::empty(
                    ui,
                    p,
                    &st.bios_error.clone().unwrap_or_else(|| "unavailable".into()),
                    Some("requires elevation: the InstrumentedBIOS namespace is admin-only"),
                );
                if w::quiet_button(ui, p, "Retry", true).clicked() {
                    self.load_bios_async();
                }
            });
            return;
        };

        ui.horizontal(|ui| {
            c::body_text(ui, p, &format!("{} settings enumerated", rows.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::quiet_button(ui, p, "Reload", true).clicked() {
                    self.load_bios_async();
                }
            });
        });
        ui.add_space(space::SM);

        table_header(ui, p);

        let max_h = (ui.available_height() - 96.0).max(160.0);
        let rows = rows.clone();
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, row) in rows.iter().enumerate() {
                    let selected = self.bios_selected == Some(i);
                    let fill = if selected {
                        p.tint(p.accent, 40)
                    } else if i % 2 == 0 {
                        p.panel
                    } else {
                        p.tint(p.text, 6)
                    };
                    let resp = egui::Frame::default()
                        .fill(fill)
                        .inner_margin(egui::Margin::symmetric(space::MD, 7.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.allocate_ui(egui::vec2(NAME_COL_W, 18.0), |ui| {
                                    ui.label(
                                        egui::RichText::new(&row.name)
                                            .size(BODY)
                                            .color(if row.read_only { p.dim } else { p.text })
                                            .family(theme::proportional()),
                                    );
                                    if row.read_only {
                                        w::badge_pill(ui, p, "RO", p.faint());
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(&row.current)
                                        .size(BODY)
                                        .color(p.accent)
                                        .family(theme::numeric()),
                                );
                                if let Some(msg) = &row.last_write {
                                    ui.label(
                                        egui::RichText::new(msg)
                                            .size(LABEL)
                                            .color(if msg.contains("accepted") {
                                                p.green
                                            } else {
                                                p.red
                                            })
                                            .family(theme::proportional()),
                                    );
                                }
                            });
                        })
                        .response;
                    if resp.interact(egui::Sense::click()).clicked() && !row.read_only {
                        self.bios_selected = Some(i);
                        self.bios_new_value = row.current.clone();
                    }
                }
            });

        ui.add_space(space::SM);
        let Some(idx) = self.bios_selected else {
            c::caption(ui, p, "select a writable setting to edit it");
            return;
        };
        let Some(row) = rows.get(idx).cloned() else {
            self.bios_selected = None;
            return;
        };
        let row: BiosRow = row;
        egui::Frame::default()
            .fill(p.panel)
            .stroke(egui::Stroke::new(theme::HAIRLINE, p.accent))
            .rounding(radius::CARD)
            .inner_margin(egui::Margin::symmetric(space::LG, space::SM))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&row.name)
                            .size(BODY)
                            .color(p.text)
                            .family(crate::theme::proportional()),
                    );
                    ui.add_space(space::SM);
                    if row.possible.is_empty() {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.bios_new_value)
                                .desired_width(240.0)
                                .font(egui::TextStyle::Monospace),
                        );
                    } else {
                        egui::ComboBox::from_id_source("bios_val")
                            .selected_text(self.bios_new_value.clone())
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                for v in &row.possible {
                                    ui.selectable_value(&mut self.bios_new_value, v.clone(), v);
                                }
                            });
                    }
                    if w::primary_button(ui, p, "Write", st.elevated).clicked() {
                        self.write(ControlRequest::SetBios {
                            name: row.name.clone(),
                            value: self.bios_new_value.clone(),
                        });
                    }
                });
                ui.add_space(space::XS);
                c::caption(
                    ui,
                    p,
                    "rc 0 accepted \u{00b7} 1 no such setting \u{00b7} 5 value rejected \u{00b7} 6 reboot required",
                );
                c::write_feedback(ui, p, st, self.pending_since);
            });
    }
}

/// The sticky-looking header row above the settings table.
fn table_header(ui: &mut egui::Ui, p: &Palette) {
    elevation::flat(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(NAME_COL_W, 18.0), |ui| {
                ui.label(
                    egui::RichText::new("SETTING")
                        .size(LABEL)
                        .color(p.dim)
                        .family(theme::proportional()),
                );
            });
            ui.label(
                egui::RichText::new("CURRENT VALUE")
                    .size(LABEL)
                    .color(p.dim)
                    .family(theme::proportional()),
            );
        });
    });
    ui.add_space(space::XS);
}
