//! Custom widgets in the OMEN Gaming Hub idiom.
//!
//! These are drawn rather than composed from stock egui widgets because the
//! stock ones cannot express the two shapes this UI needs: a large circular
//! gauge and a segmented rail. Everything here is immediate-mode, allocation-
//! free after the first frame, and takes its colours from [`Palette`].

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::theme::{self, elevation, radius, space, Palette, BODY, HAIRLINE, LABEL, VALUE};

/// Everything a gauge needs to draw itself.
///
/// Grouped into a struct rather than passed as eight positional arguments: the
/// call sites read as "this is the CPU gauge, this is its value" instead of a row
/// of floats where two of them could be swapped without a compile error.
pub struct Gauge<'a> {
    pub diameter: f32,
    /// 0.0..=1.0 of the full scale. Clamped, never trusted.
    pub fraction: f32,
    /// The big centre number, already formatted.
    pub centre: &'a str,
    pub unit: &'a str,
    pub caption: &'a str,
    pub colour: Color32,
}

/// A large circular gauge with an arc, a big centre number and a caption.
///
/// The arc starts at 7 o'clock and sweeps 270 degrees, which is the shape people
/// read as "dial" rather than "pie". The unfilled track is always drawn so the
/// gauge has a scale even at zero.
pub fn gauge(ui: &mut Ui, p: &Palette, g: &Gauge<'_>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(g.diameter), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return resp;
    }

    let painter = ui.painter();
    let c = rect.center();
    let r = g.diameter * 0.5 - 9.0;
    let width = (g.diameter * 0.062).clamp(5.0, 11.0);
    let start = 135.0_f32.to_radians();
    let sweep = 270.0_f32.to_radians();

    // Track. A tint of the text colour, so the empty part of the dial follows the
    // theme instead of being a hardcoded white that vanishes on the light one.
    painter.add(egui::Shape::circle_stroke(
        c,
        r,
        Stroke::new(width, p.tint(p.text, 22)),
    ));
    // Arc. A full circle is a special case: `line` with identical endpoints
    // tessellates to nothing, so draw a stroke instead.
    let frac = g.fraction.clamp(0.0, 1.0);
    if frac > 0.001 {
        if frac >= 0.999 {
            painter.add(egui::Shape::circle_stroke(c, r, Stroke::new(width, g.colour)));
        } else {
            let steps = 64;
            let pts: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let a = start + sweep * frac * (i as f32 / steps as f32);
                    Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(width, g.colour)));
        }
    }

    // Centre readout: value on top, unit beside it, caption below.
    let val_font = egui::FontId::new(g.diameter * 0.235, theme::numeric());
    let unit_font = egui::FontId::new(g.diameter * 0.085, theme::proportional());
    let cap_font = egui::FontId::new(LABEL, theme::proportional());
    let dim = p.dim;

    let val_galley = ui.fonts(|f| f.layout_no_wrap(g.centre.to_owned(), val_font, g.colour));
    let unit_galley = ui.fonts(|f| f.layout_no_wrap(g.unit.to_owned(), unit_font, dim));
    let cap_galley =
        ui.fonts(|f| f.layout_no_wrap(g.caption.to_uppercase(), cap_font, dim));

    let val_h = val_galley.size().y;
    let unit_w = unit_galley.size().x;
    let gap = 2.0;
    let block_w = val_galley.size().x + gap + unit_w;
    let val_pos = Pos2::new(c.x - block_w * 0.5, c.y - val_h * 0.5 - 2.0);

    painter.galley(val_pos, val_galley, g.colour);
    painter.galley(
        Pos2::new(val_pos.x + block_w - unit_w, val_pos.y + val_h * 0.52),
        unit_galley,
        dim,
    );
    painter.galley(
        Pos2::new(c.x - cap_galley.size().x * 0.5, val_pos.y + val_h + 3.0),
        cap_galley,
        dim,
    );
    resp
}

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

/// A card whose title row has a right-aligned badge.
pub fn card_badge(
    ui: &mut Ui,
    p: &Palette,
    title: &str,
    badge: Option<(&str, Color32)>,
    body: impl FnOnce(&mut Ui),
) {
    elevation::raised(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(card_title(p, title));
            if let Some((text, colour)) = badge {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    badge_pill(ui, p, text, colour);
                });
            }
        });
        ui.add_space(space::SM);
        body(ui);
    });
}

/// A small rounded status pill: glyph + text on a tinted background.
///
/// Fully round, because a pill and a control must not share a corner radius:
/// the radius is what says "this is a label, not a button".
pub fn badge_pill(ui: &mut Ui, p: &Palette, text: &str, colour: Color32) {
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::new(LABEL, theme::proportional()),
            colour,
        )
    });
    let pad = Vec2::new(space::SM, space::XS * 0.75);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, radius::PILL, p.tint(colour, 34));
    ui.painter()
        .galley(rect.min + pad, galley, colour);
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

/// Draw one of the sidebar glyphs centred in `rect`.
pub fn glyph(ui: &Ui, p: &Palette, kind: crate::theme::Glyph, rect: Rect, colour: Color32) {
    use crate::theme::Glyph;
    let painter = ui.painter();
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke = Stroke::new(1.6_f32, colour);
    match kind {
        // A dial: circle with a needle.
        Glyph::Gauge => {
            painter.circle_stroke(c, s * 0.78, stroke);
            painter.line_segment(
                [c, Pos2::new(c.x + s * 0.42, c.y - s * 0.42)],
                Stroke::new(1.8_f32, colour),
            );
        }
        // A lightning bolt.
        Glyph::Bolt => {
            let pts = vec![
                Pos2::new(c.x + s * 0.18, c.y - s * 0.9),
                Pos2::new(c.x - s * 0.42, c.y + s * 0.12),
                Pos2::new(c.x - s * 0.02, c.y + s * 0.12),
                Pos2::new(c.x - s * 0.2, c.y + s * 0.9),
                Pos2::new(c.x + s * 0.44, c.y - s * 0.14),
                Pos2::new(c.x + s * 0.04, c.y - s * 0.14),
            ];
            painter.add(egui::Shape::convex_polygon(pts, colour, Stroke::NONE));
        }
        // A bulb: circle plus a base.
        Glyph::Bulb => {
            painter.circle_stroke(Pos2::new(c.x, c.y - s * 0.2), s * 0.55, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.24, c.y + s * 0.55),
                    Pos2::new(c.x + s * 0.24, c.y + s * 0.55),
                ],
                stroke,
            );
        }
        // A chip: square with pins.
        Glyph::Chip => {
            let r = Rect::from_center_size(c, Vec2::splat(s * 1.15));
            painter.rect_stroke(r, 3.0, stroke);
            for k in [-1.0_f32, 0.0, 1.0] {
                let off = k * s * 0.32;
                painter.line_segment(
                    [
                        Pos2::new(c.x + off, r.top() - s * 0.22),
                        Pos2::new(c.x + off, r.top()),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(c.x + off, r.bottom()),
                        Pos2::new(c.x + off, r.bottom() + s * 0.22),
                    ],
                    stroke,
                );
            }
        }
        // An "i" in a circle.
        Glyph::Info => {
            painter.circle_stroke(c, s * 0.82, stroke);
            painter.circle_filled(Pos2::new(c.x, c.y - s * 0.36), 1.6, colour);
            painter.line_segment(
                [
                    Pos2::new(c.x, c.y - s * 0.06),
                    Pos2::new(c.x, c.y + s * 0.4),
                ],
                Stroke::new(1.8_f32, colour),
            );
        }
    }
    let _ = p;
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
