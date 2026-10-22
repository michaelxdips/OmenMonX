//! Overview: live numbers only. No controls on this page.

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};

use crate::sampler::{History, SharedState};
use crate::theme::{space, Palette};
use crate::ui::components as c;
use crate::widgets as w;
use crate::App;

impl App {
    pub fn page_dashboard(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState, hist: &History) {
        self.thermal_stats(ui, p, st);
        ui.add_space(space::MD);

        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                self.history_card(ui, p, st, hist);
                ui.add_space(space::MD);
                self.battery_card(ui, p, st);
            }
            c::Column::Right => {
                self.fan_status_card(ui, p, st);
                ui.add_space(space::MD);
                self.curve_card(ui, p, st);
            }
        });
    }

    fn thermal_stats(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let s = st.sample.as_ref();
        let cpu = s.and_then(|x| x.cpu);
        let gpu = s.and_then(|x| x.gpu);
        let hotspot = s.and_then(|x| x.gpu_hotspot);

        w::card(ui, p, "THERMALS", |ui| {
            if cpu.is_none() && gpu.is_none() {
                w::empty(
                    ui,
                    p,
                    "no thermal sensors",
                    Some("0x23 needs an elevated token; battery readings still work"),
                );
                ui.add_space(space::SM);
                thermal_stat(ui, p, st);
                return;
            }
            ui.horizontal(|ui| {
                w::stat(
                    ui,
                    p,
                    "cpu",
                    &cpu.map(|v| format!("{v:.0} C")).unwrap_or_else(|| "--".into()),
                    cpu.map(|v| p.temp(v)).unwrap_or(p.faint()),
                );
                ui.add_space(space::MD);
                w::stat(
                    ui,
                    p,
                    "gpu",
                    &gpu.map(|v| format!("{v:.0} C")).unwrap_or_else(|| "--".into()),
                    gpu.map(|v| p.temp(v)).unwrap_or(p.faint()),
                );
                ui.add_space(space::MD);
                w::stat(
                    ui,
                    p,
                    "hotspot",
                    &hotspot
                        .map(|v| format!("{v:.0} C"))
                        .unwrap_or_else(|| "--".into()),
                    hotspot.map(|v| p.temp(v)).unwrap_or(p.faint()),
                );
                ui.add_space(space::MD);
                w::stat(ui, p, "clock", &format!("{:.0} MHz", st.clock_mhz), p.gpu);
                ui.add_space(space::MD);
                self.cap_stat(ui, p, st);
            });
            ui.add_space(space::SM);
            thermal_stat(ui, p, st);
        });
    }
    fn cap_stat(&self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let cap = st.limits.map(|l| l.pl1);
        let ceiling = st
            .limits
            .map(|l| l.pl1_ceiling())
            .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL1);
        let (text, colour) = match cap {
            Some(v) if v < ceiling => (format!("{v} W"), p.amber),
            Some(v) => (format!("{v} W"), p.green),
            None => ("--".into(), p.faint()),
        };
        w::stat(ui, p, "power cap", &text, colour);
    }

    fn history_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState, hist: &History) {
        w::card(ui, p, "THERMAL HISTORY", |ui| {
            if self.ui.compact {
                c::caption(ui, p, "hidden in compact mode");
                return;
            }
            if hist.cpu.len() < 2 {
                if st.bios_ok {
                    w::empty(ui, p, "collecting samples", Some("the chart fills at 1 Hz"));
                } else {
                    w::empty(
                        ui,
                        p,
                        "no samples - thermal source unavailable",
                        Some("run elevated to record the history"),
                    );
                }
                return;
            }
            let cpu_pts: PlotPoints = hist
                .cpu
                .iter()
                .enumerate()
                .map(|(i, v)| [i as f64, *v as f64])
                .collect();
            let gpu_pts: PlotPoints = hist
                .gpu
                .iter()
                .enumerate()
                .map(|(i, v)| [i as f64, *v as f64])
                .collect();
            Plot::new("hist")
                .height(160.0)
                .show_axes([false, true])
                .allow_drag(false)
                .allow_scroll(false)
                .allow_zoom(false)
                .allow_boxed_zoom(false)
                .show(ui, |pl| {
                    pl.line(Line::new(cpu_pts).color(p.cpu).width(1.8_f32));
                    pl.line(Line::new(gpu_pts).color(p.gpu).width(1.8_f32));
                });
        });
    }

    fn fan_status_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let lvl = st.sample.as_ref().and_then(|s| s.fan_level_pct);
        w::card(ui, p, "COOLING", |ui| {
            ui.horizontal(|ui| {
                w::stat(
                    ui,
                    p,
                    "fan level",
                    &lvl.map(|v| format!("{v} %")).unwrap_or_else(|| "--".into()),
                    match lvl {
                        Some(v) if v >= 80 => p.amber,
                        Some(_) => p.text,
                        None => p.faint(),
                    },
                );
                ui.add_space(space::MD);
                w::stat(
                    ui,
                    p,
                    "fans",
                    &st.fan_count.map(|n| format!("{n}")).unwrap_or_else(|| "--".into()),
                    p.text,
                );
            });
            ui.add_space(space::MD);
            if let Some(v) = lvl {
                let col = if v >= 80 {
                    p.amber
                } else if v >= 40 {
                    p.gpu
                } else {
                    p.green
                };
                w::meter(ui, p, "duty", &format!("{v} %"), v as f32 / 100.0, col);
            }
            ui.add_space(space::SM);
            c::caption(ui, p, "level byte is live; fan controls live under Control");
        });
    }

    fn battery_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "BATTERY", |ui| {
            let Some(b) = st.battery.as_ref() else {
                w::empty(ui, p, "battery not readable", Some("Win32_Battery returned nothing"));
                return;
            };
            let pct = b.charge_pct.unwrap_or(0);
            ui.horizontal(|ui| {
                w::stat(ui, p, "charge", &format!("{pct} %"), p.text);
                ui.add_space(space::MD);
                if let Some(h) = b.health_pct {
                    w::stat(
                        ui,
                        p,
                        "health",
                        &format!("{h} %"),
                        if h < 70 { p.red } else { p.green },
                    );
                }
                ui.add_space(space::MD);
                w::stat(
                    ui,
                    p,
                    "source",
                    if b.ac_online { "AC" } else { "Battery" },
                    if b.ac_online { p.green } else { p.amber },
                );
            });
            ui.add_space(space::MD);
            w::meter(
                ui,
                p,
                "charge",
                &format!("{pct} %"),
                pct as f32 / 100.0,
                if pct < 20 { p.red } else { p.green },
            );
            let mut extra = Vec::new();
            if let Some(cy) = b.cycles {
                extra.push(format!("{cy} cycles"));
            }
            if let Some(t) = b.temp_c {
                extra.push(format!("{t:.1} C"));
            }
            if let Some(h) = b.health_pct {
                extra.push(format!("{} % wear", 100u32.saturating_sub(h as u32)));
            }
            if !extra.is_empty() {
                ui.add_space(space::SM);
                c::caption(ui, p, &extra.join("   -   "));
            }
        });
    }

    fn curve_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "FAN CURVE - FIRMWARE", |ui| {
            let Some(t) = st.fan_table.as_ref() else {
                w::empty(ui, p, "fan table unreadable", Some("requires the BIOS WMI interface"));
                return;
            };
            c::caption(
                ui,
                p,
                &format!("{} fans - {} points - read-only", t.fan_count, t.level_count),
            );
            if self.ui.compact {
                return;
            }
            ui.add_space(space::SM);
            // The fan curve is static firmware data, but this card runs on the
            // 250 ms repaint loop. Rebuilding the point series every frame
            // allocates for nothing; rebuild only when the sampler actually
            // refreshed the table (generation counter), else reuse the cache.
            if self.curve_cache_gen != st.fan_table_gen || self.curve_cache.is_none() {
                let fans = t.fan_count as usize;
                self.curve_cache = Some(
                    (0..fans.min(3))
                        .map(|f| {
                            t.points
                                .iter()
                                .enumerate()
                                .filter_map(|(i, pt)| {
                                    pt.duty(f, t.fan_count).map(|d| [i as f64, d as f64])
                                })
                                .collect::<Vec<[f64; 2]>>()
                        })
                        .collect::<Vec<Vec<[f64; 2]>>>(),
                );
                self.curve_cache_gen = st.fan_table_gen;
            }
            let Some(series) = &self.curve_cache else {
                return;
            };
            // Series identity is not colour-only: a legend names every fan, and
            // the palette cycles through four distinguishable channels.
            let palette = [p.cpu, p.gpu, p.text, p.dim];
            Plot::new("fw_curve")
                .height(132.0)
                .allow_boxed_zoom(false)
                .allow_scroll(false)
                .allow_drag(false)
                .allow_zoom(false)
                .legend(egui_plot::Legend::default())
                .x_axis_label("level")
                .y_axis_label("duty %")
                .show(ui, |pl| {
                    for (f, pts) in series.iter().enumerate() {
                        pl.line(
                            Line::new(PlotPoints::from(pts.clone()))
                                .name(format!("fan{}", f + 1))
                                .color(palette[f % palette.len()])
                                .width(2.0_f32),
                        );
                    }
                });
            c::caption(
                ui,
                p,
                &format!(
                    "peak {}%  -  monotonic {}",
                    t.max_duty(),
                    if t.is_monotonic() { "yes" } else { "no" }
                ),
            );
        });
    }
}

fn thermal_stat(ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
    let therm = hp_wmi::ThermalMode::from_status(st.thermal_status);
    let text = match therm {
        Some("performance (odd mode code)") => "Performance",
        Some(_) => "Balanced / Cool",
        None => "unknown",
    };
    w::stat(ui, p, "thermal profile", text, p.text);
}
