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
                // Same reasoning as the click handler: the UI loop is stopped while
                // hidden, so the window has to be shown from here.
                if !show_window_direct() {
                    cmds.lock().push(TrayCommand::Show);
                }
            } else if e.id == quit_for_cb {
                // Quit cannot be queued for the same reason Show could not: while
                // the window is hidden no frame runs, so a queued Quit would never
                // be drained and the menu item would do nothing. That is the one
                // thing a tray menu must never do, because it leaves the user
                // unable to stop a sampler that is still writing to the firmware.
                //
                // A visible window still goes through the queue, so `on_exit` runs
                // and the UI state is saved. A hidden window exits here.
                if !request_quit_direct() {
                    cmds.lock().push(TrayCommand::Quit);
                }
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
                // Show directly here rather than queueing for the UI thread. While the
                // window is hidden the update loop is not running, so a queued command
                // would never be drained - that was the bug that made the app
                // unreachable after closing the window.
                if !show_window_direct() {
                    // If the direct call failed (unexpected), still queue it so a
                    // future frame can retry.
                    cmds.lock().push(TrayCommand::Show);
                }
            }
        }));
    }
}

/// Bring the app window back without going through the egui update loop.
///
/// This cannot be done with `ViewportCommand::Visible(true)` alone. When the window
/// is hidden, winit stops delivering redraw events, so `eframe::App::update` never
/// runs again — which means a `Show` queued by the tray callback is never drained
/// and the window stays hidden forever. That was a real bug, confirmed by driving
/// the tray window proc directly: the callback fired, the command was queued, and
/// the window still never came back.
///
/// `ShowWindow` is a direct Win32 call, so it works from the tray callback thread
/// regardless of the UI loop state. Once the window is visible, winit resumes
/// delivering redraws and the normal event path takes over again.
///
/// The window is found by process id plus the title `OmenMon`, which only the real
/// window carries. Caching an HWND is not an option: the tray is created before the
/// first frame, so no handle exists yet at that point.
pub fn show_window_direct() -> bool {
    find_own_window().map(show_hwnd).unwrap_or(false)
}

/// Whether the app's own window is currently visible.
pub fn window_is_visible() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    find_own_window()
        .map(|hwnd| unsafe { IsWindowVisible(hwnd).as_bool() })
        .unwrap_or(false)
}

/// The HWND of this process's `OmenMon` window, if it exists yet.
///
/// Public within the crate so the quit path and the UI agree on exactly one
/// definition of "our window".
pub fn find_own_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId,
    };

    struct Ctx {
        pid: u32,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != ctx.pid {
            return BOOL(1);
        }
        let mut title = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut title);
        if len == 0 || String::from_utf16_lossy(&title[..len as usize]) != "OmenMon" {
            return BOOL(1);
        }
        ctx.found = Some(hwnd);
        BOOL(0)
    }

    let mut ctx = Ctx {
        pid: std::process::id(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut ctx as *mut Ctx as isize),
        );
    }
    ctx.found
}

fn show_hwnd(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    unsafe {
        // SW_SHOW first: the window was hidden, not minimised, so SW_RESTORE alone
        // can leave it hidden on some drivers.
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
        IsWindowVisible(hwnd).as_bool()
    }
}

/// What the tray callback must do about a Quit request.
///
/// Extracted as a pure function because the interesting part is the decision, not
/// the Win32 call: the bug was choosing the queue while no frame could drain it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitRoute {
    /// Exit from the callback; the process is on its way out.
    Direct,
    /// Queue it; a live UI loop will drain it and run `on_exit`.
    Queued,
}

/// Decide how to honour a Quit click given whether a frame can still run.
///
/// `window_visible` is the proxy for "the UI loop is alive": eframe stops
/// delivering redraws to a hidden window, so a queued command would sit in the
/// queue forever. That is exactly the P0-4 defect, and this is the line that
/// makes it impossible to reintroduce.
pub fn quit_route(window_visible: bool) -> QuitRoute {
    if window_visible {
        QuitRoute::Queued
    } else {
        QuitRoute::Direct
    }
}

/// Exit the process from the tray callback, without waiting for a frame.
///
/// Returns `true` when the caller must not queue anything further: the process is
/// already on its way out.
///
/// Two cases, deliberately distinguished:
///
/// * **Window hidden.** No frame will ever run, so the queued-command path is
///   dead. The process exits here. This is the P0-4 defect.
/// * **Window visible.** The UI loop is alive, so returning `false` lets the
///   caller queue `Quit`; the frame drains it, `on_exit` saves UI state, and the
///   shutdown is orderly.
///
/// The exit is `TerminateProcess` on our own handle rather than
/// `PostQuitMessage`: the tray callback runs on a thread that does not own the
/// message loop, and posting a quit to the wrong thread leaves the process alive.
pub fn request_quit_direct() -> bool {
    match quit_route(window_is_visible()) {
        QuitRoute::Queued => false,
        QuitRoute::Direct => {
            exit_now();
        }
    }
}

/// Terminate this process immediately. Never returns.
pub fn exit_now() -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    // TerminateProcess on the current process should not return. If it somehow
    // does, `process::exit` is the same outcome without unwinding.
    std::process::exit(0)
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

    /// P0-4. With the window hidden no frame runs, so a queued Quit is never
    /// drained and the menu item silently does nothing. The route must be direct
    /// exactly when the loop is dead.
    #[test]
    fn hidden_window_quits_directly() {
        assert_eq!(quit_route(false), QuitRoute::Direct);
    }

    /// A visible window still goes through the queue so `on_exit` can save UI
    /// state and stop the sampler in order. Quitting directly there would skip
    /// the state save, which is a different (quieter) bug.
    #[test]
    fn visible_window_quits_through_the_loop() {
        assert_eq!(quit_route(true), QuitRoute::Queued);
    }
}
