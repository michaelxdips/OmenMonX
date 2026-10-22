//! The shell: the fixed rail, the page header, and the banners above the page.
//!
//! The rail carries identity, navigation and machine status, so the machine's
//! state is visible from every page without a separate status bar. It is a
//! `SidePanel`, not a widget in the central area, because it must not scroll.

use eframe::egui;

use crate::sampler::SharedState;
use crate::theme::{self, radius, space, Page, Palette, Theme, BODY, LABEL, RAIL_W};
use crate::ui::components as c;
use crate::{elevate, safety, widgets as w, App};

/// Width the rail reserves. Re-exported so `update()` does not need a second
/// import of the token.
pub const RAIL_WIDTH: f32 = RAIL_W;

/// The rail's own padding: tighter than a page, because a 212 px column cannot
/// spend 16 px on each side and still fit "Performance".
const RAIL_PAD_X: f32 = space::MD;
const RAIL_PAD_Y: f32 = space::LG;

impl App {
    pub fn rail(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        self.brand_block(ui, p);
        ui.add_space(space::LG);
        self.navigation(ui, p);
        self.rail_status(ui, p, st);
    }

    fn brand_block(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, radius::CONTROL, p.accent);
            // A stylised "O" cut from the accent block. Drawn, not fonted: a glyph
            // from a font that may not exist renders as tofu.
            ui.painter().circle_stroke(
                rect.center(),
                7.5,
                egui::Stroke::new(2.6_f32, p.shell),
            );
            ui.add_space(space::XS);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("OMENMON")
                        .size(BODY + 1.5)
                        .strong()
                        .color(p.text)
                        .family(theme::proportional()),
                );
                ui.label(
                    egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                        .size(LABEL - 1.0)
                        .color(p.faint())
                        .family(theme::numeric()),
                );
            });
        });
    }

    fn navigation(&mut self, ui: &mut egui::Ui, p: &Palette) {
        for page in Page::ALL {
            let active = self.ui.page == page;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 34.0),
                egui::Sense::click(),
            );
            if ui.is_rect_visible(rect) {
                let hovered = resp.hovered();
                if active {
                    ui.painter().rect_filled(rect, radius::CONTROL, p.accent_soft);
                    // A 3 px accent bar on the leading edge marks the active page
                    // even when the fill is hard to distinguish at a glance.
                    let bar = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 6.0),
                        egui::vec2(3.0, rect.height() - 12.0),
                    );
                    ui.painter().rect_filled(bar, 1.5, p.accent);
                } else if hovered {
                    ui.painter().rect_filled(rect, radius::CONTROL, p.tint(p.text, 12));
                }
                let icon_col = if active { p.accent } else { p.dim };
                let icon_rect = egui::Rect::from_min_size(
                    rect.min + egui::vec2(space::MD, 8.0),
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
    }

    /// Status pinned to the bottom of the rail, so it is on every page.
    ///
    /// Bottom-up layout, so the first thing added is the lowest. Each control is
    /// a real toggle with a persisted value: a control that does nothing is on
    /// the design rules.
    fn rail_status(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if let Some(e) = &st.last_error {
                ui.label(
                    egui::RichText::new(e)
                        .size(LABEL - 1.0)
                        .color(p.red)
                        .family(theme::proportional()),
                );
            }
            self.health_line(ui, p, st);
            if let Some(t) = &self.tray_error {
                ui.label(
                    egui::RichText::new(format!("tray: {t}"))
                        .size(LABEL - 2.0)
                        .color(p.faint())
                        .family(theme::proportional()),
                );
            }
            ui.add_space(space::SM);
            self.view_toggles(ui, p);
        });
    }

    /// The one-line health indicator. Colour plus a word, never colour alone.
    fn health_line(&self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        let (col, text) = if st.bios_ok {
            (p.green, "telemetry OK")
        } else if st.elevated {
            (p.red, "no BIOS interface")
        } else {
            (p.amber, "read-only \u{00b7} not elevated")
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, col);
            ui.label(
                egui::RichText::new(text)
                    .size(LABEL - 1.0)
                    .color(p.dim)
                    .family(theme::proportional()),
            );
        });
    }

    /// Theme, compact and close-behaviour, as small chips.
    fn view_toggles(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let theme_label = match self.ui.theme {
                Theme::Dark => "Light theme",
                Theme::Light => "Dark theme",
            };
            if rail_chip(ui, p, theme_label, false).clicked() {
                self.ui.theme = match self.ui.theme {
                    Theme::Dark => Theme::Light,
                    Theme::Light => Theme::Dark,
                };
                self.ui.save();
            }
            if rail_chip(ui, p, "Compact", self.ui.compact)
                .on_hover_text("hide the plots")
                .clicked()
            {
                self.ui.compact = !self.ui.compact;
                self.ui.save();
            }
            // Close-to-tray must be visible and reversible. It defaults to on so
            // the sampler keeps running, but hiding that choice means a user who
            // expects "close" to exit sees the window vanish and has to discover
            // the tray icon to get it back.
            if self.tray.is_some()
                && rail_chip(
                    ui,
                    p,
                    if self.ui.close_to_tray { "Close: tray" } else { "Close: exit" },
                    self.ui.close_to_tray,
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
    }

    /// Page title, subtitle, and the elevation offer.
    pub fn page_header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(self.ui.page.title())
                        .size(theme::TITLE)
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
                    self.relaunch_elevated(ui.ctx());
                }
            });
        });
        ui.add_space(space::LG);
    }

    /// Restart elevated, or report why not. Shared by the header and the power
    /// card so both paths save state and stop the sampler the same way.
    pub fn relaunch_elevated(&mut self, ctx: &egui::Context) {
        match elevate::relaunch_elevated() {
            Ok(()) => {
                self.persist_window(ctx);
                self.sampler.stop();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.rate_msg = Some((format!("elevation: {e}"), false)),
        }
    }

    /// "A previous run left a cap applied", with the two ways out.
    pub fn recover_banner(&mut self, ui: &mut egui::Ui, p: &Palette, rec: &safety::Pending) {
        c::banner(ui, p, p.amber, |ui| {
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
                    self.write(crate::sampler::ControlRequest::RestoreDefault);
                    self.recover = None;
                }
            });
        });
    }

    /// "The last run crashed" plus where the log is.
    ///
    /// Shown once: the marker is consumed when it is read, because a banner that
    /// reappears on every launch after a single crash trains the user to dismiss
    /// it without reading.
    pub fn crash_banner(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(c) = self.crashed.clone() else { return };
        c::banner(ui, p, p.red, |ui| {
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::quiet_button(ui, p, "Dismiss", true).clicked() {
                    self.crashed = None;
                }
            });
        });
    }
}

/// A small chip button for the rail's view toggles.
///
/// Flat fill, no border: these are preferences, not actions, and a bordered
/// button at the bottom of a 212 px rail competes with the navigation above it.
fn rail_chip(ui: &mut egui::Ui, p: &Palette, text: &str, on: bool) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(LABEL - 1.0)
                .color(if on { p.accent } else { p.dim })
                .family(theme::proportional()),
        )
        .fill(p.tint(p.text, 8))
        .stroke(egui::Stroke::NONE)
        .rounding(radius::CONTROL)
        .min_size(egui::vec2(0.0, 20.0)),
    )
}

/// The rail frame. Kept here so `update()` reads as layout, not styling.
pub fn rail_frame(p: &Palette) -> egui::Frame {
    egui::Frame::default()
        .fill(p.shell)
        .inner_margin(egui::Margin::symmetric(RAIL_PAD_X, RAIL_PAD_Y))
}

/// The page frame: the surface every card sits on.
pub fn page_frame(p: &Palette) -> egui::Frame {
    egui::Frame::default()
        .fill(p.bg)
        .inner_margin(egui::Margin::symmetric(space::XL, space::LG))
}
