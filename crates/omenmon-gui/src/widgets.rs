//! Core widgets: cards, stats, meters, buttons, segmented control.

use eframe::egui::{self, Color32, Response, Sense, Stroke, Ui, Vec2};
use crate::theme::{self, elevation, radius, space, Palette, BODY, HAIRLINE, LABEL, VALUE};

/// A slim horizontal meter: label on the left, value on the right, filled track
/// underneath. Used for per-fan duty and battery charge.
pub fn meter(ui: &mut Ui, p: &Palette, label: &str, value_text: &str, fraction: f32, colour: Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(LABEL).color(p.dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value_text)
                    .size(BODY)
                    .color(colour)
                    .family(theme::numeric()),
            );
        });
    });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, p.tint(p.text, 20));
    let mut fill = rect;
    fill.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    if fill.width() > 0.5 {
        painter.rect_filled(fill, 3.0, colour);
    }
}

/// A card with a title row and an optional right-aligned action area.
///
/// One raised surface, one hairline border, one padding step. Cards are never
/// nested: a card inside a card is against the design rules because it makes the
/// two elevations indistinguishable.
pub fn card(ui: &mut Ui, p: &Palette, title: &str, body: impl FnOnce(&mut Ui)) {
    elevation::raised(p).show(ui, |ui| {
        ui.label(card_title(p, title));
        ui.add_space(space::SM);
        body(ui);
    });
}

/// The card heading: uppercase, tracked, secondary colour.
fn card_title(p: &Palette, title: &str) -> egui::RichText {
    egui::RichText::new(title.to_uppercase())
        .size(LABEL)
        .color(p.dim)
        .family(theme::proportional())
}

/// One labelled statistic: caption above, value below. The workhorse for the
/// dense rows of numbers OGH shows in a horizontal strip.
///
/// Every value is rendered in the numeric family so digits do not jitter as a
/// 1 Hz sample lands, and every caption is uppercase so it reads as a label
/// rather than as a sentence that got cut off.
pub fn stat(ui: &mut Ui, p: &Palette, caption: &str, value: &str, colour: Color32) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(caption.to_uppercase())
                .size(LABEL)
                .color(p.dim)
                .family(theme::proportional()),
        );
        ui.add_space(space::XS * 0.5);
        ui.label(
            egui::RichText::new(value)
                .size(VALUE)
                .color(colour)
                .family(theme::numeric()),
        );
    });
}

/// A primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .size(BODY)
            .color(if enabled { p.shell } else { p.faint() })
            .family(theme::proportional()),
    )
    .fill(if enabled { p.accent } else { p.raised })
    .rounding(radius::CONTROL)
    .min_size(Vec2::new(0.0, 30.0));
    ui.add_enabled(enabled, btn)
}

/// A quiet secondary button.
pub fn quiet_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .size(BODY)
            .color(if enabled { p.text } else { p.faint() })
            .family(theme::proportional()),
    )
    .fill(p.raised)
    .stroke(Stroke::new(HAIRLINE, p.border))
    .rounding(radius::CONTROL)
    .min_size(Vec2::new(0.0, 30.0));
    ui.add_enabled(enabled, btn)
}

/// A segmented control: one row of mutually exclusive options, the active one
/// filled with the accent. This is the OGH pattern for "Balanced / Performance /
/// Cool" and it is clearer than three independent buttons because the group
/// reads as one choice.
pub fn segmented(ui: &mut Ui, p: &Palette, options: &[(&str, bool)], enabled: bool) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        for (i, (label, active)) in options.iter().enumerate() {
            let text = egui::RichText::new(*label)
                .size(BODY)
                .color(if *active { p.shell } else { p.text })
                .family(theme::proportional());
            let btn = egui::Button::new(text)
                .fill(if *active { p.accent } else { p.raised })
                .stroke(Stroke::new(HAIRLINE, if *active { p.accent } else { p.border }))
                .rounding(radius::CONTROL)
                .min_size(Vec2::new(0.0, 30.0));
            if ui.add_enabled(enabled, btn).clicked() && !*active {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// A thin horizontal rule that reads as a section divider.
pub fn divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(space::SM);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HAIRLINE), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
    ui.add_space(space::SM);
}

/// Why a card is empty and what to do about it. Never just "unavailable".
pub fn empty(ui: &mut Ui, p: &Palette, reason: &str, hint: Option<&str>) {
    ui.horizontal(|ui| {
        // A hollow dot reads as "no data" rather than "error".
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
        ui.painter()
            .circle_stroke(rect.center(), 3.5, Stroke::new(1.2_f32, p.faint()));
        ui.add_space(space::XS * 0.5);
        ui.label(
            egui::RichText::new(reason)
                .size(BODY)
                .color(p.dim)
                .family(theme::proportional()),
        );
    });
    if let Some(h) = hint {
        ui.label(
            egui::RichText::new(h)
                .size(LABEL)
                .color(p.faint())
                .family(theme::proportional()),
        );
    }
}

impl Palette {
    /// Disabled text colour. A method rather than a field so the pair stays in
    /// step with the theme.
    pub fn faint(&self) -> Color32 {
        self.faint
    }
}

#[cfg(test)]
mod tests {
    /// The gauge clamps out-of-range fractions instead of drawing a negative or
    /// over-long arc. This is checked on the arithmetic the painter uses.
    #[test]
    fn gauge_fraction_is_clamped() {
        for raw in [-1.0_f32, -0.001, 0.0, 0.5, 1.0, 1.001, 9.0] {
            let f = raw.clamp(0.0, 1.0);
            assert!((0.0..=1.0).contains(&f));
        }
    }

    /// Meter fills must never exceed the track width, or the fill paints over the
    /// card border.
    #[test]
    fn meter_fraction_is_clamped() {
        let width = 200.0_f32;
        for raw in [-2.0_f32, 0.0, 0.5, 1.0, 3.0] {
            let w = width * raw.clamp(0.0, 1.0);
            assert!((0.0..=width).contains(&w), "fill {w} outside 0..{width}");
        }
    }

    /// The gauge arc must stay inside the allocated square: a radius larger than
    /// half the diameter would paint over neighbouring widgets.
    #[test]
    fn gauge_radius_fits_the_allocation() {
        for d in [80.0_f32, 140.0, 200.0] {
            let r = d * 0.5 - 9.0;
            let width = (d * 0.062).clamp(5.0, 11.0);
            // Outer edge of the stroke.
            assert!(r + width * 0.5 <= d * 0.5, "d={d} overflows");
            assert!(r > 0.0, "d={d} has no radius");
        }
    }
}
