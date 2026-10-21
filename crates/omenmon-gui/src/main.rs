//! OmenMon-rs GUI (P4): egui instrument panel.
//! Graphite #16181D bg, amber #FFB454 values, red #E5484D warnings only,
//! mono numerals. Panels: thermal strip (CPU/GPU sparkline), fan card,
//! battery card, fan-curve editor, updater.

mod sampler;
mod updater;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
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
    updates: UpdateHandle,
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

        Self {
            sampler: Sampler::spawn(1000),
            updates: UpdateHandle::default(),
            install_dir: install_dir_guess(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));


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
                        curve_card(ui, st.fan_table.as_ref());
                        ui.add_space(10.0);
                        control_card(ui, &st, &self.sampler.request);
                        ui.add_space(10.0);
                        updater_card(ui, &self.updates, &self.install_dir);
                    });
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sampler.stop();
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

/// Read-only view of the firmware's own fan curve.
///
/// This replaces the previous draggable editor. On board 8C9C the fan write
/// paths are inert — `0x2E` (SetSwFanControlLevel) and `0x32` (SetFanTable) both
/// accept a payload and change nothing, and the EC never enters manual mode — so
/// an editable curve here would be a control that lies. What the firmware does
/// expose is the real 42-point table, which OGH never shows.
fn curve_card(ui: &mut egui::Ui, table: Option<&hp_wmi::FanTable>) {
    card(ui, "FAN CURVE (firmware)", |ui| {
        let Some(t) = table else {
            ui.label(egui::RichText::new("curve unavailable").color(DIM).monospace());
            return;
        };

        ui.label(
            egui::RichText::new(format!(
                "{} fans · {} points · read-only",
                t.fan_count, t.level_count
            ))
            .color(DIM)
            .small()
            .monospace(),
        );
        ui.add_space(4.0);

        // Plot duty against the level index. Column 0 is a firmware level index
        // (dB), not a temperature, so the x-axis is labelled accordingly.
        let fans = t.fan_count as usize;
        let series: Vec<Vec<[f64; 2]>> = (0..fans.min(3))
            .map(|f| {
                t.points
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.duty(f, t.fan_count).map(|d| [i as f64, d as f64]))
                    .collect()
            })
            .collect();

        Plot::new("fw_curve")
            .height(170.0)
            .allow_boxed_zoom(false)
            .allow_scroll(false)
            .allow_drag(false)
            .allow_zoom(false)
            .x_axis_label("level")
            .y_axis_label("duty %")
            .show(ui, |p| {
                for (f, pts) in series.iter().enumerate() {
                    let colour = if f == 0 { AMBER } else { BLUE };
                    p.line(
                        Line::new(PlotPoints::from(pts.clone()))
                            .name(format!("fan{}", f + 1))
                            .color(colour)
                            .width(2.0),
                    );
                }
            });

        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(format!(
                "peak duty {}%  ·  monotonic: {}",
                t.max_duty(),
                if t.is_monotonic() { "yes" } else { "no" }
            ))
            .color(DIM)
            .small()
            .monospace(),
        );
        ui.label(
            egui::RichText::new("column 0 is a firmware level index, not a temperature")
                .color(DIM)
                .small()
                .monospace(),
        );
    });
}

/// Power cap + thermal mode. Both channels are verified on this board.
///
/// The cap is a downward-only ceiling: the firmware accepts 5..25 W and resets
/// anything above 25 to 25. It is a quiet/cool control, never a boost.
fn control_card(
    ui: &mut egui::Ui,
    st: &sampler::SharedState,
    request: &parking_lot::Mutex<Option<sampler::ControlRequest>>,
) {
    use hp_wmi::{CpuPowerLimits, ThermalMode};

    card(ui, "POWER & THERMAL", |ui| {
        // ---- current limits ----
        match st.limits {
            Some(l) => {
                ui.horizontal(|ui| {
                    for (name, v) in [
                        ("PL1", l.pl1),
                        ("PL2", l.pl2),
                        ("PL4", l.pl4),
                        ("GPU", l.pl_with_gpu),
                    ] {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(name).color(DIM).small().monospace());
                            ui.label(
                                egui::RichText::new(format!("{v} W"))
                                    .color(AMBER)
                                    .monospace()
                                    .size(16.0),
                            );
                        });
                        ui.add_space(10.0);
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("CLOCK").color(DIM).small().monospace());
                        ui.label(
                            egui::RichText::new(format!("{:.0} MHz", st.clock_mhz))
                                .color(BLUE)
                                .monospace()
                                .size(16.0),
                        );
                    });
                });
            }
            None => {
                ui.label(egui::RichText::new("limits unavailable").color(DIM).monospace());
            }
        }

        ui.add_space(8.0);

        // ---- cap slider ----
        // The slider is bounded by the proven window, so it cannot offer a value
        // the firmware will silently reject.
        let mut cap = st.limits.map(|l| l.pl1).unwrap_or(CpuPowerLimits::MAX_WATTS);
        ui.label(
            egui::RichText::new(format!(
                "power cap  ({}–{} W, downward only)",
                CpuPowerLimits::MIN_WATTS,
                CpuPowerLimits::MAX_WATTS
            ))
            .color(DIM)
            .small()
            .monospace(),
        );
        let resp = ui.add(
            egui::Slider::new(
                &mut cap,
                CpuPowerLimits::MIN_WATTS..=CpuPowerLimits::MAX_WATTS,
            )
            .suffix(" W"),
        );
        if resp.drag_stopped() || resp.lost_focus() {
            *request.lock() = Some(sampler::ControlRequest::SetPowerCap(cap));
        }
        ui.horizontal(|ui| {
            if ui.small_button("restore default").clicked() {
                *request.lock() = Some(sampler::ControlRequest::RestoreDefault);
            }
            ui.label(
                egui::RichText::new("firmware resets to 25 W on its own")
                    .color(DIM)
                    .small()
                    .monospace(),
            );
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        // ---- thermal mode ----
        let status = st.thermal_status;
        let current = ThermalMode::from_status(status).unwrap_or("unknown");
        ui.label(
            egui::RichText::new(format!("thermal mode  (firmware status {status:#04X} — {current})"))
                .color(DIM)
                .small()
                .monospace(),
        );
        ui.horizontal(|ui| {
            for (label, mode) in [
                ("balanced", ThermalMode::Balanced),
                ("performance", ThermalMode::Performance),
                ("cool", ThermalMode::Cool),
            ] {
                let active = status == mode.expected_status();
                let text = egui::RichText::new(label).monospace();
                let text = if active { text.color(AMBER) } else { text.color(TEXT) };
                if ui.add(egui::Button::new(text).small()).clicked() {
                    *request.lock() = Some(sampler::ControlRequest::SetThermal(mode));
                }
            }
        });
        ui.label(
            egui::RichText::new("status byte is read back from the firmware, not assumed")
                .color(DIM)
                .small()
                .monospace(),
        );

        // ---- last write result ----
        if let Some(msg) = &st.last_write {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(msg)
                    .color(if msg.contains("not") || msg.contains("failed") {
                        RED
                    } else {
                        GREEN
                    })
                    .small()
                    .monospace(),
            );
        }
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
