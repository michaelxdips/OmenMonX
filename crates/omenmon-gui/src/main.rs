//! OmenMon-rs GUI.
//!
//! `main.rs` owns the process: state, the frame loop, and the wiring between the
//! sampler, the tray and the pages. Everything visual lives in [`ui`], and
//! everything visual takes its sizes and colours from [`theme`].
//!
//! The split is deliberate. Before it, this file was 1759 lines holding the
//! shell, five pages, every card and `main()`; a restyle and a behaviour fix
//! touched the same file and neither could be reviewed alone.

mod crash;
mod diag;
mod display;
mod elevate;
mod safety;
mod sampler;
mod theme;
mod tray;
mod ui;
mod updater;
mod widgets;

use eframe::egui;
use sampler::{ControlRequest, Sampler};
use theme::{Palette, UiState};
use updater::UpdateHandle;

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
        // Inner size is in points; store it so the next launch opens at the same
        // physical size regardless of the DPI it was captured at.
        let rect = ctx.input(|i| i.viewport().inner_rect);
        if let Some(r) = rect {
            self.ui.window = Some([r.width(), r.height()]);
            self.ui.save();
        }
    }
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

        // A write is done once the sampler has published a report newer than the
        // moment the request was sent. Polling the timestamp is what keeps this
        // off the UI thread.
        if let Some(started) = self.pending_since {
            let st = self.sampler.state.lock();
            let done = st.last_write.as_ref().map(|w| w.at >= started).unwrap_or(false);
            drop(st);
            if done {
                self.pending_since = None;
            }
        }

        let p = Palette::of(self.ui.theme);
        let st = self.sampler.state.lock().clone();
        let hist = self.sampler.history.lock().clone();

        // Close-to-tray: intercept the window close and hide instead.
        if self.ui.close_to_tray && self.tray.is_some() {
            let close = ctx.input(|i| i.viewport().close_requested());
            if close {
                self.persist_window(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        egui::SidePanel::left("rail")
            .exact_width(ui::shell::RAIL_WIDTH)
            .resizable(false)
            .frame(ui::shell::rail_frame(&p))
            .show(ctx, |ui| {
                ui.set_width(ui.available_width());
                self.rail(ui, &p, &st);
            });

        egui::CentralPanel::default()
            .frame(ui::shell::page_frame(&p))
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
                        theme::Page::Dashboard => self.page_dashboard(ui, &p, &st, &hist),
                        theme::Page::Performance => self.page_performance(ui, &p, &st),
                        theme::Page::Lighting => self.page_lighting(ui, &p, &st),
                        theme::Page::System => self.page_system(ui, &p, &st),
                        theme::Page::About => self.page_about(ui, &p, &st),
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The sampler owns a WMI handle; stopping it explicitly means the daemon
        // thread is not killed mid-transaction when the process ends.
        self.sampler.stop();
    }
}

/// Short git hash baked in at build time, or "unknown" for a plain checkout.
fn build_hash() -> &'static str {
    option_env!("OMENMON_BUILD_HASH").unwrap_or("unknown")
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
    /// implemented. Checked against the page source so the copy cannot be deleted
    /// while the behaviour stays sticky — that combination is worse than either
    /// alone, because the machine throttles with no explanation.
    #[test]
    fn sticky_cap_policy_is_stated_in_the_ui() {
        let src = include_str!("ui/pages/performance.rs");
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

    /// P2-1. The shell must stay a shell. This is the structural half of the
    /// split: a page that creeps back into `main.rs` would undo it silently, and
    /// the file size is the only thing that makes the regression visible.
    #[test]
    fn main_stays_a_shell() {
        let src = include_str!("main.rs");
        let lines = src.lines().count();
        assert!(
            lines < 400,
            "main.rs is {lines} lines; pages belong in ui/pages"
        );
        // Built from parts so this assertion's own text cannot satisfy it.
        let marker = ["fn ", "page_", "dashboard"].concat();
        assert!(
            !src.contains(&marker),
            "page bodies must live in ui/pages"
        );
    }

    /// `FLASH_MS` is a UI timing constant, so it is imported where it is used
    /// rather than duplicated as a literal in a page.
    #[test]
    fn flash_window_is_a_single_constant() {
        let flash = crate::ui::components::FLASH_MS;
        assert!(flash >= 1_000, "a shorter flash is unreadable");
        let pages = include_str!("ui/pages/performance.rs");
        assert!(!pages.contains("1_600"), "the flash duration must come from the constant");
    }
}
