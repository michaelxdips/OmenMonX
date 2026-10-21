//! OmenMon-rs GUI (P4): egui instrument panel.
//! Graphite #16181D bg, amber #FFB454 values, red #E5484D warnings only,
//! mono numerals. Panels: thermal strip (CPU/GPU sparkline), fan card,
//! battery card, fan-curve editor, updater.

mod sampler;
mod updater;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Points};
use sampler::{History, Sampler};
use updater::UpdateHandle;

const BG: egui::Color32 = egui::Color32::from_rgb(0x16, 0x18, 0x1D);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x20, 0x27);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x2E, 0x38);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB4, 0x54);
const BLUE: egui::Color32 = egui::Color32::from_rgb(0x6E, 0xA8, 0xDC);
const RED: egui::Color32 = egui::Color32::from_rgb(0xE5, 0x48, 0x4D);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x6B, 0xC9, 0x7B);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xD8, 0xDA, 0xE0);
const DIM: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x8F, 0x9A);

fn temp_color(t: f32) -> egui::Color32 {
    if t >= 85.0 {
        RED
    } else if t >= 70.0 {
        AMBER
    } else {
        GREEN
    }
}

struct App {
    sampler: Sampler,
    curve_pts: Vec<[f64; 2]>,
    updates: UpdateHandle,
    dirty: bool,
    last_save: std::time::Instant,
    install_dir: String,
}

fn install_dir_guess() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if dir.starts_with(r"C:\Program Files") {
        dir.to_string_lossy().to_string()
    } else {
        r"C:\Program Files\OmenMon".into()
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        let mut style = (*ctx.style()).clone();
        style.visuals.dark_mode = true;
        style.visuals.panel_fill = PANEL;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = BG;
        style.visuals.override_text_color = Some(TEXT);
        ctx.set_style(style);

        let cfg = hp_core::Config::load();
        let curve_pts = cfg
            .profile
            .curve
            .iter()
            .map(|p| [p.temp_c as f64, p.fan_pct as f64])
            .collect();
        Self {
            sampler: Sampler::spawn(1000),
            curve_pts,
            updates: UpdateHandle::default(),
            dirty: false,
            last_save: std::time::Instant::now(),
            install_dir: install_dir_guess(),
        }
    }

    fn save_curve(&self) {
        let mut cfg = hp_core::Config::load();
        cfg.profile.curve = self
            .curve_pts
            .iter()
            .map(|p| hp_core::config::FanCurvePoint {
                temp_c: p[0].round().clamp(0.0, 120.0) as u8,
                fan_pct: p[1].round().clamp(0.0, 100.0) as u8,
            })
            .collect();
        cfg.profile.curve.sort_by_key(|p| p.temp_c);
        let _ = cfg.save();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        // autosave edited curve every 2 s
        if self.dirty && self.last_save.elapsed().as_secs() >= 2 {
            self.save_curve();
            self.dirty = false;
            self.last_save = std::time::Instant::now();
        }

        let st = self.sampler.state.lock().clone();
        let hist = self.sampler.history.lock().clone();

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("OMENMON").color(AMBER).monospace().size(20.0));
                    ui.label(
                        egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                            .color(DIM)
                            .monospace(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (txt, c) = if st.bios_ok {
                            ("telemetry OK", GREEN)
                        } else {
                            ("degraded — run elevated for full telemetry", RED)
                        };
                        ui.label(egui::RichText::new(format!("● {txt}")).color(c).monospace());
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG).inner_margin(10.0))
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        thermal_card(ui, &st, &hist);
                        ui.add_space(10.0);
                        fan_card(ui, &st);
                        ui.add_space(10.0);
                        battery_card(ui, st.battery.as_ref());
                    });
                    cols[1].vertical(|ui| {
                        curve_card(ui, &mut self.curve_pts, &st.target_fan_pct, &mut self.dirty);
                        ui.add_space(10.0);
                        updater_card(ui, &self.updates, &self.install_dir);
                    });
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sampler.stop();
        self.save_curve();
    }
}

// ------------------------------------------------------------------ cards --

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .rounding(6.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).color(DIM).monospace().small());
            ui.add_space(4.0);
            body(ui);
        });
}

fn big_value(ui: &mut egui::Ui, label: &str, v: f32, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).color(DIM).monospace().small());
        ui.label(
            egui::RichText::new(format!("{v:.0}°"))
                .size(30.0)
                .color(color)
                .monospace(),
        );
    });
}

fn thermal_card(ui: &mut egui::Ui, st: &sampler::SharedState, hist: &History) {
    card(ui, "THERMAL", |ui| {
        let s = st.sample.as_ref();
        let cpu = s.and_then(|s| s.cpu).unwrap_or(0.0);
        let gpu = s.and_then(|s| s.gpu).unwrap_or(0.0);
        ui.horizontal(|ui| {
            big_value(ui, "CPU", cpu, temp_color(cpu));
            ui.add_space(24.0);
            big_value(ui, "GPU", gpu, temp_color(gpu));
            if let Some(h) = s.and_then(|s| s.gpu_hotspot) {
                ui.add_space(24.0);
                big_value(ui, "HOTSPOT", h, temp_color(h));
            }
        });
        ui.add_space(6.0);
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
        Plot::new("strip")
            .height(100.0)
            .show_axes([false, true])
            .allow_drag(false)
            .allow_scroll(false)
            .allow_zoom(false)
            .allow_boxed_zoom(false)
            .legend(egui_plot::Legend::default())
            .show(ui, |p| {
                p.line(Line::new(cpu_pts).name("CPU").color(AMBER).width(1.5));
                p.line(Line::new(gpu_pts).name("GPU").color(BLUE).width(1.5));
            });
        if let Some(e) = &st.last_error {
            ui.label(egui::RichText::new(e).color(RED).monospace().small());
        }
    });
}

fn fan_card(ui: &mut egui::Ui, st: &sampler::SharedState) {
    card(ui, "FAN", |ui| {
        let s = st.sample.as_ref();
        ui.horizontal(|ui| {
            let rpm = s
                .and_then(|s| s.fan_rpm)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "—".into());
            ui.label(egui::RichText::new(rpm).size(26.0).color(AMBER).monospace());
            ui.label(egui::RichText::new("RPM").color(DIM).monospace());
            ui.add_space(24.0);
            let lvl = s
                .and_then(|s| s.fan_level_pct)
                .map(|v| format!("{v} %"))
                .unwrap_or_else(|| "—".into());
            ui.label(egui::RichText::new(lvl).size(26.0).color(TEXT).monospace());
            ui.label(egui::RichText::new("level").color(DIM).monospace());
        });
        ui.add_space(4.0);
        ui.add(
            egui::ProgressBar::new(st.target_fan_pct as f32 / 100.0)
                .show_percentage()
                .desired_height(8.0),
        );
        ui.label(
            egui::RichText::new(format!(
                "curve target {} % — EC fan write pending signed PawnIO module",
                st.target_fan_pct
            ))
            .color(DIM)
            .small(),
        );
    });
}

fn battery_card(ui: &mut egui::Ui, b: Option<&hp_core::battery::BatteryInfo>) {
    card(ui, "BATTERY", |ui| {
        match b {
            None => {
                ui.label(egui::RichText::new("unavailable").color(DIM).monospace());
            }
            Some(b) => {
                let pct = b.charge_pct.unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{pct} %"))
                            .size(26.0)
                            .color(AMBER)
                            .monospace(),
                    );
                    ui.label(
                        egui::RichText::new(if b.ac_online { "AC" } else { "BAT" })
                            .color(if b.ac_online { GREEN } else { DIM })
                            .monospace(),
                    );
                    ui.add_space(20.0);
                    if let Some(h) = b.health_pct {
                        ui.label(
                            egui::RichText::new(format!("health {h} %"))
                                .color(if h < 70 { RED } else { GREEN })
                                .monospace(),
                        );
                    }
                });
                ui.add_space(2.0);
                ui.add(
                    egui::ProgressBar::new(pct as f32 / 100.0)
                        .show_percentage()
                        .desired_height(8.0),
                );
                let mut extra = String::new();
                if let Some(cy) = b.cycles {
                    extra.push_str(&format!("cycles {cy}"));
                }
                if let Some(t) = b.temp_c {
                    if !extra.is_empty() {
                        extra.push_str("   ·   ");
                    }
                    extra.push_str(&format!("temp {t:.1} °C"));
                }
                if !extra.is_empty() {
                    ui.label(egui::RichText::new(extra).color(DIM).monospace().small());
                }
            }
        }
    });
}

fn curve_card(ui: &mut egui::Ui, pts: &mut Vec<[f64; 2]>, target: &u8, dirty: &mut bool) {
    card(ui, "FAN CURVE", |ui| {
        ui.label(
            egui::RichText::new("drag the nearest point to edit · autosaved")
                .color(DIM)
                .small(),
        );
        let lines = pts.clone();
        let resp = Plot::new("curve")
            .height(190.0)
            .allow_boxed_zoom(false)
            .allow_scroll(false)
            .allow_drag(false)
            .x_axis_label("°C")
            .y_axis_label("%")
            .show(ui, |p| {
                p.line(
                    Line::new(PlotPoints::from(lines.clone()))
                        .name("curve")
                        .color(AMBER)
                        .width(2.0),
                );
                p.line(
                    Line::new(PlotPoints::new(vec![
                        [0.0, *target as f64],
                        [100.0, *target as f64],
                    ]))
                    .name("target")
                    .color(RED)
                    .width(1.0),
                );
                for (i, pt) in pts.iter().enumerate() {
                    p.points(
                        Points::new(PlotPoints::new(vec![*pt]))
                            .id(egui::Id::new(("curve_pt", i)))
                            .radius(5.0)
                            .color(AMBER)
                            .highlight(true),
                    );
                }
            });
        // drag nearest point under pointer
        if resp.response.dragged() {
            if let Some(pos) = resp.response.interact_pointer_pos() {
                if let Some(coord) = Some(resp.transform.value_from_position(pos)) {
                    if let Some(nearest) = pts
                        .iter()
                        .enumerate()
                        .min_by(|a, b| {
                            let d = |p: &[f64; 2]| {
                                (p[0] - coord.x).powi(2) + (p[1] - coord.y).powi(2)
                            };
                            d(a.1).partial_cmp(&d(b.1)).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(i, _)| i)
                    {
                        let p = pts[nearest];
                        let dist = ((p[0] - coord.x).powi(2) + (p[1] - coord.y).powi(2)).sqrt();
                        if dist < 6.0 {
                            pts[nearest] = [
                                coord.x.clamp(0.0, 120.0).round(),
                                coord.y.clamp(0.0, 100.0).round(),
                            ];
                            *dirty = true;
                            pts.sort_by(|a, b| {
                                a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                    }
                }
            }
        }
        ui.label(
            egui::RichText::new(format!("{} points · autosaved to config.toml", pts.len()))
                .color(DIM)
                .small(),
        );
    });
}

fn updater_card(ui: &mut egui::Ui, handle: &UpdateHandle, install_dir: &str) {
    card(ui, "UPDATER", |ui| {
        let s = handle.0.lock();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("current v{}", updater::current_version()))
                    .color(TEXT)
                    .monospace(),
            );
            if let Some(l) = &s.latest {
                ui.label(
                    egui::RichText::new(format!("latest v{l}"))
                        .color(if s.available { GREEN } else { DIM })
                        .monospace(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if s.available
                        && ui
                            .add_enabled(!s.busy, egui::Button::new("Download & install"))
                            .clicked()
                    {
                        handle.update_async(install_dir);
                    }
                });
            } else if ui
                .add_enabled(!s.busy, egui::Button::new("Check for updates"))
                .clicked()
            {
                handle.check_async();
            }
        });
        if s.busy {
            match s.progress {
                Some((done, total)) if total > 0 => {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32).desired_height(10.0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {} MB",
                            done / 1_048_576,
                            total / 1_048_576
                        ))
                        .color(DIM)
                        .small(),
                    );
                }
                Some((done, _)) => {
                    ui.spinner();
                    ui.label(egui::RichText::new(format!("{done} bytes")).color(DIM).small());
                }
                None => {
                    ui.spinner();
                    ui.label(egui::RichText::new("contacting github…").color(DIM).small());
                }
            }
        }
        if let Some(e) = &s.err {
            ui.label(egui::RichText::new(e).color(RED).small());
        }
        if let Some(m) = &s.done_msg {
            ui.label(egui::RichText::new(m).color(GREEN).small());
        }
    });
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("OmenMon-rs"),
        ..Default::default()
    };
    eframe::run_native(
        "OmenMon-rs",
        opts,
        Box::new(|cc| {
            let app = App::new(cc);
            app.updates.check_async(); // silent startup check
            Ok(Box::new(app))
        }),
    )
}
