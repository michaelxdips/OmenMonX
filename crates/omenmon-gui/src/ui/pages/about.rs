//! About: build identity, updates, diagnostics and the capability matrix.
//!
//! The capability matrix is the page's real content. A row that says "dead" is a
//! measurement that failed, and keeping it visible is what stops the next person
//! re-implementing software fan control for a fourth time.

use eframe::egui;

use crate::sampler::SharedState;
use crate::theme::{self, space, Palette, BODY, LABEL};
use crate::ui::components as c;
use crate::widgets as w;
use crate::{build_hash, diag, elevate, updater, App};

/// One verified capability.
enum Cap {
    Works,
    Dead,
}

/// Every row is a measurement from the hardware report; nothing here is
/// aspirational. A row that says "dead" had a test that failed.
const CAPABILITIES: &[(&str, Cap, &str)] = &[
    (
        "CPU power cap",
        Cap::Works,
        "5\u{2013}65 W on the 230 W brick, 5\u{2013}25 W on USB-C; verified by clock outcome",
    ),
    (
        "Thermal profile",
        Cap::Works,
        "0x1A V1 remap, status read back from 0x28[8]",
    ),
    (
        "Keyboard RGB",
        Cap::Works,
        "0x20009 subtype 2/3, byte-exact RMW restore",
    ),
    (
        "BIOS settings write",
        Cap::Works,
        "instance-path SetBIOSSetting, rc table calibrated",
    ),
    ("Fan curve read", Cap::Works, "0x2F, 42 points, 2 fans"),
    (
        "Refresh rate",
        Cap::Works,
        "Win32 EnumDisplaySettings + CDS_TEST",
    ),
    (
        "Software fan control",
        Cap::Dead,
        "0x2E and 0x32 are silent no-ops on 8C9C",
    ),
    (
        "Battery charge limit",
        Cap::Dead,
        "Battery Care Function returns rc 5 for all 27 encodings",
    ),
    ("GPU mode switch", Cap::Dead, "0x52 returns BIOS err 5"),
    (
        "BIOS undervolt / OC",
        Cap::Dead,
        "0x35 returns WMI err 31; 0x28[6] = 0",
    ),
];

impl App {
    pub fn page_about(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        // Two labelled sections rather than one undifferentiated stack: build and
        // updates answer "what am I running", the matrix answers "what can it do".
        c::section_header(ui, p, "This build", None);
        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                build_card(ui, p);
                ui.add_space(space::MD);
                self.updates_card(ui, p);
            }
            c::Column::Right => {
                self.diagnostics_card(ui, p, st);
            }
        });

        ui.add_space(space::XXL);
        c::section_header(ui, p, "Hardware support", None);
        capabilities_card(ui, p);
    }

    fn updates_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
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
                    c::caption(ui, p, "contacting GitHub\u{2026}");
                });
            }
            // Consent for code execution needs to say what will run and that it
            // was verified. A bare "Download & install" button is not informed
            // consent.
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
                    egui::RichText::new(format!("sha256 {short}\u{2026} \u{2014} checked before running"))
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
            ui.add_space(space::SM);
            ui.horizontal(|ui| {
                if w::quiet_button(ui, p, "Check", !busy).clicked() {
                    self.updates.check_async();
                }
                if available && w::primary_button(ui, p, "Download & install", !busy).clicked() {
                    self.updates.update_async();
                }
            });
            let s = self.updates.0.lock();
            if let Some((done, total)) = s.progress {
                let frac = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
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
            c::caption(ui, p, &format!("installs to {}", self.install_dir));
            // What the detached helper did last time. Without this the helper's
            // verdict is written and never read, which is the same silent failure
            // the errorlevel checks exist to prevent.
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
                                .map(|d| format!(" \u{2192} {d}"))
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
    }

    fn diagnostics_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "DIAGNOSTICS", |ui| {
            c::body_text(
                ui,
                p,
                "One block with every readback this app has, for a bug report.",
            );
            ui.add_space(space::SM);
            if w::quiet_button(ui, p, "Copy to clipboard", true).clicked() {
                let text = diag::build(
                    st,
                    self.tray_error.as_deref(),
                    self.ui.zoom,
                    match self.ui.theme {
                        crate::theme::Theme::Dark => "Dark",
                        crate::theme::Theme::Light => "Light",
                    },
                );
                self.diag = match elevate::copy_to_clipboard(&text) {
                    Ok(()) => "copied".into(),
                    Err(e) => format!("copy failed: {e}"),
                };
            }
            if !self.diag.is_empty() {
                c::caption(ui, p, &self.diag);
            }
        });
    }
}

fn build_card(ui: &mut egui::Ui, p: &Palette) {
    w::card(ui, p, "BUILD", |ui| {
        ui.horizontal(|ui| {
            w::stat(ui, p, "version", concat!("v", env!("CARGO_PKG_VERSION")), p.text);
            ui.add_space(space::XL);
            w::stat(ui, p, "commit", build_hash(), p.dim);
        });
        ui.add_space(space::SM);
        c::caption(
            ui,
            p,
            &format!("{} \u{00b7} {}", std::env::consts::OS, std::env::consts::ARCH),
        );
    });
}

fn capabilities_card(ui: &mut egui::Ui, p: &Palette) {
    w::card(ui, p, "VERIFIED ON THIS BOARD", |ui| {
        for (cap, state, note) in CAPABILITIES {
            let (colour, glyph) = match state {
                Cap::Works => (p.green, "\u{2713}"),
                Cap::Dead => (p.red, "\u{2717}"),
            };
            ui.horizontal(|ui| {
                // Glyph plus colour: the glyph carries the meaning when the
                // colour does not survive a screenshot or a colour deficiency.
                ui.label(
                    egui::RichText::new(glyph)
                        .size(BODY)
                        .color(colour)
                        .family(theme::numeric()),
                );
                ui.label(
                    egui::RichText::new(*cap)
                        .size(BODY)
                        .color(p.text)
                        .family(theme::proportional()),
                );
            });
            c::caption(ui, p, note);
            ui.add_space(space::SM);
        }
    });
}
