//! System: BIOS settings, display cadence, build, diagnostics.

use eframe::egui;

use crate::diag;
use crate::display;
use crate::elevate;
use crate::sampler::{BiosRow, ControlRequest, SharedState};
use crate::theme::{self, elevation, radius, space, Palette, BODY, LABEL};
use crate::ui::components as c;
use crate::updater;
use crate::widgets as w;
use crate::{build_hash, App};

/// Name column width. Fixed so values start at the same x on every row.
const NAME_COL_W: f32 = 300.0;

impl App {
    pub fn page_system(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        c::two_columns(ui, |ui, side| match side {
            c::Column::Left => {
                self.bios_card(ui, p, st);
            }
            c::Column::Right => {
                self.display_card(ui, p);
                ui.add_space(space::MD);
                self.build_card(ui, p, st);
                ui.add_space(space::MD);
                self.diagnostics_card(ui, p, st);
            }
        });
        ui.add_space(space::MD);
        self.capabilities_collapsed(ui, p);
    }

    fn bios_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "BIOS SETTINGS", |ui| {
            if st.bios_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    c::body_text(ui, p, "reading BIOS settings…");
                });
                return;
            }
            let Some(rows) = st.bios_settings.as_ref() else {
                // The raw WMI text is `Impersonation failed - Access denied
                // (0x80041003)`, which tells a user nothing they can act on. The
                // card says what is missing and what unlocks it instead.
                let reason = if st.elevated {
                    "the BIOS interface refused the read"
                } else {
                    "BIOS settings need an elevated token"
                };
                w::empty(
                    ui,
                    p,
                    reason,
                    Some(if st.elevated {
                        "the InstrumentedBIOS namespace did not answer on this boot"
                    } else {
                        "the InstrumentedBIOS namespace is admin-only; telemetry and battery still work"
                    }),
                );
                if w::quiet_button(ui, p, "Retry", true).clicked() {
                    self.load_bios_async();
                }
                return;
            };

            ui.horizontal(|ui| {
                c::body_text(ui, p, &format!("{} settings", rows.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::quiet_button(ui, p, "Reload", true).clicked() {
                        self.load_bios_async();
                    }
                });
            });
            ui.add_space(space::SM);
            table_header(ui, p);

            // Content-based height: rows (≈36 px each incl. margins) + the
            // table header, capped so a huge table cannot eat the page. The old
            // hardcoded -96.0 had no relation to what it was reserving.
            let row_h = 36.0_f32;
            let header_h = 26.0_f32;
            let max_h = (rows.len() as f32 * row_h + header_h)
                .min((ui.available_height() - header_h).max(160.0))
                .max(160.0);
            let rows = rows.clone();
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
                            .inner_margin(egui::Margin::symmetric(space::MD, 7.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.allocate_ui(egui::vec2(NAME_COL_W, 18.0), |ui| {
                                        ui.label(
                                            egui::RichText::new(&row.name)
                                                .size(BODY)
                                                .color(if row.read_only {
                                                    p.dim
                                                } else {
                                                    p.text
                                                })
                                                .family(theme::proportional()),
                                        );
                                    });
                                    ui.label(
                                        egui::RichText::new(&row.current)
                                            .size(BODY)
                                            .color(p.text)
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

            ui.add_space(space::SM);
            let Some(idx) = self.bios_selected else {
                c::caption(ui, p, "select a writable setting to edit it");
                return;
            };
            let Some(row) = rows.get(idx).cloned() else {
                self.bios_selected = None;
                return;
            };
            let row: BiosRow = row;
            // No accent stroke: the edit box is a control, not a selected state.
            egui::Frame::default()
                .fill(p.raised)
                .stroke(egui::Stroke::new(theme::HAIRLINE, p.border))
                .rounding(radius::CARD)
                .inner_margin(egui::Margin::symmetric(space::LG, space::SM))
                .show(ui, |ui| {
                    // Row 1: what is being edited. Row 2: the editor. A single
                    // row overflowed a half-width card at the 940 px minimum.
                    ui.label(
                        egui::RichText::new(&row.name)
                            .size(BODY)
                            .color(p.text)
                            .family(crate::theme::proportional()),
                    );
                    ui.add_space(space::SM);
                    ui.horizontal(|ui| {
                        if row.possible.is_empty() {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.bios_new_value)
                                    .desired_width(ui.available_width() - 96.0)
                                    .font(egui::TextStyle::Monospace),
                            );
                        } else {
                            egui::ComboBox::from_id_source("bios_val")
                                .selected_text(self.bios_new_value.clone())
                                .width((ui.available_width() - 96.0).max(120.0))
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
                    ui.add_space(space::SM);
                    c::caption(
                        ui,
                        p,
                        "rc 0 accepted · 1 no such setting · 5 value rejected · 6 reboot required",
                    );
                });
        });
    }

    fn display_card(&mut self, ui: &mut egui::Ui, p: &Palette) {
        w::card(ui, p, "DISPLAY CADENCE", |ui| {
            let cur = display::current();
            if self.rates.is_empty() {
                w::empty(
                    ui,
                    p,
                    "no modes reported",
                    Some("EnumDisplaySettings returned nothing"),
                );
                return;
            }
            if let Some(c) = cur {
                ui.label(
                    egui::RichText::new(format!(
                        "{} × {} · {} Hz",
                        c.width, c.height, c.hz
                    ))
                    .size(BODY)
                    .color(p.text)
                    .family(theme::numeric()),
                );
            }
            ui.add_space(space::SM);
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
                            theme::HAIRLINE,
                            if is_active { p.accent } else { p.border },
                        ))
                        .rounding(theme::radius::CONTROL)
                        .min_size(egui::vec2(0.0, 30.0));
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
                ui.add_space(space::SM);
                ui.label(
                    egui::RichText::new(msg)
                        .size(LABEL)
                        .color(if *ok { p.green } else { p.red })
                        .family(theme::proportional()),
                );
            }
            c::caption(
                ui,
                p,
                "CDS_TEST runs first, so a rejected mode never reaches the screen",
            );
        });
    }

    fn build_card(&mut self, ui: &mut egui::Ui, p: &Palette, _st: &SharedState) {
        w::card(ui, p, "BUILD", |ui| {
            ui.horizontal(|ui| {
                w::stat(ui, p, "version", concat!("v", env!("CARGO_PKG_VERSION")), p.text);
                ui.add_space(space::MD);
                w::stat(ui, p, "commit", build_hash(), p.dim);
            });
            ui.add_space(space::SM);
            c::caption(
                ui,
                p,
                &format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH),
            );
            ui.add_space(space::SM);
            w::divider(ui, p);
            // UPDATES inline under a divider: a card inside a card breaks the
            // two-elevation rule, and the content is small enough to share.
            self.updates_section(ui, p);
        });
    }

    fn updates_section(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.label(
            egui::RichText::new("UPDATES")
                .size(LABEL)
                .color(p.dim)
                .family(theme::proportional()),
        );
        ui.add_space(space::SM);
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
                    c::caption(ui, p, "contacting GitHub…");
                });
            }
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
                    egui::RichText::new(format!("✗ {why}"))
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
            if let Some(r) = updater::read_last_result() {
                let (mark, colour, text) = if r.ok {
                    (
                        "✓",
                        p.green,
                        format!("last install succeeded ({})", r.step),
                    )
                } else {
                    (
                        "✗",
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
                    egui::RichText::new(format!("{mark} {text}"))
                        .size(LABEL)
                        .color(colour)
                        .family(theme::proportional()),
                );
            }
    }
    fn diagnostics_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &SharedState) {
        w::card(ui, p, "DIAGNOSTICS", |ui| {
            c::body_text(ui, p, "One block with every readback for a bug report.");
            ui.add_space(space::SM);
            if w::quiet_button(ui, p, "Copy to clipboard", true).clicked() {
                let text = diag::build(st, self.tray_error.as_deref(), self.ui.zoom);
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

    fn capabilities_collapsed(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::CollapsingHeader::new("VERIFIED ON THIS BOARD")
            .default_open(false)
            .show(ui, |ui| {
                for (cap, state, note) in CAPABILITIES {
                    let (colour, mark) = match state {
                        Cap::Works => (p.green, "✓"),
                        Cap::Dead => (p.red, "✗"),
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(mark)
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
}

/// The sticky-looking header row above the settings table.
fn table_header(ui: &mut egui::Ui, p: &Palette) {
    elevation::flat(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(NAME_COL_W, 18.0), |ui| {
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
    ui.add_space(space::SM);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cap {
    Works,
    Dead,
}

/// Verified against the live board; dead means dead on this board only.
const CAPABILITIES: &[(&str, Cap, &str)] = &[
    (
        "CPU power cap",
        Cap::Works,
        "5–65 W on the 230 W brick, 5–25 W on USB-C; verified by clock outcome",
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
        "Fan max mode",
        Cap::Works,
        "0x27 {1,0,0,0}: live level 23→59, verified twice",
    ),
    (
        "Fan manual speed",
        Cap::Works,
        "0x2E {mode,cpu,gpu,fan3}: level 58→4 at 20 %, verified",
    ),
    (
        "Refresh rate",
        Cap::Works,
        "Win32 EnumDisplaySettings + CDS_TEST",
    ),
    (
        "Software fan curve write",
        Cap::Dead,
        "0x32 rejected; 0x2E takes a mode+speeds, not a curve",
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

