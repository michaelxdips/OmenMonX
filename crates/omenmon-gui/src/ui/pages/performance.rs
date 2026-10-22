//! Performance: the two things a user actually changes, and the measured limits
//! that make the numbers credible.

use eframe::egui;

use crate::sampler::{ControlRequest, SharedState};
use crate::theme::{self, space, Palette, BODY, DISPLAY, LABEL};
use crate::ui::components as c;
use crate::widgets as w;
use crate::{display, App};

impl App {
    pub fn page_performance(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                self.power_card(ui, p, st);
                ui.add_space(space::MD);
                self.display_card(ui, p);
            }
            c::Column::Right => {
                self.thermal_card(ui, p, st);
                ui.add_space(space::MD);
                measured_limits_card(ui, p);
            }
        });
    }

    fn power_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        // Without elevation there is no 0x2A block, so there is nothing honest to
        // show. Say why and offer the fix, rather than drawing a card of dashes.
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
                        .size(DISPLAY)
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
                                "PL2 {} \u{00b7} PL4 {} \u{00b7} GPU {}",
                                l.pl2, l.pl4, l.pl_with_gpu
                            ))
                            .size(LABEL)
                            .color(p.faint())
                            .family(theme::numeric()),
                        );
                    }
                });
            });
            ui.add_space(space::XS);
            c::caption(ui, p, "0x2A is a configured-limits block, not live package power");
            ui.add_space(space::MD);

            // The slider maximum comes from the firmware, never from a constant.
            // The ceiling is 25 W on USB-C PD and 65 W on the 230 W brick; a
            // hardcoded 25 would make the app unable to use the machine's
            // headroom on a brick.
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
                    "range {}\u{2013}{} W on this supply \u{00b7} PL4 ceiling {} W",
                    hp_wmi::CpuPowerLimits::MIN_WATTS,
                    ceiling,
                    st.limits
                        .map(|l| l.pl4_ceiling())
                        .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL4)
                ),
            );
            ui.add_space(space::SM);
            ui.horizontal(|ui| {
                // Not "Restore 25 W": on a brick that would be throttling.
                if w::quiet_button(ui, p, "Restore stock", st.elevated).clicked() {
                    self.write(ControlRequest::RestoreDefault);
                }
                c::caption(ui, p, "firmware re-applies its own block");
            });
            // P1-3's decision, stated where the control is rather than in a
            // changelog: the cap is sticky. It is written to config.toml and
            // re-applied by the logon task, so it survives a reboot. Leaving that
            // implicit is how a user ends up with a throttled machine and no idea
            // why.
            ui.add_space(space::SM);
            c::body_text(
                ui,
                p,
                "This cap stays until you change it or press Restore stock \u{2014} it is saved \
                 and re-applied at every login, including after a reboot.",
            );
            c::write_feedback(ui, p, st, self.pending_since);
        });
    }

    fn thermal_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let status = st.thermal_status;
        let current = hp_wmi::ThermalMode::from_status(status);
        let active = if status == hp_wmi::ThermalMode::Performance.expected_status() {
            1
        } else if status == hp_wmi::ThermalMode::Balanced.expected_status()
            || status == hp_wmi::ThermalMode::Cool.expected_status()
        {
            0
        } else {
            usize::MAX
        };
        w::card_badge(
            ui,
            p,
            "THERMAL PROFILE",
            Some((&format!("status {status:#04X}"), p.dim)),
            |ui| {
                let opts = [
                    ("Balanced", active == 0),
                    ("Performance", active == 1),
                    ("Cool", false),
                ];
                // Balanced and Cool share the same status byte, so the segmented
                // control shows the firmware's answer rather than the last click.
                if let Some(i) = w::segmented(ui, p, &opts, st.elevated) {
                    let mode = match i {
                        1 => hp_wmi::ThermalMode::Performance,
                        2 => hp_wmi::ThermalMode::Cool,
                        _ => hp_wmi::ThermalMode::Balanced,
                    };
                    self.write(ControlRequest::SetThermal(mode));
                }
                ui.add_space(space::SM);
                c::body_text(
                    ui,
                    p,
                    &match current {
                        Some(t) => format!("firmware reports: {t}"),
                        // Distinguish "the firmware said something unknown" from
                        // "we could not ask", which is what an unelevated run
                        // actually is. Showing 0x00 as an unknown mode would blame
                        // the firmware for our missing token.
                        None if !st.bios_ok => {
                            "status unavailable \u{2014} needs an elevated token".to_string()
                        }
                        None => format!("firmware status {status:#04X} is not a known mode"),
                    },
                );
                c::caption(
                    ui,
                    p,
                    "Balanced and Cool share status 0x1E; the byte is read back, not assumed",
                );
                c::write_feedback(ui, p, st, self.pending_since);
            },
        );
    }

    fn display_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        w::card(ui, p, "DISPLAY CADENCE", |ui| {
            let cur = display::current();
            if self.rates.is_empty() {
                w::empty(
                    ui,
                    p,
                    "no modes reported",
                    Some("EnumDisplaySettings returned nothing"),
                );
                return;
            }
            if let Some(c) = cur {
                ui.label(
                    egui::RichText::new(format!("{} \u{00d7} {} \u{00b7} {} Hz", c.width, c.height, c.hz))
                        .size(BODY)
                        .color(p.text)
                        .family(theme::numeric()),
                );
            }
            ui.add_space(space::SM);
            let rates = self.rates.clone();
            let mut chosen = None;
            ui.horizontal_wrapped(|ui| {
                for m in &rates {
                    let is_active = cur.map(|c| c.hz == m.hz).unwrap_or(false);
                    let text = egui::RichText::new(format!("{} Hz", m.hz))
                        .size(BODY)
                        .color(if is_active { p.shell } else { p.text })
                        .family(theme::numeric());
                    let btn = egui::Button::new(text)
                        .fill(if is_active { p.accent } else { p.raised })
                        .stroke(egui::Stroke::new(
                            theme::HAIRLINE,
                            if is_active { p.accent } else { p.border },
                        ))
                        .rounding(theme::radius::CONTROL)
                        .min_size(egui::vec2(0.0, 28.0));
                    if ui.add(btn).clicked() && !is_active {
                        chosen = Some(*m);
                    }
                }
            });
            if let Some(m) = chosen {
                match display::apply(m) {
                    Ok(()) => {
                        self.rate_msg = Some((format!("applied {}", m.label()), true));
                        self.ui.refresh_hz = Some(m.hz);
                        self.ui.save();
                        self.refresh_rates();
                    }
                    Err(e) => self.rate_msg = Some((format!("{}: {e}", m.label()), false)),
                }
            }
            if let Some((msg, ok)) = &self.rate_msg {
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(msg)
                        .size(LABEL)
                        .color(if *ok { p.green } else { p.red })
                        .family(theme::proportional()),
                );
            }
            c::caption(
                ui,
                p,
                "CDS_TEST runs first, so a rejected mode never reaches the screen",
            );
        });
    }
}

/// The measured envelope, so the slider's range is evidence rather than a guess.
fn measured_limits_card(ui: &mut egui::Ui, p: &Palette) {
    w::card(ui, p, "MEASURED LIMITS", |ui| {
        for (k, v) in [
            ("0x29 accepted, brick", "5\u{2013}65 W PL1, PL4 \u{2264} 55"),
            ("0x29 accepted, USB-C", "5\u{2013}25 W (PD profile)"),
            ("stock on brick", "65 / 55 / 55 W, stable 90 s"),
            ("stock on USB-C", "25 / 25 / 25 W"),
            ("clock at 65 W", "4425 MHz, 16 threads"),
            ("clock at 25 W", "3515 MHz, 910 MHz lower"),
            ("reset", "over-limit write \u{2192} firmware stock"),
            ("0x2A[0]", "not watts \u{2014} falls 64\u{2192}62 under load"),
        ] {
            c::kv_row(ui, p, k, v);
        }
        ui.add_space(space::SM);
        c::caption(
            ui,
            p,
            "The ceiling follows the supply. On USB-C the cap can only lower the profile; \
             on the brick it is a real 5\u{2013}65 W control.",
        );
    });
}
