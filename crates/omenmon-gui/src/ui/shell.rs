//! The shell: the fixed rail, the page header, and the banners above the page.
//!
//! The rail carries identity, navigation and machine status, so the machine's
//! state is visible from every page without a separate status bar. It is a
//! `SidePanel`, not a widget in the central area, because it must not scroll.

use eframe::egui;

use crate::sampler::SharedState;
use crate::theme::{self, radius, space, Page, Palette, BODY, HERO, LABEL, RAIL_W};
use crate::ui::components as c;
use crate::{elevate, safety, widgets as w, App};

/// Width the rail reserves. Re-exported so `update()` does not need a second
/// import of the token.
pub const RAIL_WIDTH: f32 = RAIL_W;

/// The rail's own padding: icon rail, minimal chrome.
const RAIL_PAD_X: f32 = space::SM;
const RAIL_PAD_Y: f32 = space::MD;

impl App {
    pub fn rail(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        self.brand_mark(ui, p);
        ui.add_space(space::MD);
        self.navigation(ui, p);
        ui.add_space(space::MD);
        self.health_dot(ui, p, st);
    }

    /// Accent block only. Wordmark moves to the header; the rail stays icons.
    fn brand_mark(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, radius::CONTROL, p.accent);
            ui.painter().circle_stroke(
                rect.center(),
                8.0,
                egui::Stroke::new(2.6_f32, p.shell),
            );
        });
    }

    fn navigation(&mut self, ui: &mut egui::Ui, p: &Palette) {
        for page in Page::ALL {
            let active = self.ui.page == page;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 44.0),
                egui::Sense::click(),
            );
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, active, true, page.title())
            });
            if ui.is_rect_visible(rect) {
                let hovered = resp.hovered();
                if active {
                    ui.painter().rect_filled(rect, radius::CONTROL, p.accent_soft);
                    let bar = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 8.0),
                        egui::vec2(3.0, rect.height() - 16.0),
                    );
                    ui.painter().rect_filled(bar, 1.5, p.accent);
                } else if hovered {
                    ui.painter().rect_filled(rect, radius::CONTROL, p.tint(p.text, 12));
                }
                if resp.has_focus() {
                    let inset = egui::vec2(2.0, 2.0);
                    ui.painter().rect_stroke(
                        rect.shrink2(inset),
                        radius::CONTROL,
                        egui::Stroke::new(2.0_f32, p.accent),
                    );
                }
                // The label stays in the text colour even when active: accent on
                // accent_soft is 4.08:1, below AA. The leading bar + fill carry
                // the "active" state; the word must stay readable.
                let icon_col = if active { p.text } else { p.dim };
                let label = page.short();
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::new(BODY, theme::proportional()),
                    icon_col,
                );
            }
            let resp = resp.on_hover_text(page.title());
            if resp.clicked() {
                self.ui.page = page;
                self.ui.save();
            }
        }
    }

    /// One dot + one word, pinned to the rail bottom. Colour never stands alone.
    fn health_dot(&self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            // `bottom_up` is a vertical layout: without this inner horizontal
            // the dot stacks above the word instead of beside it.
            ui.horizontal(|ui| {
                let (col, text) = if st.bios_ok {
                    (p.green, "OK")
                } else if st.elevated {
                    // "NO BIOS" clipped the 68 px rail; "BIOS" + red reads the same.
                    (p.red, "BIOS")
                } else {
                    (p.amber, "RO")
                };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, col);
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(text)
                        .size(LABEL)
                        .color(p.dim)
                        .family(theme::proportional()),
                );
            });
        });
    }

    /// Page title, subtitle, and the elevation offer.
    pub fn page_header(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(self.ui.page.title())
                    .size(HERO)
                    .strong()
                    .color(p.text)
                    .family(theme::proportional()),
            );
            // Not glued to the 32 pt title: one micro gap, and the label sits on
            // the title's baseline so it does not read as floating.
            ui.add_space(space::SM);
            ui.label(
                egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(LABEL)
                    .color(p.faint())
                    .family(theme::numeric()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !self.sampler.state.lock().elevated
                    && w::primary_button(ui, p, "Restart as administrator", true).clicked()
                {
                    self.relaunch_elevated(ui.ctx());
                }
            });
        });
        self.global_write_status(ui, p);
        ui.add_space(space::MD);
    }

    /// One global write status. Per-card spinners are gone; the header owns it.
    ///
    /// The row is a fixed-height allocation: an in-flow label that appears and
    /// disappears made the header jump ~20 px when a write started or expired.
    fn global_write_status(&self, ui: &mut egui::Ui, p: &Palette) {
        use crate::sampler::WriteOutcome as WO;
        let w = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::hover());
        let paint = |ui: &mut egui::Ui, pos: egui::Pos2| {
            if let Some(started) = self.pending_since {
                ui.painter().text(
                    pos,
                    egui::Align2::LEFT_CENTER,
                    format!("writing… {} ms", started.elapsed().as_millis()),
                    egui::FontId::new(LABEL, theme::numeric()),
                    p.amber,
                );
                return;
            }
            let st = self.sampler.state.lock();
            let Some(wr) = st.last_write.as_ref() else { return };
            let fresh = wr.at.elapsed().as_millis() < crate::ui::components::FLASH_MS as u128;
            let (mark, word, colour) = match wr.outcome {
                WO::Verified => ("✓", "verified", p.green),
                WO::Adjusted => ("≈", "adjusted", p.amber),
                WO::Failed => ("✗", "refused", p.red),
            };
            let colour = if fresh { colour } else { colour.gamma_multiply(0.8) };
            ui.painter().text(
                pos,
                egui::Align2::LEFT_CENTER,
                format!("{mark} {word} · {}", wr.message),
                egui::FontId::new(LABEL, theme::proportional()),
                colour,
            );
        };
        paint(ui, rect.left_center());
        if self.pending_since.is_some()
            || self
                .sampler
                .state
                .lock()
                .last_write
                .as_ref()
                .map(|wr| wr.at.elapsed().as_millis() < crate::ui::components::FLASH_MS as u128)
                .unwrap_or(false)
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    /// Restart elevated, or report why not. Shared by the header and the power
    /// card so both paths save state and stop the sampler the same way.
    pub fn relaunch_elevated(&mut self, ctx: &egui::Context) {
        match elevate::relaunch_elevated() {
            Ok(()) => {
                self.persist_window(ctx);
                self.sampler.stop();
                // The replacement process is starting; this close must actually
                // quit, not hide to tray, or the tray holds a zombie while the
                // elevated copy runs. `quitting` is the flag `should_hide_on_close`
                // checks.
                self.quitting = true;
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
                // right_to_left puts the FIRST child rightmost; the primary
                // action leads the call so it lands trailing (standard order).
                if w::primary_button(ui, p, "Restore stock", true).clicked() {
                    self.write(crate::sampler::ControlRequest::RestoreDefault);
                    self.recover = None;
                }
                if w::quiet_button(ui, p, "Dismiss", true).clicked() {
                    safety::clear();
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
        .inner_margin(egui::Margin::symmetric(space::LG, space::MD))
}
