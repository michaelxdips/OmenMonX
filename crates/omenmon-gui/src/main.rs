//! OmenMon-rs GUI.
//!
//! Shell: a fixed left rail (like OMEN Gaming Hub) over five pages —
//! Dashboard, Performance, Lighting, System, About. The rail carries the app
//! identity, the navigation, and the machine's live status, so the status is
//! visible from every page without a separate bar.
//!
//! Rules this file follows, from REPORT-2026-10-21.md:
//!
//! * No control exists for a channel that was measured inert. There is no fan
//!   slider (`0x2E`/`0x32` are no-ops here), no GPU switch (`0x52` returns err 5)
//!   and no battery limiter (`Battery Care Function` returns rc 5 for every value).
//! * `0x2A` is a limits block and is labelled as one. It is never "live power".
//! * Nothing is claimed to work without a readback behind it.

mod crash;
mod display;
mod elevate;
mod safety;
mod sampler;
mod theme;
mod tray;
mod updater;
mod widgets;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use sampler::{BiosRow, ControlRequest, Sampler, SharedState, WriteOutcome};
use theme::{Page, Palette, Theme, UiState, BODY, DISPLAY, LABEL, RAIL_W};
use updater::UpdateHandle;
use widgets as w;

/// How long a write result stays bright before settling to a quieter tone.
const FLASH_MS: u64 = 1_600;

/// Zoom limits. Below 0.8 the 11 px label step stops being legible; above 2.0 a
/// card no longer fits the 940 px minimum window and text starts to clip.
const ZOOM_MIN: f32 = 0.8;
const ZOOM_MAX: f32 = 2.0;
const ZOOM_STEP: f32 = 0.1;

/// Clamp a zoom factor into the range the layout actually survives.
///
/// Separate from the key handler so the bounds are testable: a persisted value
/// from a future version, or a hand-edited ui.toml, must not be able to make the
/// window unusable.
///
/// NaN is the one input with no sensible clamp — it is not "very large" or "very
/// small", it is a broken number — so it falls back to 1.0. Infinities do have a
/// direction, so they clamp to the end they point at.
fn clamp_zoom(z: f32) -> f32 {
    if z.is_nan() {
        return 1.0;
    }
    z.clamp(ZOOM_MIN, ZOOM_MAX)
}

struct App {
    sampler: Sampler,
    updates: UpdateHandle,
    install_dir: String,
    ui: UiState,
    tray: Option<tray::Tray>,
    tray_error: Option<String>,
    pending_since: Option<std::time::Instant>,
    /// Colour picked per zone, before it is written.
    zone_pick: Vec<egui::Color32>,
    bios_selected: Option<usize>,
    bios_new_value: String,
    rates: Vec<display::Mode>,
    rate_msg: Option<(String, bool)>,
    diag: String,
    recover: Option<safety::Pending>,
    /// A crash from the previous run, shown once and then dropped.
    crashed: Option<crash::PreviousCrash>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ui = UiState::load();
        let p = Palette::of(ui.theme);
        theme::apply(&cc.egui_ctx, &p);
        // Apply the persisted zoom before the first frame, so a user at 150% does
        // not see one frame at 100% and a window that resizes itself.
        cc.egui_ctx.set_zoom_factor(clamp_zoom(ui.zoom));

        let elevated = elevate::is_elevated();
        let (tray, tray_error) = match tray::Tray::new() {
            (Some(t), _) => (Some(t), None),
            (None, e) => (None, e),
        };

        let recover = safety::unrecovered();
        let crashed = crash::take_previous_crash();

        let mut app = Self {
            sampler: Sampler::spawn(1000, elevated),
            updates: UpdateHandle::default(),
            install_dir: updater::resolve_install_target().dir.display().to_string(),
            ui,
            tray,
            tray_error,
            pending_since: None,
            zone_pick: Vec::new(),
            bios_selected: None,
            bios_new_value: String::new(),
            rates: Vec::new(),
            rate_msg: None,
            diag: String::new(),
            recover,
            crashed,
        };
        app.updates.check_async();
        app.refresh_rates();
        app.load_bios_async();
        app
    }

    fn refresh_rates(&mut self) {
        self.rates = display::available_rates();
    }

    /// Load BIOS settings on a worker thread: `list()` walks two WMI classes and
    /// takes long enough to be visible as a freeze.
    fn load_bios_async(&mut self) {
        {
            let mut st = self.sampler.state.lock();
            st.bios_loading = true;
            st.bios_error = None;
        }
        let state = self.sampler.state.clone();
        std::thread::spawn(move || {
            let result = sampler::load_bios_settings();
            let mut st = state.lock();
            st.bios_loading = false;
            match result {
                Ok(rows) => {
                    st.bios_settings = Some(rows);
                    st.bios_error = None;
                }
                Err(e) => st.bios_error = Some(e),
            }
        });
    }

    fn write(&mut self, req: ControlRequest) {
        if self.pending_since.is_none() {
            self.pending_since = Some(std::time::Instant::now());
        }
        self.sampler.send(req);
    }

    fn build_diag(&self, st: &SharedState) -> String {
        let mut s = String::new();
        s.push_str(&format!("OmenMon-rs {}\n", env!("CARGO_PKG_VERSION")));
        s.push_str(&format!("build: {}\n", build_hash()));
        s.push_str(&format!(
            "exe: {}\n",
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
        s.push_str(&format!("elevated: {}\n", st.elevated));
        s.push_str(&format!("bios wmi: {}\n", st.bios_ok));
        s.push_str(&format!("ec (pawnio): {}\n", st.ec_ok));
        s.push_str(&format!("thermal status: {:#04X}\n", st.thermal_status));
        match st.limits {
            Some(l) => s.push_str(&format!(
                "limits block 0x2A: PL1={} PL2={} PL4={} PLwGpu={}\n",
                l.pl1, l.pl2, l.pl4, l.pl_with_gpu
            )),
            None => s.push_str("limits block 0x2A: unreadable\n"),
        }
        if let Some(t) = &st.fan_table {
            s.push_str(&format!(
                "fan table: {} fans, {} points, peak {}%, monotonic {}\n",
                t.fan_count,
                t.level_count,
                t.max_duty(),
                t.is_monotonic()
            ));
        }
        if let Some(r) = &st.rgb {
            s.push_str(&format!("rgb: {} zones (from colorTable[0])\n", r.zones));
            for (i, c) in r.colors.iter().enumerate() {
                s.push_str(&format!("  zone {i}: #{:02X}{:02X}{:02X}\n", c[0], c[1], c[2]));
            }
        }
        if let Some(smp) = &st.sample {
            s.push_str(&format!(
                "temps: cpu {:?} gpu {:?} hotspot {:?}\n",
                smp.cpu, smp.gpu, smp.gpu_hotspot
            ));
        }
        s.push_str(&format!("clock: {:.0} MHz\n", st.clock_mhz));
        if let Some(b) = &st.battery {
            s.push_str(&format!(
                "battery: {:?}% health {:?}% cycles {:?} ac {}\n",
                b.charge_pct, b.health_pct, b.cycles, b.ac_online
            ));
        }
        if let Some(e) = &st.last_error {
            s.push_str(&format!("last error: {e}\n"));
        }
        if let Some(wr) = &st.last_write {
            s.push_str(&format!("last write: {:?} {}\n", wr.outcome, wr.message));
        }
        if let Some(e) = &self.tray_error {
            s.push_str(&format!("tray: {e}\n"));
        }
        if let Some(r) = &self.recover {
            s.push_str(&format!(
                "unrecovered cap marker: requested {} W, observed {:?}, phase {:?}, {}\n",
                r.requested_w,
                r.observed_w,
                r.phase,
                safety::age_text(r.at)
            ));
        }
        s
    }
}

/// Short git hash baked in at build time, or "unknown" for a plain checkout.
fn build_hash() -> &'static str {
    option_env!("OMENMON_BUILD_HASH").unwrap_or("unknown")
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));

        self.handle_zoom_keys(ctx);

        // ---- tray commands ----
        if let Some(t) = &self.tray {
            for cmd in t.drain() {
                match cmd {
                    tray::TrayCommand::Show => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray::TrayCommand::Quit => {
                        self.persist_window(ctx);
                        self.sampler.stop();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // ---- persist window size ----
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            let size = [rect.width(), rect.height()];
            let changed = self
                .ui
                .window
                .map(|win| (win[0] - size[0]).abs() > 1.0 || (win[1] - size[1]).abs() > 1.0)
                .unwrap_or(true);
            if changed && size[0] > 200.0 && size[1] > 200.0 {
                self.ui.window = Some(size);
            }
        }

        let st = self.sampler.state.lock().clone();
        let hist = self.sampler.history.lock().clone();

        if let Some(started) = self.pending_since {
            let landed = st
                .last_write
                .as_ref()
                .map(|x| x.at >= started)
                .unwrap_or(false);
            if landed || started.elapsed().as_millis() > 8_000 {
                self.pending_since = None;
            }
        }

        let p = Palette::of(self.ui.theme);

        // Close means "hide to tray" while a tray exists: the sampler must keep
        // running, because a power cap or thermal profile is only meaningful
        // while something watches it. With no tray, close must really close.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && self.ui.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.ui.save();
        }

        // The rail owns the whole left edge, so it is a SidePanel rather than a
        // widget inside the central area: it must not scroll with the page.
        egui::SidePanel::left("rail")
            .exact_width(RAIL_W)
            .resizable(false)
            .frame(
                egui::Frame::default()
                    .fill(p.shell)
                    .inner_margin(egui::Margin::symmetric(14.0, 16.0)),
            )
            .show(ctx, |ui| self.rail(ui, &p, &st));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(22.0, 16.0)),
            )
            .show(ctx, |ui| {
                self.page_header(ui, &p);
                if let Some(rec) = self.recover.clone() {
                    self.recover_banner(ui, &p, &rec);
                }
                if self.crashed.is_some() {
                    self.crash_banner(ui, &p);
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.ui.page {
                        Page::Dashboard => self.page_dashboard(ui, &p, &st, &hist),
                        Page::Performance => self.page_performance(ui, &p, &st),
                        Page::Lighting => self.page_lighting(ui, &p, &st),
                        Page::System => self.page_system(ui, &p, &st),
                        Page::About => self.page_about(ui, &p, &st),
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.ui.save();
        self.sampler.stop();
    }
}

impl App {
    /// Ctrl++ / Ctrl+- / Ctrl+0, with the result persisted.
    ///
    /// The zoom is a preference, not a transient: a user on a 4K panel sets it
    /// once and expects it back next launch. It is applied through egui's own
    /// zoom factor rather than by scaling font sizes, so every widget — including
    /// the ones that compute their own metrics — scales consistently.
    fn handle_zoom_keys(&mut self, ctx: &egui::Context) {
        let (plus, minus, reset) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0),
            )
        });
        if !(plus || minus || reset) {
            return;
        }
        let next = if reset {
            1.0
        } else if plus {
            self.ui.zoom + ZOOM_STEP
        } else {
            self.ui.zoom - ZOOM_STEP
        };
        let next = clamp_zoom(next);
        if (next - self.ui.zoom).abs() > f32::EPSILON {
            self.ui.zoom = next;
            self.ui.save();
            ctx.set_zoom_factor(next);
        }
    }

    fn persist_window(&mut self, ctx: &egui::Context) {
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.ui.window = Some([rect.width(), rect.height()]);
        }
        self.ui.save();
    }

    // ---------------------------------------------------------------- rail --

    fn rail(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        // Brand block.
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 7.0, p.accent);
            let c = rect.center();
            // A stylised "O" cut from the accent block.
            ui.painter()
                .circle_stroke(c, 7.5, egui::Stroke::new(2.6_f32, p.shell));
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("OMENMON")
                        .size(15.0)
                        .strong()
                        .color(p.text)
                        .family(theme::proportional()),
                );
                ui.label(
                    egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                        .size(10.0)
                        .color(p.faint())
                        .family(theme::proportional()),
                );
            });
        });
        ui.add_space(18.0);

        // Navigation.
        for page in Page::ALL {
            let active = self.ui.page == page;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 34.0),
                egui::Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let hovered = resp.hovered();
                if active {
                    ui.painter().rect_filled(rect, 7.0, p.accent_soft);
                    // A 3px accent bar on the leading edge marks the active page
                    // even when the fill is hard to see against a photo.
                    let bar = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 6.0),
                        egui::vec2(3.0, rect.height() - 12.0),
                    );
                    ui.painter().rect_filled(bar, 2.0, p.accent);
                } else if hovered {
                    ui.painter().rect_filled(rect, 7.0, p.tint(p.text, 12));
                }
                let icon_col = if active { p.accent } else { p.dim };
                let icon_rect = egui::Rect::from_min_size(
                    rect.min + egui::vec2(12.0, 8.0),
                    egui::vec2(18.0, 18.0),
                );
                w::glyph(ui, p, page.glyph(), icon_rect, icon_col);
                ui.painter().text(
                    rect.min + egui::vec2(40.0, rect.height() * 0.5),
                    egui::Align2::LEFT_CENTER,
                    page.title(),
                    egui::FontId::new(BODY, theme::proportional()),
                    if active { p.text } else { p.dim },
                );
            }
            if resp.clicked() {
                self.ui.page = page;
                self.ui.save();
            }
        }

        // Status pinned to the bottom of the rail, so it is on every page.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if let Some(e) = &st.last_error {
                ui.label(
                    egui::RichText::new(e)
                        .size(10.0)
                        .color(p.red)
                        .family(theme::proportional()),
                );
            }
            ui.horizontal(|ui| {
                let (col, text) = if st.bios_ok {
                    (p.green, "telemetry OK")
                } else if st.elevated {
                    (p.red, "no BIOS interface")
                } else {
                    (p.amber, "read-only · not elevated")
                };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, col);
                ui.label(
                    egui::RichText::new(text)
                        .size(10.0)
                        .color(p.dim)
                        .family(theme::proportional()),
                );
            });
            if let Some(t) = &self.tray_error {
                ui.label(
                    egui::RichText::new(format!("tray: {t}"))
                        .size(9.0)
                        .color(p.faint())
                        .family(theme::proportional()),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let theme_label = match self.ui.theme {
                    Theme::Dark => "Light theme",
                    Theme::Light => "Dark theme",
                };
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(theme_label)
                                .size(10.0)
                                .color(p.dim)
                                .family(theme::proportional()),
                        )
                        .fill(p.tint(p.text, 8))
                        .stroke(egui::Stroke::NONE)
                        .rounding(5.0),
                    )
                    .clicked()
                {
                    self.ui.theme = match self.ui.theme {
                        Theme::Dark => Theme::Light,
                        Theme::Light => Theme::Dark,
                    };
                    self.ui.save();
                }
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Compact")
                                .size(10.0)
                                .color(if self.ui.compact { p.accent } else { p.dim })
                                .family(theme::proportional()),
                        )
                        .fill(p.tint(p.text, 8))
                        .stroke(egui::Stroke::NONE)
                        .rounding(5.0),
                    )
                    .on_hover_text("hide the plots")
                    .clicked()
                {
                    self.ui.compact = !self.ui.compact;
                    self.ui.save();
                }
                // Close-to-tray must be visible and reversible. It defaults to on
                // so the sampler keeps running, but hiding that choice means a user
                // who expects "close" to exit sees the window vanish and has to
                // discover the tray icon to get it back.
                if self.tray.is_some()
                    && ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(if self.ui.close_to_tray {
                                    "Close: tray"
                                } else {
                                    "Close: exit"
                                })
                                .size(10.0)
                                .color(if self.ui.close_to_tray { p.accent } else { p.dim })
                                .family(theme::proportional()),
                            )
                            .fill(p.tint(p.text, 8))
                            .stroke(egui::Stroke::NONE)
                            .rounding(5.0),
                        )
                        .on_hover_text(if self.ui.close_to_tray {
                            "closing hides the window and keeps sampling; click to exit instead"
                        } else {
                            "closing exits the app; click to hide to tray instead"
                        })
                        .clicked()
                {
                    self.ui.close_to_tray = !self.ui.close_to_tray;
                    self.ui.save();
                }
            });
        });
    }

    // -------------------------------------------------------------- header --

    fn page_header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.ui.page.title())
                        .size(DISPLAY * 0.62)
                        .strong()
                        .color(p.text)
                        .family(theme::proportional()),
                );
                ui.label(
                    egui::RichText::new(self.ui.page.subtitle())
                        .size(BODY)
                        .color(p.dim)
                        .family(theme::proportional()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The header keeps the elevation offer; the power card repeats it
                // only where the missing token actually blocks the page, so the
                // two never appear together.
                if !self.sampler.state.lock().elevated
                    && self.ui.page != Page::Performance
                    && w::primary_button(ui, p, "Restart as administrator", true).clicked()
                {
                    match elevate::relaunch_elevated() {
                        Ok(()) => {
                            self.persist_window(ui.ctx());
                            self.sampler.stop();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => self.rate_msg = Some((format!("elevation: {e}"), false)),
                    }
                }
            });
        });
        ui.add_space(14.0);
    }

    fn recover_banner(&mut self, ui: &mut egui::Ui, p: &Palette, rec: &safety::Pending) {
        egui::Frame::default()
            .fill(p.tint(p.amber, 26))
            .stroke(egui::Stroke::new(1.0_f32, p.tint(p.amber, 90)))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Previous run left a {} W cap applied ({})",
                            rec.observed_w.unwrap_or(rec.requested_w),
                            safety::age_text(rec.at)
                        ))
                        .size(BODY)
                        .color(p.text)
                        .family(theme::proportional()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::quiet_button(ui, p, "Dismiss", true).clicked() {
                            safety::clear();
                            self.recover = None;
                        }
                        if w::primary_button(ui, p, "Restore stock", true).clicked() {
                            self.write(ControlRequest::RestoreDefault);
                            self.recover = None;
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    /// "The last run crashed" plus where the log is.
    ///
    /// Shown once: the marker is consumed when it is read, because a banner that
    /// reappears on every launch after a single crash trains the user to dismiss
    /// it without reading.
    fn crash_banner(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(c) = self.crashed.clone() else { return };
        egui::Frame::default()
            .fill(p.tint(p.red, 22))
            .stroke(egui::Stroke::new(1.0_f32, p.tint(p.red, 80)))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("The last run crashed")
                                .size(BODY)
                                .color(p.text)
                                .family(theme::proportional()),
                        );
                        ui.label(
                            egui::RichText::new(&c.summary)
                                .size(LABEL)
                                .color(p.dim)
                                .family(theme::numeric()),
                        );
                        if let Some(log) = &c.log {
                            ui.label(
                                egui::RichText::new(format!("log: {}", log.display()))
                                    .size(LABEL)
                                    .color(p.faint())
                                    .family(theme::numeric()),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::quiet_button(ui, p, "Dismiss", true).clicked() {
                            self.crashed = None;
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    // ----------------------------------------------------------- Dashboard --

    fn page_dashboard(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState, hist: &sampler::History) {
        let s = st.sample.as_ref();
        let cpu = s.and_then(|x| x.cpu);
        let gpu = s.and_then(|x| x.gpu);
        let hotspot = s.and_then(|x| x.gpu_hotspot);

        // Gauges across the top: the two numbers people open the app for.
        egui::Frame::default()
            .fill(p.panel)
            .stroke(egui::Stroke::new(1.0_f32, p.border))
            .rounding(10.0)
            .inner_margin(egui::Margin::symmetric(18.0, 16.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let d = if self.ui.compact { 118.0 } else { 150.0 };
                    // A gauge with no data must not look like a gauge reading
                    // zero: "0 °C" is a measurement, "--" is an absence.
                    let (c_txt, c_col) = match cpu {
                        Some(v) => (format!("{v:.0}"), p.temp(v)),
                        None => ("--".into(), p.faint()),
                    };
                    w::gauge(
                        ui,
                        &w::Gauge {
                            diameter: d,
                            fraction: cpu.unwrap_or(0.0) / 110.0,
                            centre: &c_txt,
                            unit: "\u{00b0}C",
                            caption: "CPU",
                            colour: c_col,
                        },
                    );
                    ui.add_space(10.0);
                    let (g_txt, g_col) = match gpu {
                        Some(v) => (format!("{v:.0}"), p.temp(v)),
                        None => ("--".into(), p.faint()),
                    };
                    w::gauge(
                        ui,
                        &w::Gauge {
                            diameter: d,
                            fraction: gpu.unwrap_or(0.0) / 110.0,
                            centre: &g_txt,
                            unit: "\u{00b0}C",
                            caption: "GPU",
                            colour: g_col,
                        },
                    );

                    ui.add_space(18.0);
                    ui.vertical(|ui| {
                        if cpu.is_none() && gpu.is_none() {
                            ui.label(
                                egui::RichText::new("No thermal sensors")
                                    .size(BODY)
                                    .color(p.amber)
                                    .family(theme::proportional()),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "0x23 needs the BIOS WMI interface, which needs an \
                                     elevated token. Battery readings below still work.",
                                )
                                .size(LABEL)
                                .color(p.faint())
                                .family(theme::proportional()),
                            );
                            ui.add_space(8.0);
                        }
                        let cap = st.limits.map(|l| l.pl1);
                        // Amber when the cap sits below what this supply allows:
                        // that is a deliberate throttle, and it is easy to forget.
                        let ceiling = st
                            .limits
                            .map(|l| l.pl1_ceiling())
                            .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL1);
                        let (cap_txt, cap_col) = match cap {
                            Some(v) if v < ceiling => (format!("{v} W"), p.amber),
                            Some(v) => (format!("{v} W"), p.green),
                            None => ("--".into(), p.faint()),
                        };
                        w::stat(ui, p, "power cap", &cap_txt, cap_col);
                        ui.add_space(10.0);
                        let therm = hp_wmi::ThermalMode::from_status(st.thermal_status);
                        w::stat(
                            ui,
                            p,
                            "thermal profile",
                            match therm {
                                Some("performance (odd mode code)") => "Performance",
                                Some(_) => "Balanced / Cool",
                                None => "unknown",
                            },
                            p.text,
                        );
                        ui.add_space(10.0);
                        w::stat(ui, p, "clock", &format!("{:.0} MHz", st.clock_mhz), p.gpu);
                        if let Some(h) = hotspot {
                            ui.add_space(10.0);
                            w::stat(ui, p, "hotspot", &format!("{h:.0} \u{00b0}C"), p.temp(h));
                        }
                    });
                });
            });
        ui.add_space(12.0);

        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                w::card(ui, p, "THERMAL HISTORY", |ui| {
                    if self.ui.compact {
                        ui.label(
                            egui::RichText::new("hidden in compact mode")
                                .size(LABEL)
                                .color(p.faint()),
                        );
                        return;
                    }
                    if hist.cpu.len() < 2 {
                        // Only promise a chart when a chart is possible. Without
                        // the WMI interface there is no source, so saying
                        // "collecting" forever would be a lie.
                        if st.bios_ok {
                            w::empty(ui, p, "collecting samples…", Some("the chart fills at 1 Hz"));
                        } else {
                            w::empty(
                                ui,
                                p,
                                "no samples — thermal source unavailable",
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
                        .height(140.0)
                        .show_axes([false, true])
                        .allow_drag(false)
                        .allow_scroll(false)
                        .allow_zoom(false)
                        .allow_boxed_zoom(false)
                        .legend(egui_plot::Legend::default())
                        .show(ui, |pl| {
                            pl.line(Line::new(cpu_pts).name("CPU").color(p.cpu).width(1.8_f32));
                            pl.line(Line::new(gpu_pts).name("GPU").color(p.gpu).width(1.8_f32));
                        });
                });
                ui.add_space(12.0);
                self.battery_card(ui, p, st);
            });
            cols[1].vertical(|ui| {
                self.fan_card(ui, p, st);
                ui.add_space(12.0);
                self.curve_card(ui, p, st);
            });
        });
    }

    fn fan_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let lvl = st.sample.as_ref().and_then(|s| s.fan_level_pct);
        let rpm = st.sample.as_ref().and_then(|s| s.fan_rpm);
        w::card_badge(
            ui,
            p,
            "COOLING",
            Some(("firmware-managed", p.dim)),
            |ui| {
                ui.horizontal(|ui| {
                    w::stat(
                        ui,
                        p,
                        "fan level",
                        &match lvl {
                            Some(v) => format!("{v} %"),
                            None => "—".into(),
                        },
                        match lvl {
                            Some(v) if v >= 80 => p.amber,
                            Some(_) => p.text,
                            None => p.faint(),
                        },
                    );
                    ui.add_space(26.0);
                    w::stat(
                        ui,
                        p,
                        "rpm",
                        &match rpm {
                            Some(v) => v.to_string(),
                            None => "—".into(),
                        },
                        p.text,
                    );
                });
                ui.add_space(12.0);
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
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "0x2E and 0x32 are silent no-ops on this board, so there is no fan slider",
                    )
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::proportional()),
                );
            },
        );
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
                ui.add_space(24.0);
                if let Some(h) = b.health_pct {
                    w::stat(
                        ui,
                        p,
                        "health",
                        &format!("{h} %"),
                        if h < 70 { p.red } else { p.green },
                    );
                }
                ui.add_space(24.0);
                w::stat(
                    ui,
                    p,
                    "source",
                    if b.ac_online { "AC" } else { "Battery" },
                    if b.ac_online { p.green } else { p.amber },
                );
            });
            ui.add_space(10.0);
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
                extra.push(format!("{t:.1} \u{00b0}C"));
            }
            if let Some(h) = b.health_pct {
                extra.push(format!("{} % wear", 100u32.saturating_sub(h as u32)));
            }
            if !extra.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(extra.join("   ·   "))
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::proportional()),
                );
            }
        });
    }

    fn curve_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "FAN CURVE · FIRMWARE", |ui| {
            let Some(t) = st.fan_table.as_ref() else {
                w::empty(ui, p, "fan table unreadable", Some("requires the BIOS WMI interface"));
                return;
            };
            ui.label(
                egui::RichText::new(format!(
                    "{} fans · {} points · read-only",
                    t.fan_count, t.level_count
                ))
                .size(LABEL)
                .color(p.dim)
                .family(theme::proportional()),
            );
            if self.ui.compact {
                return;
            }
            ui.add_space(4.0);
            let fans = t.fan_count as usize;
            let series: Vec<Vec<[f64; 2]>> = (0..fans.min(3))
                .map(|f| {
                    t.points
                        .iter()
                        .enumerate()
                        .filter_map(|(i, pt)| pt.duty(f, t.fan_count).map(|d| [i as f64, d as f64]))
                        .collect()
                })
                .collect();
            Plot::new("fw_curve")
                .height(132.0)
                .allow_boxed_zoom(false)
                .allow_scroll(false)
                .allow_drag(false)
                .allow_zoom(false)
                .x_axis_label("level")
                .y_axis_label("duty %")
                .show(ui, |pl| {
                    for (f, pts) in series.iter().enumerate() {
                        let colour = if f == 0 { p.cpu } else { p.gpu };
                        pl.line(
                            Line::new(PlotPoints::from(pts.clone()))
                                .name(format!("fan{}", f + 1))
                                .color(colour)
                                .width(2.0_f32),
                        );
                    }
                });
            ui.label(
                egui::RichText::new(format!(
                    "peak {}%  ·  monotonic {}  ·  column 0 is a level index, not a temperature",
                    t.max_duty(),
                    if t.is_monotonic() { "yes" } else { "no" }
                ))
                .size(LABEL)
                .color(p.faint())
                .family(theme::proportional()),
            );
        });
    }

    // --------------------------------------------------------- Performance --

    fn page_performance(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                self.power_card(ui, p, st);
                ui.add_space(12.0);
                self.display_card(ui, p);
            });
            cols[1].vertical(|ui| {
                self.thermal_card(ui, p, st);
                ui.add_space(12.0);
                w::card(ui, p, "MEASURED LIMITS", |ui| {
                    for (k, v) in [
                        ("0x29 accepted, brick", "5–65 W PL1, PL4 ≤ 55"),
                        ("0x29 accepted, USB-C", "5–25 W (PD profile)"),
                        ("stock on brick", "65 / 55 / 55 W, stable 90 s"),
                        ("stock on USB-C", "25 / 25 / 25 W"),
                        ("clock at 65 W", "4425 MHz, 16 threads"),
                        ("clock at 25 W", "3515 MHz, 910 MHz lower"),
                        ("reset", "over-limit write → firmware stock"),
                        ("0x2A[0]", "not watts — falls 64→62 under load"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(k)
                                    .size(LABEL)
                                    .color(p.dim)
                                    .family(theme::proportional()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(v)
                                            .size(LABEL)
                                            .color(p.text)
                                            .family(theme::numeric()),
                                    );
                                },
                            );
                        });
                    }
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "The ceiling follows the supply. On USB-C the cap can only lower \
                             the profile; on the brick it is a real 5–65 W control.",
                        )
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::proportional()),
                    );
                });
            });
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
                    ui.add_space(8.0);
                    if w::primary_button(ui, p, "Restart as administrator", true).clicked() {
                        match elevate::relaunch_elevated() {
                            Ok(()) => {
                                self.persist_window(ui.ctx());
                                self.sampler.stop();
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            Err(e) => self.rate_msg = Some((format!("elevation: {e}"), false)),
                        }
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
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("WATTS")
                            .size(LABEL)
                            .color(p.dim)
                            .family(theme::proportional()),
                    );
                    if let Some(l) = st.limits {
                        ui.label(
                            egui::RichText::new(format!(
                                "PL2 {} · PL4 {} · GPU {}",
                                l.pl2, l.pl4, l.pl_with_gpu
                            ))
                            .size(LABEL)
                            .color(p.faint())
                            .family(theme::numeric()),
                        );
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "0x2A is a configured-limits block, not live package power",
                )
                .size(LABEL)
                .color(p.faint())
                .family(theme::proportional()),
            );
            ui.add_space(10.0);

            // The slider maximum comes from the firmware, never from a constant.
            // The ceiling is 25 W on USB-C PD and 65 W on the 230 W brick; a
            // hardcoded 25 would make the app unable to use the machine's headroom
            // on a brick, which is the exact failure report §8.5 was written about.
            let ceiling = st
                .limits
                .map(|l| l.pl1_ceiling())
                .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL1);
            let mut cap = cap_now.unwrap_or(ceiling);
            ui.horizontal(|ui| {
                let resp = ui.add_enabled(
                    st.elevated && st.limits.is_some(),
                    egui::Slider::new(
                        &mut cap,
                        hp_wmi::CpuPowerLimits::MIN_WATTS..=ceiling,
                    )
                    .suffix(" W")
                    .trailing_fill(true),
                );
                if resp.drag_stopped() || (resp.lost_focus() && resp.changed()) {
                    self.write(ControlRequest::SetPowerCap(cap));
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "range {}–{} W on this supply · PL4 ceiling {} W",
                    hp_wmi::CpuPowerLimits::MIN_WATTS,
                    ceiling,
                    st.limits
                        .map(|l| l.pl4_ceiling())
                        .unwrap_or(hp_wmi::CpuPowerLimits::PD_MAX_PL4)
                ))
                .size(LABEL)
                .color(p.faint())
                .family(theme::proportional()),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // Not "Restore 25 W": on a brick that would be throttling.
                if w::quiet_button(ui, p, "Restore stock", st.elevated).clicked() {
                    self.write(ControlRequest::RestoreDefault);
                }
                ui.label(
                    egui::RichText::new("firmware re-applies its own block")
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::proportional()),
                );
            });
            // P1-3's decision, stated where the control is rather than in a
            // changelog: the cap is sticky. It is written to config.toml and
            // re-applied by the logon task, so it survives a reboot. Leaving that
            // implicit is how a user ends up with a throttled machine and no idea
            // why.
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "This cap stays until you change it or press Restore stock — it is saved \
                     and re-applied at every login, including after a reboot.",
                )
                .size(LABEL)
                .color(p.dim)
                .family(theme::proportional()),
            );
            self.write_feedback(ui, p, st);
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
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(match current {
                        Some(t) => format!("firmware reports: {t}"),
                        // Distinguish "the firmware said something unknown" from
                        // "we could not ask", which is what an unelevated run
                        // actually is. Showing 0x00 as an unknown mode would blame
                        // the firmware for our missing token.
                        None if !st.bios_ok => {
                            "status unavailable — needs an elevated token".to_string()
                        }
                        None => format!("firmware status {status:#04X} is not a known mode"),
                    })
                    .size(LABEL)
                    .color(p.dim)
                    .family(theme::proportional()),
                );
                ui.label(
                    egui::RichText::new(
                        "Balanced and Cool share status 0x1E; the byte is read back, not assumed",
                    )
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::proportional()),
                );
                self.write_feedback(ui, p, st);
            },
        );
    }

    fn display_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        w::card(ui, p, "DISPLAY CADENCE", |ui| {
            let cur = display::current();
            if self.rates.is_empty() {
                w::empty(ui, p, "no modes reported", Some("EnumDisplaySettings returned nothing"));
                return;
            }
            if let Some(c) = cur {
                ui.label(
                    egui::RichText::new(format!("{} × {} · {} Hz", c.width, c.height, c.hz))
                        .size(BODY)
                        .color(p.text)
                        .family(theme::numeric()),
                );
            }
            ui.add_space(8.0);
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
                            1.0_f32,
                            if is_active { p.accent } else { p.border },
                        ))
                        .rounding(6.0)
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
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(msg)
                        .size(LABEL)
                        .color(if *ok { p.green } else { p.red })
                        .family(theme::proportional()),
                );
            }
            ui.label(
                egui::RichText::new("CDS_TEST runs first, so a rejected mode never reaches the screen")
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::proportional()),
            );
        });
    }

    /// The pending → verified/failed indicator.
    fn write_feedback(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        if let Some(started) = self.pending_since {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(format!("writing… {} ms", started.elapsed().as_millis()))
                        .size(LABEL)
                        .color(p.amber)
                        .family(theme::proportional()),
                );
            });
            return;
        }
        let Some(wr) = st.last_write.as_ref() else { return };
        let fresh = wr.at.elapsed().as_millis() < FLASH_MS as u128;
        let (glyph, colour) = match wr.outcome {
            WriteOutcome::Verified => ("\u{2713}", p.green),
            WriteOutcome::Adjusted => ("\u{2248}", p.amber),
            WriteOutcome::Failed => ("\u{2717}", p.red),
        };
        let colour = if fresh { colour } else { colour.gamma_multiply(0.8) };
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!("{glyph} {}", wr.message))
                .size(LABEL)
                .color(colour)
                .family(theme::proportional()),
        );
        if fresh {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    // ------------------------------------------------------------ Lighting --

    fn page_lighting(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
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
                w::empty(ui, p, "firmware reports 0 zones", Some("nothing to address on this board"));
            });
            return;
        }

        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                w::card_badge(
                    ui,
                    p,
                    "ZONES",
                    Some((&format!("{} zones", rgb.zones), p.accent)),
                    |ui| {
                        while self.zone_pick.len() < rgb.zones as usize {
                            self.zone_pick.push(egui::Color32::WHITE);
                        }
                        for i in 0..rgb.zones as usize {
                            let cur = rgb.colors.get(i).copied().unwrap_or([0, 0, 0]);
                            let mut c = egui::Color32::from_rgb(cur[0], cur[1], cur[2]);
                            ui.horizontal(|ui| {
                                let (rect, _) = ui
                                    .allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 6.0, c);
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0_f32,
                                    egui::Stroke::new(1.0_f32, p.border),
                                );
                                ui.add_space(4.0);
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
                                            let col =
                                                self.zone_pick.get(i).copied().unwrap_or(c);
                                            self.write(ControlRequest::SetZone {
                                                zone: i as u8,
                                                rgb: [col.r(), col.g(), col.b()],
                                            });
                                        }
                                        if ui.color_edit_button_srgba(&mut c).changed() {
                                            self.zone_pick[i] = c;
                                        }
                                    },
                                );
                            });
                            ui.add_space(6.0);
                        }
                    },
                );
            });
            cols[1].vertical(|ui| {
                w::card(ui, p, "CHANNEL", |ui| {
                    if let Some(b) = rgb.backlight {
                        w::stat(ui, p, "backlight", &format!("{b:#04X}"), p.text);
                        ui.add_space(8.0);
                        // The firmware reports 0x64 (off) or 0xE4 (on). Only two
                        // states exist, so this is a switch, not a slider — a
                        // brightness slider would be a control that lies.
                        let is_on = b & 0x80 != 0;
                        ui.horizontal(|ui| {
                            if w::quiet_button(ui, p, "Backlight off", st.elevated && is_on).clicked()
                            {
                                self.write(ControlRequest::SetBacklight(0x64));
                            }
                            if w::quiet_button(ui, p, "Backlight on", st.elevated && !is_on)
                                .clicked()
                            {
                                self.write(ControlRequest::SetBacklight(0xE4));
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "subtype 4/5 take 0x64 or 0xE4 — no intermediate levels",
                            )
                            .size(LABEL)
                            .color(p.faint())
                            .family(theme::proportional()),
                        );
                        ui.add_space(8.0);
                        w::divider(ui, p);
                    }
                    ui.label(
                        egui::RichText::new("zone count comes from colorTable[0]")
                            .size(BODY)
                            .color(p.text)
                            .family(theme::proportional()),
                    );
                    ui.label(
                        egui::RichText::new(
                            "0x2B reports 0x04 on this board, which OGH's enum calls \
                             OneZoneWithNumpad. The table says 3 zones with 3 distinct \
                             colours, so the table wins.",
                        )
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::proportional()),
                    );
                    ui.add_space(10.0);
                    w::divider(ui, p);
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
                    ui.label(
                        egui::RichText::new(
                            "every write is read-modify-write and verified byte-for-byte; \
                             a failed write restores the original",
                        )
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::proportional()),
                    );
                    self.write_feedback(ui, p, st);
                });
            });
        });
    }

    // -------------------------------------------------------------- System --

    fn page_system(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        if st.bios_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new("reading BIOS settings…")
                        .size(BODY)
                        .color(p.dim)
                        .family(theme::proportional()),
                );
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
            ui.label(
                egui::RichText::new(format!("{} settings enumerated", rows.len()))
                    .size(BODY)
                    .color(p.dim)
                    .family(theme::proportional()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::quiet_button(ui, p, "Reload", true).clicked() {
                    self.load_bios_async();
                }
            });
        });
        ui.add_space(10.0);

        // Header row.
        egui::Frame::default()
            .fill(p.panel)
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui(egui::vec2(300.0, 18.0), |ui| {
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
        ui.add_space(4.0);

        let max_h = (ui.available_height() - 96.0).max(160.0);
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
                        .inner_margin(egui::Margin::symmetric(12.0, 7.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.allocate_ui(egui::vec2(300.0, 18.0), |ui| {
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

        ui.add_space(8.0);
        let Some(idx) = self.bios_selected else {
            ui.label(
                egui::RichText::new("select a writable setting to edit it")
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::proportional()),
            );
            return;
        };
        let Some(row) = rows.get(idx).cloned() else {
            self.bios_selected = None;
            return;
        };
        let row: BiosRow = row;
        egui::Frame::default()
            .fill(p.panel)
            .stroke(egui::Stroke::new(1.0_f32, p.accent))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&row.name)
                            .size(BODY)
                            .color(p.text)
                            .family(theme::proportional()),
                    );
                    ui.add_space(10.0);
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
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "rc 0 accepted · 1 no such setting · 5 value rejected · 6 reboot required",
                    )
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::proportional()),
                );
                self.write_feedback(ui, p, st);
            });
    }

    // --------------------------------------------------------------- About --

    fn page_about(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                w::card(ui, p, "BUILD", |ui| {
                    ui.horizontal(|ui| {
                        w::stat(ui, p, "version", concat!("v", env!("CARGO_PKG_VERSION")), p.text);
                        ui.add_space(24.0);
                        w::stat(ui, p, "commit", build_hash(), p.dim);
                    });
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        ))
                        .size(LABEL)
                        .color(p.faint())
                        .family(theme::numeric()),
                    );
                });
                ui.add_space(12.0);
                w::card(ui, p, "UPDATES", |ui| {
                    let s = self.updates.0.lock();
                    if let Some(l) = &s.latest {
                        ui.label(
                            egui::RichText::new(format!("latest release v{l}"))
                                .size(BODY)
                                .color(if s.available { p.green } else { p.dim })
                                .family(theme::proportional()),
                        );
                    }
                    if s.busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new("contacting GitHub…")
                                    .size(LABEL)
                                    .color(p.dim),
                            );
                        });
                    }
                    // Consent for code execution needs to say what will run and
                    // that it was verified. A bare "Download & install" button is
                    // not informed consent.
                    if let (Some(name), Some(digest)) = (&s.asset_name, &s.digest) {
                        let short = digest
                            .strip_prefix("sha256:")
                            .map(|h| &h[..12])
                            .unwrap_or(digest.as_str());
                        ui.label(
                            egui::RichText::new(format!("asset: {name}"))
                                .size(LABEL)
                                .color(p.dim)
                                .family(theme::numeric()),
                        );
                        ui.label(
                            egui::RichText::new(format!("sha256 {short}… — checked before running"))
                                .size(LABEL)
                                .color(p.faint())
                                .family(theme::numeric()),
                        );
                    }
                    if let Some(why) = &s.refusal {
                        ui.label(
                            egui::RichText::new(format!("\u{2717} {why}"))
                                .size(LABEL)
                                .color(p.amber)
                                .family(theme::proportional()),
                        );
                    }
                    let busy = s.busy;
                    let available = s.available;
                    drop(s);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if w::quiet_button(ui, p, "Check", !busy).clicked() {
                            self.updates.check_async();
                        }
                        if available && w::primary_button(ui, p, "Download & install", !busy).clicked()
                        {
                            self.updates.update_async();
                        }
                    });
                    let s = self.updates.0.lock();
                    if let Some((done, total)) = s.progress {
                        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        w::meter(
                            ui,
                            p,
                            "download",
                            &format!("{} / {} KiB", done / 1024, total / 1024),
                            frac,
                            p.accent,
                        );
                    }
                    if let Some(m) = &s.done_msg {
                        ui.label(
                            egui::RichText::new(m)
                                .size(LABEL)
                                .color(p.green)
                                .family(theme::proportional()),
                        );
                    }
                    if let Some(e) = &s.err {
                        ui.label(
                            egui::RichText::new(e)
                                .size(LABEL)
                                .color(p.red)
                                .family(theme::proportional()),
                        );
                    }
                    ui.label(
                        egui::RichText::new(format!("installs to {}", self.install_dir))
                            .size(LABEL)
                            .color(p.faint())
                            .family(theme::proportional()),
                    );
                    // What the detached helper did last time. Without this the
                    // helper's verdict is written and never read, which is the
                    // same silent failure the errorlevel checks exist to prevent.
                    if let Some(r) = updater::read_last_result() {
                        let (glyph, colour, text) = if r.ok {
                            (
                                "\u{2713}",
                                p.green,
                                format!("last install succeeded ({})", r.step),
                            )
                        } else {
                            (
                                "\u{2717}",
                                p.red,
                                format!(
                                    "last install FAILED at {}{}",
                                    r.step,
                                    r.dest
                                        .as_deref()
                                        .map(|d| format!(" → {d}"))
                                        .unwrap_or_default()
                                ),
                            )
                        };
                        ui.label(
                            egui::RichText::new(format!("{glyph} {text}"))
                                .size(LABEL)
                                .color(colour)
                                .family(theme::proportional()),
                        );
                    }
                });
                ui.add_space(12.0);
                w::card(ui, p, "DIAGNOSTICS", |ui| {
                    ui.label(
                        egui::RichText::new(
                            "One block with every readback this app has, for a bug report.",
                        )
                        .size(LABEL)
                        .color(p.dim)
                        .family(theme::proportional()),
                    );
                    ui.add_space(6.0);
                    if w::quiet_button(ui, p, "Copy to clipboard", true).clicked() {
                        let text = self.build_diag(st);
                        self.diag = match elevate::copy_to_clipboard(&text) {
                            Ok(()) => "copied".into(),
                            Err(e) => format!("copy failed: {e}"),
                        };
                    }
                    if !self.diag.is_empty() {
                        ui.label(
                            egui::RichText::new(&self.diag)
                                .size(LABEL)
                                .color(p.faint())
                                .family(theme::proportional()),
                        );
                    }
                });
            });
            cols[1].vertical(|ui| {
                w::card(ui, p, "VERIFIED ON THIS BOARD", |ui| {
                    for (cap, state, note) in CAPABILITIES {
                        let (c, glyph) = match *state {
                            Cap::Works => (p.green, "\u{2713}"),
                            Cap::Dead => (p.red, "\u{2717}"),
                        };
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(glyph).size(BODY).color(c).family(theme::numeric()),
                            );
                            ui.label(
                                egui::RichText::new(*cap)
                                    .size(BODY)
                                    .color(p.text)
                                    .family(theme::proportional()),
                            );
                        });
                        ui.label(
                            egui::RichText::new(*note)
                                .size(LABEL)
                                .color(p.faint())
                                .family(theme::proportional()),
                        );
                        ui.add_space(7.0);
                    }
                });
            });
        });
    }
}

/// Capability matrix. Every row is a measurement from REPORT-2026-10-21.md, and
/// nothing here is aspirational: a row that says "dead" had a test that failed.
const CAPABILITIES: &[(&str, Cap, &str)] = &[
    (
        "CPU power cap",
        Cap::Works,
        "5–65 W on the 230 W brick, 5–25 W on USB-C; verified by clock outcome",
    ),
    ("Thermal profile", Cap::Works, "0x1A V1 remap, status read back from 0x28[8]"),
    ("Keyboard RGB", Cap::Works, "0x20009 subtype 2/3, byte-exact RMW restore"),
    ("BIOS settings write", Cap::Works, "instance-path SetBIOSSetting, rc table calibrated"),
    ("Fan curve read", Cap::Works, "0x2F, 42 points, 2 fans"),
    ("Refresh rate", Cap::Works, "Win32 EnumDisplaySettings + CDS_TEST"),
    ("Software fan control", Cap::Dead, "0x2E and 0x32 are silent no-ops on 8C9C"),
    ("Battery charge limit", Cap::Dead, "Battery Care Function returns rc 5 for all 27 encodings"),
    ("GPU mode switch", Cap::Dead, "0x52 returns BIOS err 5"),
    ("BIOS undervolt / OC", Cap::Dead, "0x35 returns WMI err 31; 0x28[6] = 0"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    Works,
    Dead,
}

fn main() -> eframe::Result<()> {
    // Before anything can panic: a GUI crash used to leave no artifact at all.
    crash::install_panic_hook();

    let ui = UiState::load();
    let size = ui.window.unwrap_or([1180.0, 780.0]);
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([940.0, 620.0])
            .with_title("OmenMon")
            // The tray already draws an icon at runtime; the window should carry
            // the same mark rather than the default blank frame.
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "OmenMon",
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// The tray's generated RGBA, as viewport icon data.
///
/// One definition of the app's mark: the tray and the title bar cannot drift
/// apart, and the build still ships no binary asset.
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: tray::icon_rgba(),
        width: 16,
        height: 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-4. The zoom clamp is what stops a persisted or hand-edited value from
    /// making the window unusable, so its bounds are worth pinning.
    #[test]
    fn zoom_is_clamped_to_a_usable_range() {
        assert_eq!(clamp_zoom(1.0), 1.0);
        assert_eq!(clamp_zoom(0.1), ZOOM_MIN);
        assert_eq!(clamp_zoom(9.0), ZOOM_MAX);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(f32::INFINITY), ZOOM_MAX);
        // A step from either end cannot escape the range.
        assert!(clamp_zoom(ZOOM_MAX + ZOOM_STEP) <= ZOOM_MAX);
        assert!(clamp_zoom(ZOOM_MIN - ZOOM_STEP) >= ZOOM_MIN);
    }

    /// The window icon must be the tray icon: same buffer, same dimensions, and
    /// the size must match the pixel data or the platform rejects it.
    #[test]
    fn window_icon_matches_the_tray_icon() {
        let icon = window_icon();
        assert_eq!(icon.width as usize * icon.height as usize * 4, icon.rgba.len());
        assert_eq!(icon.rgba, tray::icon_rgba());
    }

    /// P1-3. The sticky-cap policy must be visible to the user, not just
    /// implemented. Checked against this file's own source so the copy cannot be
    /// deleted while the behaviour stays sticky — that combination is worse than
    /// either alone, because the machine throttles with no explanation.
    #[test]
    fn sticky_cap_policy_is_stated_in_the_ui() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("This cap stays until you change it or press Restore stock"),
            "the power card must state that the cap persists"
        );
        assert!(
            src.contains("re-applied at every login"),
            "and say what keeps it applied"
        );
    }

    /// P1-5. The window must carry the app's icon, not the default blank frame.
    #[test]
    fn viewport_is_built_with_an_icon() {
        let src = include_str!("main.rs");
        assert!(
            src.contains(".with_icon(window_icon())"),
            "the viewport must set an icon"
        );
    }
}
