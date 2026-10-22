//! Shared UI pieces that more than one page uses.
//!
//! Everything here takes colours from [`Palette`] and sizes from the type and
//! spacing scales. There is deliberately no styling in this file that a page
//! could have written itself — if two pages need it, it lives here; if one page
//! needs it, it lives in that page.

use eframe::egui;

use crate::sampler::{SharedState, WriteOutcome};
use crate::theme::{self, radius, space, Palette, BODY, LABEL, VALUE};
use crate::widgets as w;

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
/// narrower than ~420 px cannot hold a gauge row without wrapping, which is why
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

/// A page section heading with an optional right-aligned action.
///
/// Distinct from a card title: this sits on the page background and groups
/// cards, so it is larger and carries primary text colour.
pub fn section_header(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    action: Option<&str>,
) -> Option<egui::Response> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(VALUE)
                .color(p.text)
                .family(theme::proportional()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(a) = action {
                let r = w::quiet_button(ui, p, a, true);
                if r.clicked() {
                    clicked = Some(r);
                }
            }
        });
    });
    ui.add_space(space::SM);
    clicked
}

/// The pending → verified/adjusted/failed indicator.
///
/// Every state carries a glyph *and* a word, so a colour-blind reader and a
/// greyscale screenshot both still say what happened.
pub fn write_feedback(ui: &mut egui::Ui, p: &Palette, st: &SharedState, pending_since: Option<std::time::Instant>) {
    if let Some(started) = pending_since {
        ui.add_space(space::SM);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new(format!("writing\u{2026} {} ms", started.elapsed().as_millis()))
                    .size(LABEL)
                    .color(p.amber)
                    .family(theme::numeric()),
            );
        });
        return;
    }
    let Some(wr) = st.last_write.as_ref() else { return };
    let fresh = wr.at.elapsed().as_millis() < FLASH_MS as u128;
    let (glyph, word, colour) = match wr.outcome {
        WriteOutcome::Verified => ("\u{2713}", "verified", p.green),
        WriteOutcome::Adjusted => ("\u{2248}", "adjusted", p.amber),
        WriteOutcome::Failed => ("\u{2717}", "refused", p.red),
    };
    let colour = if fresh { colour } else { colour.gamma_multiply(0.8) };
    ui.add_space(space::SM);
    ui.label(
        egui::RichText::new(format!("{glyph} {word} \u{00b7} {}", wr.message))
            .size(LABEL)
            .color(colour)
            .family(theme::proportional()),
    );
    if fresh {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// A label/value row for dense read-only lists.
///
/// The value is right-aligned in the numeric family so a column of numbers lines
/// up; the label is never centred, because a centred paragraph is on the
/// design rules.
pub fn kv_row(ui: &mut egui::Ui, p: &Palette, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(key)
                .size(LABEL)
                .color(p.dim)
                .family(theme::proportional()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(LABEL)
                    .color(p.text)
                    .family(theme::numeric()),
            );
        });
    });
}

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
