//! Control: power ceiling, thermal profile, fan control.

use eframe::egui;

use crate::sampler::{ControlRequest, SharedState};
use crate::theme::{self, space, Palette, HERO, LABEL};
use crate::ui::components as c;
use crate::widgets as w;
use crate::App;

impl App {
    pub fn page_performance(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                self.power_card(ui, p, st);
            }
            c::Column::Right => {
                self.thermal_card(ui, p, st);
                ui.add_space(space::MD);
                self.fan_control_card(ui, p, st);
            }
        });
    }

    fn power_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        if st.limits.is_none() {
            w::card(ui, p, "POWER CEILING", |ui| {
                w::empty(
                    ui,
                    p,
                    "power limits need an elevated token",
                    Some(if st.elevated {
                        "the 0x2A block did not answer on this boot"
                    } else {
                        "0x29 and 0x2A are admin-only; the rest of the app works without them"
                    }),
                );
                if !st.elevated {
                    ui.add_space(space::SM);
                    if w::primary_button(ui, p, "Restart as administrator", true).clicked() {
                        self.relaunch_elevated(ui.ctx());
                    }
                }
            });
            return;
        }

        let cap_now = st.limits.map(|l| l.pl1);
        let ceiling = st
            .limits
            .map(|l| l.pl1_ceiling())
            .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL1);
        w::card(ui, p, "POWER CEILING", |ui| {
            ui.horizontal(|ui| {
                let big = match cap_now {
                    Some(v) => format!("{v}"),
                    None => "--".into(),
                };
                ui.label(
                    egui::RichText::new(big)
                        .size(HERO)
                        .strong()
                        .color(match cap_now {
                            Some(v) if v < ceiling => p.amber,
                            Some(_) => p.text,
                            None => p.faint(),
                        })
                        .family(theme::numeric()),
                );
                ui.vertical(|ui| {
                    ui.add_space(space::SM);
                    ui.label(
                        egui::RichText::new("WATTS")
                            .size(LABEL)
                            .color(p.dim)
                            .family(theme::proportional()),
                    );
                    if let Some(l) = st.limits {
                        ui.label(
                            egui::RichText::new(format!(
                                "PL2 {} - PL4 {} - GPU {}",
                                l.pl2, l.pl4, l.pl_with_gpu
                            ))
                            .size(LABEL)
                            .color(p.faint())
                            .family(theme::numeric()),
                        );
                    }
                });
            });
            ui.add_space(space::SM);

            let mut cap = cap_now.unwrap_or(ceiling);
            ui.horizontal(|ui| {
                let resp = ui.add_enabled(
                    st.elevated && st.limits.is_some(),
                    egui::Slider::new(&mut cap, hp_wmi::CpuPowerLimits::MIN_WATTS..=ceiling)
                        .suffix(" W")
                        .trailing_fill(true),
                );
                if resp.drag_stopped() || (resp.lost_focus() && resp.changed()) {
                    self.write(ControlRequest::SetPowerCap(cap));
                }
            });
            c::caption(
                ui,
                p,
                &format!(
                    "range {}-{} W on this supply - PL4 ceiling {} W",
                    hp_wmi::CpuPowerLimits::MIN_WATTS,
                    ceiling,
                    st.limits
                        .map(|l| l.pl4_ceiling())
                        .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL4)
                ),
            );
            ui.add_space(space::SM);
            ui.horizontal(|ui| {
                if w::quiet_button(ui, p, "Restore stock", st.elevated).clicked() {
                    self.write(ControlRequest::RestoreDefault);
                }
                c::caption(ui, p, "firmware re-applies its own block");
            });
            ui.add_space(space::SM);
            c::body_text(
                ui,
                p,
                "This cap stays until you change it or press Restore stock - saved and re-applied at every login.",
            );
        });
    }

    fn thermal_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let status = st.thermal_status;
        let active = if status == hp_wmi::ThermalMode::Performance.expected_status() {
            1
        } else if status == hp_wmi::ThermalMode::Balanced.expected_status()
            || status == hp_wmi::ThermalMode::Cool.expected_status()
        {
            0
        } else {
            usize::MAX
        };
        w::card(ui, p, "THERMAL PROFILE", |ui| {
            let opts = [
                ("Balanced", active == 0),
                ("Performance", active == 1),
                ("Cool", false),
            ];
            if let Some(i) = w::segmented(ui, p, &opts, st.elevated) {
                let mode = match i {
                    1 => hp_wmi::ThermalMode::Performance,
                    2 => hp_wmi::ThermalMode::Cool,
                    _ => hp_wmi::ThermalMode::Balanced,
                };
                self.write(ControlRequest::SetThermal(mode));
            }
            ui.add_space(space::SM);
            ui.label(
                egui::RichText::new(format!("firmware status {status:#04X}"))
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::numeric()),
            );
        });
    }

    fn fan_control_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let lvl = st.sample.as_ref().and_then(|s| s.fan_level_pct);
        w::card(ui, p, "FAN CONTROL", |ui| {
            w::stat(
                ui,
                p,
                "level",
                &lvl.map(|v| format!("{v} %")).unwrap_or_else(|| "--".into()),
                p.text,
            );
            ui.add_space(space::SM);
            ui.horizontal(|ui| {
                if w::primary_button(ui, p, "Max", st.elevated).clicked() {
                    self.write(ControlRequest::SetMaxFan(true));
                }
                if w::quiet_button(ui, p, "Hand back to firmware", st.elevated).clicked() {
                    self.write(ControlRequest::ReleaseMaxFan);
                }
            });
            ui.add_space(space::SM);
            c::caption(
                ui,
                p,
                "Max holds full speed until you hand back; Max off alone does not release",
            );
        });
    }
}
