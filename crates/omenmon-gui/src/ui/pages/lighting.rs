//! Lighting: the keyboard colour table.

use eframe::egui;

use crate::sampler::{ControlRequest, SharedState};
use crate::theme::{self, radius, space, Palette, BODY, LABEL};
use crate::ui::components as c;
use crate::widgets as w;
use crate::App;

impl App {
    pub fn page_lighting(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let Some(rgb) = st.rgb.as_ref() else {
            w::card(ui, p, "KEYBOARD", |ui| {
                w::empty(
                    ui,
                    p,
                    "colour table unreadable",
                    Some(if st.elevated {
                        "the keyboard channel did not answer"
                    } else {
                        "run elevated to read and write the colour table"
                    }),
                );
            });
            return;
        };
        if rgb.zones == 0 {
            w::card(ui, p, "KEYBOARD", |ui| {
                w::empty(
                    ui,
                    p,
                    "firmware reports 0 zones",
                    Some("nothing to address on this board"),
                );
            });
            return;
        }

        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                w::card(
                    ui,
                    p,
                    &format!("ZONES - {} ZONES", rgb.zones),
                    |ui| {
                        while self.zone_pick.len() < rgb.zones as usize {
                            self.zone_pick.push(egui::Color32::WHITE);
                        }
                        for i in 0..rgb.zones as usize {
                            let cur = rgb.colors.get(i).copied().unwrap_or([0, 0, 0]);
                            let mut col = egui::Color32::from_rgb(cur[0], cur[1], cur[2]);
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(26.0, 26.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, radius::CONTROL, col);
                                ui.painter().rect_stroke(
                                    rect,
                                    radius::CONTROL,
                                    egui::Stroke::new(theme::HAIRLINE, p.border),
                                );
                                ui.add_space(space::SM);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("Zone {}", i + 1))
                                            .size(BODY)
                                            .color(p.text)
                                            .family(theme::proportional()),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "#{:02X}{:02X}{:02X}",
                                            cur[0], cur[1], cur[2]
                                        ))
                                        .size(LABEL)
                                        .color(p.faint())
                                        .family(theme::numeric()),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if w::quiet_button(ui, p, "Apply", st.elevated).clicked() {
                                            let picked =
                                                self.zone_pick.get(i).copied().unwrap_or(col);
                                            self.write(ControlRequest::SetZone {
                                                zone: i as u8,
                                                rgb: [picked.r(), picked.g(), picked.b()],
                                            });
                                        }
                                        if ui.color_edit_button_srgba(&mut col).changed() {
                                            self.zone_pick[i] = col;
                                        }
                                    },
                                );
                            });
                            ui.add_space(space::SM);
                        }
                    },
                );
            }
            c::Column::Right => {
                w::card(ui, p, "CHANNEL", |ui| {
                    if let Some(b) = rgb.backlight {
                        w::stat(ui, p, "backlight", &format!("{b:#04X}"), p.text);
                        ui.add_space(space::SM);
                        let is_on = b & 0x80 != 0;
                        ui.horizontal(|ui| {
                            if w::quiet_button(ui, p, "Backlight off", st.elevated && is_on)
                                .clicked()
                            {
                                self.write(ControlRequest::SetBacklight(0x64));
                            }
                            if w::quiet_button(ui, p, "Backlight on", st.elevated && !is_on)
                                .clicked()
                            {
                                self.write(ControlRequest::SetBacklight(0xE4));
                            }
                        });
                        c::caption(ui, p, "0x64 off / 0xE4 on - no intermediate levels");
                        ui.add_space(space::SM);
                        w::divider(ui, p);
                    }
                    ui.add_space(space::SM);
                    if w::quiet_button(
                        ui,
                        p,
                        "Restore original table",
                        st.elevated && rgb.pristine.is_some(),
                    )
                    .clicked()
                    {
                        self.write(ControlRequest::RestoreRgb);
                    }
                    c::caption(ui, p, "read-modify-write, verified byte-for-byte");
                });
            }
        });
    }
}
