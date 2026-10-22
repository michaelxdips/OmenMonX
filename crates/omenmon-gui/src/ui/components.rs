//! Shared UI pieces that more than one page uses.
//!
//! Everything here takes colours from [`Palette`] and sizes from the type and
//! spacing scales. There is deliberately no styling in this file that a page
//! could have written itself — if two pages need it, it lives here; if one page
//! needs it, it lives in that page.

use eframe::egui;

use crate::theme::{self, radius, space, Palette, BODY, LABEL};

/// How long a write result stays bright before settling to a quieter tone.
pub const FLASH_MS: u64 = 1_600;

/// A tinted, bordered strip above the page content.
///
/// One definition of "something needs your attention": the recovery banner and
/// the crash banner differ only in colour and copy, and duplicating the frame
/// meant they had already drifted to different radii and margins.
pub fn banner(
    ui: &mut egui::Ui,
    p: &Palette,
    colour: egui::Color32,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::default()
        .fill(p.tint(colour, 22))
        .stroke(egui::Stroke::new(theme::HAIRLINE, p.tint(colour, 80)))
        .rounding(radius::CARD)
        .inner_margin(egui::Margin::symmetric(space::LG, space::MD))
        .show(ui, body);
    ui.add_space(space::MD);
}

/// Which half of a two-column row is being drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Left,
    Right,
}

/// Two equal cards side by side on the 12-column grid, stacked when the window
/// is too narrow to give each one a readable width.
///
/// Every two-column page goes through this, so the column split, the gutter and
/// the responsive breakpoint are one decision instead of five copies. A card
/// narrower than ~420 px cannot hold a stat row without wrapping, which is why
/// that is the breakpoint rather than a percentage.
///
/// Takes a *single* closure called once per column rather than two closures: two
/// `FnOnce`s that both capture the page would each need unique access to it, and
/// the borrow checker is right to refuse. The [`Column`] marker keeps the call
/// site readable — `match side { Left => …, Right => … }` reads as the layout it
/// draws.
pub fn two_columns(ui: &mut egui::Ui, mut body: impl FnMut(&mut egui::Ui, Column)) {
    let available = ui.available_width();
    let grid = theme::Grid::DEFAULT;
    // A two-column row is two 6-column spans; going through `legal_span` means
    // the layout cannot drift from the documented grid even if the constant
    // changes.
    let span = theme::Grid::legal_span(6);
    let half = grid.span_w(available, span);

    if half < MIN_CARD_W {
        // Stacked: at this width two columns would each be unreadable.
        body(ui, Column::Left);
        ui.add_space(space::MD);
        body(ui, Column::Right);
        return;
    }

    ui.horizontal_top(|ui| {
        ui.allocate_ui(egui::vec2(half, 0.0), |ui| {
            ui.set_width(half);
            body(ui, Column::Left);
        });
        ui.add_space(grid.gutter);
        ui.allocate_ui(egui::vec2(half, 0.0), |ui| {
            ui.set_width(half);
            body(ui, Column::Right);
        });
    });
}

/// Narrowest a card may be before the two-column layout gives up.
const MIN_CARD_W: f32 = 420.0;

/// A caption under a value: the "what is this number" line.
pub fn caption(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(LABEL)
            .color(p.faint())
            .family(theme::proportional()),
    );
}

/// Body copy at the body step. Used for the explanatory sentences that make a
/// card self-documenting instead of relying on a tooltip.
pub fn body_text(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(BODY)
            .color(p.dim)
            .family(theme::proportional()),
    );
}
