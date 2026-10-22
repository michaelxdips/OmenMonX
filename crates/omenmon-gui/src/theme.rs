//! Theme, type scale and persisted UI preferences.
//!
//! Visual language follows OMEN Gaming Hub: a dark chrome shell with a left
//! navigation rail, a single hot accent for the active/primary thing, and large
//! circular readouts for the two numbers people actually watch. The differences
//! are deliberate and documented where they matter:
//!
//! * **No gradient-on-gradient.** OGH layers red gradients on dark gradients and
//!   the result is mush at a glance. Here the accent appears once per screen
//!   region, so "active" is unambiguous.
//! * **State is never colour-only.** Every coloured state also carries a glyph or
//!   a word: a red/green distinction is invisible to a colour-blind reader and to
//!   a screenshot pasted into a bug report.
//! * **Tabular numerals.** Every value here updates at 1 Hz. Proportional digits
//!   shift horizontally on each update, which reads as jitter.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Type scale, in points. Fixed steps so the eye learns them.
///
/// Nothing else is a valid font size. A literal size in a page file is a bug:
/// it is how a UI ends up with eleven slightly different labels that read as
/// noise rather than hierarchy.
pub const DISPLAY: f32 = 40.0;
pub const HERO: f32 = 32.0;
pub const VALUE: f32 = 19.0;
pub const BODY: f32 = 13.5;
pub const LABEL: f32 = 11.0;

/// Page-title size. Sits between [`HERO`] and [`DISPLAY`] in the scale and is
/// named rather than written as `DISPLAY * 0.62` at the call site.
pub const TITLE: f32 = 24.0;

/// Spacing scale. Every gap is one of these; 4 / 8 / 12 / 16 / 24 / 32 and
/// nothing in between.
pub mod space {
    /// 4 — inside a tight pair (label to its value).
    pub const XS: f32 = 4.0;
    /// 8 — inside a card between rows.
    pub const SM: f32 = 8.0;
    /// 12 — between stacked cards.
    pub const MD: f32 = 12.0;
    /// 16 — card padding, gutter.
    pub const LG: f32 = 16.0;
    /// 24 — between page sections.
    pub const XL: f32 = 24.0;
    /// 32 — page margins, hero separation.
    pub const XXL: f32 = 32.0;
}

/// Corner radii. Three values, each with one job.
pub mod radius {
    /// 6 — controls: buttons, inputs, pills that are not fully round.
    pub const CONTROL: f32 = 6.0;
    /// 10 — cards and other raised surfaces.
    pub const CARD: f32 = 10.0;
    /// Fully round. Used for pills and status dots.
    pub const PILL: f32 = 999.0;
}

/// Hairline width. One pixel at every DPI, never two.
pub const HAIRLINE: f32 = 1.0;

/// Sidebar width. Wide enough for "Performance" at the body size.
pub const RAIL_W: f32 = 212.0;

/// The two elevations in the app, expressed as the frame each one draws.
///
/// Two levels only, and neither is a drop shadow: a hairline border does the work
/// a shadow would do on Windows 11, and it survives the DPI scaling that turns a
/// 1 px shadow into a grey smear. [`raised`] is what every card uses; [`flat`] is
/// for surfaces that sit directly on the page (table rows, the header strip).
pub mod elevation {
    use super::{radius, space, Palette, HAIRLINE};

    /// A card: filled panel, hairline border, card radius, card padding.
    pub fn raised(p: &Palette) -> eframe::egui::Frame {
        eframe::egui::Frame::default()
            .fill(p.panel)
            .stroke(eframe::egui::Stroke::new(HAIRLINE, p.border))
            .rounding(radius::CARD)
            .inner_margin(eframe::egui::Margin::symmetric(space::LG, space::LG))
    }

    /// A surface on the page: filled panel, no border, control radius.
    pub fn flat(p: &Palette) -> eframe::egui::Frame {
        eframe::egui::Frame::default()
            .fill(p.panel)
            .rounding(radius::CONTROL)
            .inner_margin(eframe::egui::Margin::symmetric(space::MD, space::SM))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// A top-level page. Replaces the old tab strip: OGH uses a rail, and a rail
/// scales to five destinations without wrapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Page {
    #[default]
    Dashboard,
    Performance,
    Lighting,
    System,
    About,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Dashboard,
        Page::Performance,
        Page::Lighting,
        Page::System,
        Page::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Dashboard => "Dashboard",
            Page::Performance => "Performance",
            Page::Lighting => "Lighting",
            Page::System => "System",
            Page::About => "About",
        }
    }

    /// One-line description shown under the page title. Keeps every screen
    /// self-explanatory without a tooltip.
    pub fn subtitle(self) -> &'static str {
        match self {
            Page::Dashboard => "Live thermal, fan and battery state",
            Page::Performance => "Power ceiling, thermal profile and display cadence",
            Page::Lighting => "Keyboard zones, straight from the firmware table",
            Page::System => "BIOS settings with the firmware's own return code",
            Page::About => "Build identity and what was measured on this board",
        }
    }

    /// A simple geometric glyph, drawn rather than fonted so it cannot fall back
    /// to a tofu box on a machine without the icon font.
    pub fn glyph(self) -> Glyph {
        match self {
            Page::Dashboard => Glyph::Gauge,
            Page::Performance => Glyph::Bolt,
            Page::Lighting => Glyph::Bulb,
            Page::System => Glyph::Chip,
            Page::About => Glyph::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Gauge,
    Bolt,
    Bulb,
    Chip,
    Info,
}

/// Everything the UI remembers between runs.
///
/// Stored in its own file, not in `config.toml`: that file is written by the CLI
/// and read by the login task, and a GUI window size must not be able to make
/// `omenmon apply` fail to parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub page: Page,
    /// Inner window size in points. `None` means "use the default".
    #[serde(default)]
    pub window: Option<[f32; 2]>,
    /// Compact mode hides both plots to fit on a 1366x768 panel.
    #[serde(default)]
    pub compact: bool,
    /// Minimise to the tray instead of closing.
    #[serde(default = "yes")]
    pub close_to_tray: bool,
    /// Refresh-rate preference, in Hz. `None` = leave the panel alone.
    #[serde(default)]
    pub refresh_hz: Option<u32>,
    /// UI zoom factor. Applied with `ctx.set_zoom_factor`, so it scales every
    /// widget rather than only the font sizes. Persisted because a user on a 4K
    /// panel sets it once and expects it back.
    #[serde(default = "default_zoom")]
    pub zoom: f32,
}

fn yes() -> bool {
    true
}

fn default_zoom() -> f32 {
    1.0
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            page: Page::Dashboard,
            window: None,
            compact: false,
            close_to_tray: true,
            refresh_hz: None,
            zoom: 1.0,
        }
    }
}

impl UiState {
    pub fn path() -> PathBuf {
        let home = std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        home.join(".config").join("omenmon").join("ui.toml")
    }

    pub fn load() -> Self {
        let p = Self::path();
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            // A corrupt UI file must never stop the app from starting.
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&p, t);
        }
    }
}

/// A 12-column grid with a 16 px gutter.
///
/// Cards snap to 3/4/6/8/12 columns and no card is ever narrower than 3. The
/// helper exists so page files say `grid::span(ui, 6, |ui| ...)` instead of
/// hand-dividing `available_width()`, which is how two cards end up 1 px apart
/// and neither aligns with the header.
pub struct Grid {
    pub columns: usize,
    pub gutter: f32,
}

impl Grid {
    pub const DEFAULT: Grid = Grid {
        columns: 12,
        gutter: space::LG,
    };

    /// Width of one column, given the available width.
    pub fn column_w(&self, available: f32) -> f32 {
        let gaps = self.gutter * (self.columns.saturating_sub(1)) as f32;
        ((available - gaps) / self.columns as f32).max(0.0)
    }

    /// Width of a `span`-column card, including its internal gutters.
    pub fn span_w(&self, available: f32, span: usize) -> f32 {
        let span = span.clamp(1, self.columns);
        self.column_w(available) * span as f32 + self.gutter * (span - 1) as f32
    }

    /// Snap a requested span to a legal one. Legal spans are 3, 4, 6, 8, 12.
    pub fn legal_span(requested: usize) -> usize {
        const LEGAL: [usize; 5] = [3, 4, 6, 8, 12];
        LEGAL
            .iter()
            .copied()
            .find(|s| *s >= requested)
            .unwrap_or(12)
    }
}

/// The palette for one theme.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Window chrome behind everything (the rail sits on this).
    pub shell: egui::Color32,
    /// Page background.
    pub bg: egui::Color32,
    /// Card surface.
    pub panel: egui::Color32,
    /// Raised surface: inputs, selected rows, hovered cards.
    pub raised: egui::Color32,
    pub border: egui::Color32,
    /// Primary brand accent. Used for the active nav item and primary actions.
    pub accent: egui::Color32,
    /// Accent at low alpha, for fills behind accent text.
    pub accent_soft: egui::Color32,
    /// CPU channel.
    pub cpu: egui::Color32,
    /// GPU channel.
    pub gpu: egui::Color32,
    pub red: egui::Color32,
    pub amber: egui::Color32,
    pub green: egui::Color32,
    pub text: egui::Color32,
    /// Secondary text.
    pub dim: egui::Color32,
    /// Tertiary text and disabled glyphs.
    pub faint: egui::Color32,
    pub dark_mode: bool,
}

impl Palette {
    /// OMEN's own identity: near-black chrome, crimson accent, cyan for the GPU.
    pub const DARK: Palette = Palette {
        shell: egui::Color32::from_rgb(0x0B, 0x0C, 0x0F),
        bg: egui::Color32::from_rgb(0x12, 0x13, 0x17),
        panel: egui::Color32::from_rgb(0x1A, 0x1C, 0x22),
        raised: egui::Color32::from_rgb(0x23, 0x26, 0x2E),
        border: egui::Color32::from_rgb(0x2C, 0x30, 0x3A),
        accent: egui::Color32::from_rgb(0xFF, 0x2E, 0x4D),
        accent_soft: egui::Color32::from_rgb(0x4A, 0x14, 0x1E),
        cpu: egui::Color32::from_rgb(0xFF, 0x4D, 0x36),
        gpu: egui::Color32::from_rgb(0x35, 0xC2, 0xF0),
        red: egui::Color32::from_rgb(0xFF, 0x4D, 0x4F),
        amber: egui::Color32::from_rgb(0xFF, 0xB0, 0x20),
        green: egui::Color32::from_rgb(0x3D, 0xDC, 0x84),
        text: egui::Color32::from_rgb(0xF2, 0xF3, 0xF5),
        dim: egui::Color32::from_rgb(0x9A, 0xA0, 0xAC),
        faint: egui::Color32::from_rgb(0x63, 0x69, 0x76),
        dark_mode: true,
    };

    /// Light theme keeps the same hue meanings but darkens each accent until it
    /// clears 4.5:1 on the light surface. The dark crimson is illegible on white.
    pub const LIGHT: Palette = Palette {
        shell: egui::Color32::from_rgb(0xE7, 0xE9, 0xEE),
        bg: egui::Color32::from_rgb(0xF5, 0xF6, 0xF9),
        panel: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
        raised: egui::Color32::from_rgb(0xEE, 0xF0, 0xF4),
        border: egui::Color32::from_rgb(0xD3, 0xD7, 0xDF),
        accent: egui::Color32::from_rgb(0xC4, 0x0E, 0x2C),
        accent_soft: egui::Color32::from_rgb(0xFB, 0xE0, 0xE4),
        cpu: egui::Color32::from_rgb(0xB8, 0x2A, 0x14),
        gpu: egui::Color32::from_rgb(0x0B, 0x6C, 0x8F),
        red: egui::Color32::from_rgb(0xC0, 0x1C, 0x22),
        amber: egui::Color32::from_rgb(0x8A, 0x53, 0x00),
        green: egui::Color32::from_rgb(0x14, 0x6B, 0x33),
        text: egui::Color32::from_rgb(0x16, 0x18, 0x1D),
        dim: egui::Color32::from_rgb(0x55, 0x5B, 0x67),
        faint: egui::Color32::from_rgb(0x86, 0x8C, 0x98),
        dark_mode: false,
    };

    pub fn of(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self::DARK,
            Theme::Light => Self::LIGHT,
        }
    }

    /// Temperature colour. Thresholds are the ones the thermal card documents.
    pub fn temp(&self, t: f32) -> egui::Color32 {
        if t >= 90.0 {
            self.red
        } else if t >= 78.0 {
            self.amber
        } else {
            self.green
        }
    }

    /// A soft fill of `c` suitable for a chip or a selected row.
    pub fn tint(&self, c: egui::Color32, alpha: u8) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
    }
}

/// Install fonts, palette and type scale.
///
/// Segoe UI and Consolas ship with Windows, so no font is bundled and the build
/// stays asset-free. If either is missing the egui defaults are kept, which is
/// ugly but never broken.
pub fn apply(ctx: &egui::Context, p: &Palette) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = p.dark_mode;
    style.visuals.panel_fill = p.bg;
    style.visuals.window_fill = p.panel;
    style.visuals.extreme_bg_color = p.shell;
    style.visuals.override_text_color = Some(p.text);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    style.visuals.widgets.inactive.bg_fill = p.raised;
    style.visuals.widgets.inactive.weak_bg_fill = p.raised;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    style.visuals.widgets.hovered.bg_fill = p.raised;
    style.visuals.widgets.hovered.weak_bg_fill = p.raised;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.accent);
    style.visuals.widgets.active.bg_fill = p.accent_soft;
    style.visuals.widgets.active.weak_bg_fill = p.accent_soft;
    style.visuals.selection.bg_fill = p.tint(p.accent, 90);
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, p.accent);
    style.visuals.window_rounding = egui::Rounding::same(10.0);
    style.visuals.menu_rounding = egui::Rounding::same(8.0);
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 24.0;

    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::new(HERO, proportional())),
        (egui::TextStyle::Body, egui::FontId::new(BODY, proportional())),
        (egui::TextStyle::Monospace, egui::FontId::new(BODY, numeric())),
        (egui::TextStyle::Button, egui::FontId::new(BODY, proportional())),
        (egui::TextStyle::Small, egui::FontId::new(LABEL, proportional())),
    ]
    .into();
    ctx.set_style(style);
}

/// Proportional family, Segoe UI when available.
pub fn proportional() -> egui::FontFamily {
    egui::FontFamily::Name("ui".into())
}

/// Tabular family, Consolas when available.
pub fn numeric() -> egui::FontFamily {
    egui::FontFamily::Name("num".into())
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};

    let mut fonts = FontDefinitions::default();
    let ui = load_font(&["segoeui.ttf", "SegoeUI.ttf"]);
    let num = load_font(&["consola.ttf", "Consola.ttf"]);

    match (ui, num) {
        (Some(ui), Some(num)) => {
            fonts
                .font_data
                .insert("omen-ui".to_owned(), FontData::from_owned(ui));
            fonts
                .font_data
                .insert("omen-num".to_owned(), FontData::from_owned(num));
            fonts
                .families
                .insert(FontFamily::Name("ui".into()), vec!["omen-ui".to_owned()]);
            fonts.families.insert(
                FontFamily::Name("num".into()),
                vec!["omen-num".to_owned(), "omen-ui".to_owned()],
            );
            // Keep the defaults reachable so any fallback glyph still resolves.
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push("omen-ui".to_owned());
            ctx.set_fonts(fonts);
        }
        _ => {
            // Fall back to egui's own families; nothing else changes.
            fonts.families.insert(
                FontFamily::Name("ui".into()),
                vec!["Ubuntu-Light".to_owned()],
            );
            fonts.families.insert(
                FontFamily::Name("num".into()),
                vec!["Hack".to_owned()],
            );
            ctx.set_fonts(fonts);
        }
    }
}

fn load_font(names: &[&str]) -> Option<Vec<u8>> {
    let root = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for n in names {
        let p = std::path::Path::new(&root).join("Fonts").join(n);
        if let Ok(b) = std::fs::read(p) {
            return Some(b);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light-theme accents must stay readable on the light surface. This is a
    /// coarse relative-luminance check, not a full WCAG audit, but it catches the
    /// obvious mistake of reusing the dark crimson on white.
    #[test]
    fn light_accents_clear_background() {
        fn lum(c: egui::Color32) -> f32 {
            let ch = |v: u8| {
                let s = v as f32 / 255.0;
                if s <= 0.03928 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
            let (x, y) = (lum(a), lum(b));
            let (hi, lo) = if x > y { (x, y) } else { (y, x) };
            (hi + 0.05) / (lo + 0.05)
        }
        let p = Palette::LIGHT;
        for (name, c) in [
            ("accent", p.accent),
            ("cpu", p.cpu),
            ("gpu", p.gpu),
            ("red", p.red),
            ("amber", p.amber),
            ("green", p.green),
            ("text", p.text),
            ("dim", p.dim),
        ] {
            let cr = contrast(c, p.panel);
            assert!(cr >= 4.5, "{name} on light panel is {cr:.2}:1, need 4.5");
        }
    }

    /// Dark chrome must stay dark, and the accent must actually be hot. A theme
    /// regression that swaps these is otherwise invisible until someone looks.
    #[test]
    fn dark_palette_is_dark_with_a_hot_accent() {
        let p = Palette::DARK;
        for (name, c) in [("shell", p.shell), ("bg", p.bg), ("panel", p.panel)] {
            assert!(c.r() < 0x30 && c.g() < 0x30 && c.b() < 0x40, "{name} is not dark");
        }
        assert!(p.accent.r() > 0xE0, "accent must be a hot red");
        assert!(p.accent.g() < 0x60);
        // Surface order: shell < bg < panel < raised. A broken order makes cards
        // read as holes rather than raised surfaces.
        assert!(p.shell.r() < p.bg.r());
        assert!(p.bg.r() < p.panel.r());
        assert!(p.panel.r() < p.raised.r());
    }

    #[test]
    fn palettes_differ() {
        assert_ne!(Palette::DARK.accent, Palette::LIGHT.accent);
        assert_ne!(Palette::DARK.bg, Palette::LIGHT.bg);
    }

    /// Temperature thresholds: cool is green, warm amber, hot red. The boundary
    /// values are inclusive, which is what the gauge labels assume.
    #[test]
    fn temp_colours_at_boundaries() {
        let p = Palette::DARK;
        assert_eq!(p.temp(40.0), p.green);
        assert_eq!(p.temp(77.9), p.green);
        assert_eq!(p.temp(78.0), p.amber);
        assert_eq!(p.temp(89.9), p.amber);
        assert_eq!(p.temp(90.0), p.red);
    }

    /// A truncated or corrupt ui.toml must fall back to defaults, not panic.
    #[test]
    fn ui_state_tolerates_missing_fields() {
        let s: UiState = toml::from_str("theme = \"light\"").expect("partial config parses");
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.page, Page::Dashboard);
        assert!(s.close_to_tray, "default is to keep sampling in the tray");
        assert!(toml::from_str::<UiState>("not toml at all").is_err());
    }

    /// A page name written by a future version must not break loading: serde
    /// rejects it, and `load()` falls back to defaults rather than crashing.
    #[test]
    fn unknown_page_is_rejected_by_serde() {
        assert!(toml::from_str::<UiState>("page = \"overclocking\"").is_err());
    }

    #[test]
    fn ui_state_roundtrips() {
        let s = UiState {
            theme: Theme::Light,
            page: Page::System,
            window: Some([1100.0, 720.0]),
            compact: true,
            close_to_tray: false,
            refresh_hz: Some(144),
            zoom: 1.5,
        };
        let t = toml::to_string_pretty(&s).unwrap();
        let back: UiState = toml::from_str(&t).unwrap();
        assert_eq!(back.theme, Theme::Light);
        assert_eq!(back.page, Page::System);
        assert_eq!(back.window, Some([1100.0, 720.0]));
        assert!(back.compact);
        assert!(!back.close_to_tray);
        assert_eq!(back.refresh_hz, Some(144));
        assert_eq!(back.zoom, 1.5);
    }

    /// P1-4. A zoom chosen once must survive a restart; that is the whole point
    /// of persisting it rather than keeping it in memory.
    #[test]
    fn zoom_survives_a_round_trip() {
        let s = UiState { zoom: 1.25, ..Default::default() };
        let back: UiState = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.zoom, 1.25);
    }

    /// An old ui.toml written before zoom existed must load at 1.0 rather than
    /// failing the whole file and losing the user's window size and theme.
    #[test]
    fn missing_zoom_defaults_to_one() {
        let s: UiState = toml::from_str("theme = \"light\"\npage = \"system\"\n").unwrap();
        assert_eq!(s.zoom, 1.0);
        assert_eq!(s.page, Page::System, "the rest of the file must still load");
    }

    /// Every page needs a title, a subtitle and a glyph; a missing arm would be
    /// a compile error, but an empty string is not, so check it.
    #[test]
    fn every_page_has_copy() {
        for p in Page::ALL {
            assert!(!p.title().is_empty());
            assert!(!p.subtitle().is_empty());
            let _ = p.glyph();
        }
    }

    /// The spacing scale has to stay a scale. If someone adds 10.0 because it
    /// "looked better", every gap in the app stops relating to every other gap.
    #[test]
    fn spacing_scale_is_the_documented_set() {
        let steps = [
            space::XS,
            space::SM,
            space::MD,
            space::LG,
            space::XL,
            space::XXL,
        ];
        assert_eq!(steps, [4.0, 8.0, 12.0, 16.0, 24.0, 32.0]);
        // Strictly increasing, so "next step up" is always meaningful.
        assert!(steps.windows(2).all(|w| w[1] > w[0]));
    }

    /// Three radii, three jobs. A fourth is how "rounded" stops meaning anything.
    #[test]
    fn radius_scale_has_three_values() {
        assert_eq!(radius::CONTROL, 6.0);
        assert_eq!(radius::CARD, 10.0);
        assert_eq!(radius::PILL, 999.0);
        // The ordering is a compile-time invariant, so state it as one.
        const _: () = assert!(radius::CONTROL < radius::CARD);
    }

    /// Elevation is two levels and neither is a shadow: a hairline does the work.
    /// A raised card carries exactly one border; a flat surface carries none.
    #[test]
    fn elevation_is_two_levels_of_hairline() {
        let p = Palette::of(Theme::Dark);
        let raised = elevation::raised(&p);
        let flat = elevation::flat(&p);

        assert_eq!(raised.stroke.width, HAIRLINE, "a 2 px border is a different design");
        assert_eq!(raised.stroke.color, p.border, "the raised border is the border token");
        assert_eq!(raised.rounding.nw, radius::CARD);
        assert_eq!(flat.stroke.width, 0.0, "a flat surface has no border");
        assert_eq!(flat.rounding.nw, radius::CONTROL);
        assert_eq!(HAIRLINE, 1.0);
        // Both are filled panels, so elevation is a border difference only.
        assert_eq!(raised.fill, p.panel);
        assert_eq!(flat.fill, p.panel);
    }

    /// The type scale is the documented five steps plus the page title.
    #[test]
    fn type_scale_is_the_documented_set() {
        let scale = [LABEL, BODY, VALUE, TITLE, HERO, DISPLAY];
        assert_eq!(scale, [11.0, 13.5, 19.0, 24.0, 32.0, 40.0]);
        assert!(scale.windows(2).all(|w| w[1] > w[0]), "the scale must be ordered");
    }

    /// Grid spans snap to 3/4/6/8/12, and nothing is narrower than 3.
    #[test]
    fn grid_snaps_to_legal_spans() {
        for (requested, expected) in [(1, 3), (2, 3), (3, 3), (4, 4), (5, 6), (6, 6), (7, 8), (8, 8), (9, 12), (12, 12), (99, 12)] {
            assert_eq!(Grid::legal_span(requested), expected, "span {requested}");
        }
    }

    /// A full row of cards must add up to the available width, or cards stop
    /// aligning with the page header. Note `span_w` returns the *content* width
    /// of the span, so tiling adds the gutters between cards explicitly.
    #[test]
    fn grid_spans_tile_the_row_exactly() {
        let g = Grid::DEFAULT;
        let available = 1200.0;
        // Three 4-column cards plus the two gutters between them fill the row.
        let three = g.span_w(available, 4) * 3.0 + g.gutter * 2.0;
        assert!((three - available).abs() < 0.5, "row is {three}, expected {available}");
        // Two 6-column cards plus one gutter fill the row.
        let two = g.span_w(available, 6) * 2.0 + g.gutter;
        assert!((two - available).abs() < 0.5, "row is {two}, expected {available}");
        // A single 12-column card is the whole width, no gutter.
        assert!((g.span_w(available, 12) - available).abs() < 0.5);
        // A wider span is always wider than a narrower one.
        assert!(g.span_w(available, 8) > g.span_w(available, 6));
        assert!(g.span_w(available, 6) > g.span_w(available, 4));
        assert!(g.span_w(available, 4) > g.span_w(available, 3));
    }

    /// Design rule: no colour literal outside this file.
    ///
    /// Enforced here so it can actually stop a regression. It scans the crate's own sources, so adding
    /// `Color32::from_rgb(...)` to a page fails the build instead of quietly
    /// creating a twelfth grey.
    #[test]
    fn no_colour_literals_outside_theme() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        collect_offenders(&src, &mut offenders);
        assert!(
            offenders.is_empty(),
            "colour literals outside theme.rs: {offenders:#?}"
        );
    }

    /// Walk the crate sources for hardcoded colours, skipping this file.
    ///
    /// Only *literal* construction counts. Reading a colour out of firmware data
    /// (`Color32::from_rgb(cur[0], cur[1], cur[2])`) is not a design decision, so
    /// those lines are allowed by name.
    fn collect_offenders(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                collect_offenders(&path, out);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some("theme.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for (i, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains("Color32::from_rgb(") || code.contains("Color32::from_rgba") {
                    // Firmware-derived colour, not a design token.
                    if code.contains("cur[0]") || code.contains("cur[1]") {
                        continue;
                    }
                    out.push(format!("{}:{}: {}", path.display(), i + 1, code.trim()));
                }
            }
        }
    }
}
