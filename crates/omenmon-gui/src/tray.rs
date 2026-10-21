//! Tray icon: minimise to tray and keep sampling.
//!
//! The point is not decoration. A power cap or thermal mode is only meaningful
//! while the sampler is running, so closing the window must not stop the thing
//! that watches the machine. Close hides the window; the tray brings it back.
//!
//! The icon is drawn at runtime rather than shipped as an .ico: one 16x16 RGBA
//! buffer of an amber arc on graphite, which is all the visual identity this app
//! has and all it needs. That also keeps the build free of binary assets that
//! nothing can regenerate.

use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the tray asked the app to do. Polled by the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Quit,
}

/// Handles shared with the UI. Dropping this removes the icon.
pub struct Tray {
    _icon: TrayIcon,
    pub commands: Arc<parking_lot::Mutex<Vec<TrayCommand>>>,
}

impl Tray {
    pub fn new() -> (Option<Self>, Option<String>) {
        let commands: Arc<parking_lot::Mutex<Vec<TrayCommand>>> = Arc::default();

        let menu = Menu::new();
        let show = MenuItem::new("Show OmenMon", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let show_id = show.id().clone();
        let quit_id = quit.id().clone();
        if let Err(e) = menu.append_items(&[&show, &quit]) {
            return (None, Some(format!("tray menu: {e}")));
        }

        let icon = match Icon::from_rgba(icon_rgba(), 16, 16) {
            Ok(i) => i,
            Err(e) => return (None, Some(format!("tray icon: {e}"))),
        };

        let tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("OmenMon-rs")
            // Left click must show the window, not open the menu: this is a
            // monitor first, and the common action is "bring it back".
            .with_menu_on_left_click(false)
            .with_icon(icon)
            .build()
        {
            Ok(t) => t,
            Err(e) => return (None, Some(format!("tray: {e}"))),
        };

        // Menu events arrive on a channel owned by the crate; forward the two we
        // care about into our own queue so the UI can drain it once per frame.
        let cmds = commands.clone();
        let show_for_cb = show_id.clone();
        let quit_for_cb = quit_id.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_for_cb {
                cmds.lock().push(TrayCommand::Show);
            } else if e.id == quit_for_cb {
                cmds.lock().push(TrayCommand::Quit);
            }
        }));

        let cmds = commands.clone();
        TrayIconEventBridge::install(cmds);

        (
            Some(Self { _icon: tray, commands }),
            None,
        )
    }

    /// Drain queued commands.
    pub fn drain(&self) -> Vec<TrayCommand> {
        std::mem::take(&mut *self.commands.lock())
    }
}

/// Forwards tray clicks (not menu items) into the same command queue.
struct TrayIconEventBridge;

impl TrayIconEventBridge {
    fn install(cmds: Arc<parking_lot::Mutex<Vec<TrayCommand>>>) {
        tray_icon::TrayIconEvent::set_event_handler(Some(move |e| {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                cmds.lock().push(TrayCommand::Show);
            }
        }));
    }
}

/// 16x16 amber arc on graphite, drawn as an anti-aliased ring segment.
///
/// Generated rather than loaded so the binary has no icon asset to lose, and so
/// the colour matches [`crate::theme::Palette::DARK`] amber exactly.
pub fn icon_rgba() -> Vec<u8> {
    const S: usize = 16;
    let mut px = vec![0u8; S * S * 4];
    let (bg, fg) = ([0x16u8, 0x18, 0x1D], [0xFFu8, 0xB4, 0x54]);
    let c = (S as f32 - 1.0) / 2.0;
    let r_out = 7.2f32;
    let r_in = 4.6f32;
    for y in 0..S {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let r = (dx * dx + dy * dy).sqrt();
            // Angle measured from 12 o'clock, clockwise; draw a 270-degree arc.
            let ang = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
            let in_arc = ang <= 270.0;
            // 1px feather on both radii.
            let ring = (r_out - r).clamp(0.0, 1.0).min((r - r_in).clamp(0.0, 1.0));
            let a = if in_arc { ring } else { 0.0 };
            let i = (y * S + x) * 4;
            for k in 0..3 {
                px[i + k] = (bg[k] as f32 * (1.0 - a) + fg[k] as f32 * a) as u8;
            }
            // Outside the ring but inside the arc stays opaque background, so the
            // icon reads as a filled disc rather than a floating stroke.
            px[i + 3] = 255;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_buffer_is_the_right_size_and_opaque() {
        let px = icon_rgba();
        assert_eq!(px.len(), 16 * 16 * 4);
        assert!(px.chunks(4).all(|p| p[3] == 255), "every pixel is opaque");
    }

    #[test]
    fn icon_actually_draws_amber() {
        let px = icon_rgba();
        let amber_pixels = px
            .chunks(4)
            .filter(|p| p[0] > 0xC0 && p[1] > 0x80 && p[2] < 0x90)
            .count();
        assert!(amber_pixels > 8, "the arc must be visible, found {amber_pixels} px");
        // And the centre is background, so it is a ring not a disc.
        let centre = (8 * 16 + 8) * 4;
        assert!(px[centre] < 0x40, "centre should be graphite, got {:#04X}", px[centre]);
    }
}
