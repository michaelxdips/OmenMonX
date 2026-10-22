//! Behavioural tests for the shell.
//!
//! Kept out of `main.rs` so the shell stays the size of a shell, and so the
//! "main.rs is a shell" assertion below is not made trivially true by the tests
//! that check it.

use super::*;

/// P0-4, the visible-window half. Quit closes the window, so the close handler
/// must not treat that close as "hide to tray" — otherwise Quit leaves the app
/// running with no window, which is indistinguishable from a hang.
///
/// This is the case a real tray menu exposed: the process was still alive after
/// choosing Quit, because `CancelClose` had cancelled it.
#[test]
fn quit_close_is_not_turned_into_a_hide() {
    // Quitting wins over close-to-tray.
    assert!(!should_hide_on_close(true, true, true));
    // An ordinary close still hides.
    assert!(should_hide_on_close(true, true, false));
    // Without a tray icon, a close must exit (there would be no way back).
    assert!(!should_hide_on_close(true, false, false));
    // With the preference off, a close exits.
    assert!(!should_hide_on_close(false, true, false));
}

/// P1-4. The zoom clamp is what stops a persisted or hand-edited value from
/// making the window unusable, so its bounds are worth pinning.
#[test]
fn zoom_is_clamped_to_a_usable_range() {
    assert_eq!(clamp_zoom(1.0), 1.0);
    assert_eq!(clamp_zoom(0.1), ZOOM_MIN);
    assert_eq!(clamp_zoom(9.0), ZOOM_MAX);
    assert_eq!(clamp_zoom(f32::NAN), 1.0);
    assert_eq!(clamp_zoom(f32::INFINITY), ZOOM_MAX);
    // A step from either end cannot escape the range.
    assert!(clamp_zoom(ZOOM_MAX + ZOOM_STEP) <= ZOOM_MAX);
    assert!(clamp_zoom(ZOOM_MIN - ZOOM_STEP) >= ZOOM_MIN);
}

/// The window icon must be the tray icon: same buffer, same dimensions, and the
/// size must match the pixel data or the platform rejects it.
#[test]
fn window_icon_matches_the_tray_icon() {
    let icon = window_icon();
    assert_eq!(icon.width as usize * icon.height as usize * 4, icon.rgba.len());
    assert_eq!(icon.rgba, tray::icon_rgba());
}

/// P1-3. The sticky-cap policy must be visible to the user, not just
/// implemented. Checked against the page source so the copy cannot be deleted
/// while the behaviour stays sticky — that combination is worse than either
/// alone, because the machine throttles with no explanation.
#[test]
fn sticky_cap_policy_is_stated_in_the_ui() {
    let src = include_str!("ui/pages/performance.rs");
    assert!(
        src.contains("This cap stays until you change it or press Restore stock"),
        "the power card must state that the cap persists"
    );
    assert!(
        src.contains("re-applied at every login"),
        "and say what keeps it applied"
    );
}

/// P1-5. The window must carry the app's icon, not the default blank frame.
#[test]
fn viewport_is_built_with_an_icon() {
    let src = include_str!("main.rs");
    assert!(
        src.contains(".with_icon(window_icon())"),
        "the viewport must set an icon"
    );
}

/// P2-1. The shell must stay a shell. This is the structural half of the split:
/// a page that creeps back into `main.rs` would undo it silently, and the file
/// size is the only thing that makes the regression visible.
#[test]
fn main_stays_a_shell() {
    let src = include_str!("main.rs");
    let lines = src.lines().count();
    assert!(
        lines < 400,
        "main.rs is {lines} lines; pages belong in ui/pages"
    );
    // Built from parts so this assertion's own text cannot satisfy it.
    let marker = ["fn ", "page_", "dashboard"].concat();
    assert!(!src.contains(&marker), "page bodies must live in ui/pages");
}

/// `FLASH_MS` is a UI timing constant, so it is imported where it is used rather
/// than duplicated as a literal in a page.
#[test]
fn flash_window_is_a_single_constant() {
    let flash = crate::ui::components::FLASH_MS;
    assert!(flash >= 1_000, "a shorter flash is unreadable");
    let pages = include_str!("ui/pages/performance.rs");
    assert!(
        !pages.contains("1_600"),
        "the flash duration must come from the constant"
    );
}

/// The capability matrix must not claim fan control is dead.
///
/// This is a correction, not a formality: the About page used to say
/// "0x2E and 0x32 are silent no-ops" and listed software fan control as dead.
/// Hardware testing disproved that — 0x27 spins the fans to max (level 23→59,
/// twice) and 0x2E sets a manual speed (58→4 at 20 %). The old claim came from a
/// probe that sent the wrong payload shape, so the page was telling users a
/// feature they have does not exist.
#[test]
fn about_page_does_not_call_fan_control_dead() {
    let src = include_str!("ui/pages/about.rs");
    assert!(
        !src.contains("0x2E and 0x32 are silent no-ops"),
        "0x2E is verified working; that sentence was wrong"
    );
    assert!(
        src.contains("Fan max mode") && src.contains("Cap::Works"),
        "max fan must be listed as working"
    );
    assert!(
        src.contains("Fan manual speed"),
        "manual speed must be listed as working"
    );
}

/// The dashboard must offer a way *out* of max fan.
///
/// `0x27 [0,0,0,0]` does not release the fans on this board — verified: the level
/// stayed at 58-59 for 25 s, twice, and through a thermal profile change. Only a
/// 0x2E write released them. A "Max" button without a release is a trap.
#[test]
fn dashboard_offers_a_way_out_of_max_fan() {
    let src = include_str!("ui/pages/dashboard.rs");
    assert!(src.contains("SetMaxFan(true)"), "the Max button must exist");
    assert!(
        src.contains("ReleaseMaxFan"),
        "there must be a release control, or max mode is a one-way trip"
    );
}

/// The RPM stat was removed because the sensor is stale.
///
/// EC 0xC4 reads a frozen 1249 rpm on this board even while the thermal profile
/// changes, so showing it as "rpm" would present a constant as a measurement.
#[test]
fn dashboard_does_not_show_the_stale_rpm_sensor() {
    let src = include_str!("ui/pages/dashboard.rs");
    assert!(
        !src.contains("\"rpm\""),
        "0xC4 is frozen at 1249 rpm; do not display it as live"
    );
}

///
/// This is the bug a live run exposed: `find_own_window` required the class to be
/// `OmenMon`, but winit registers `Window Class`, so it returned `None` on every
/// call and both tray actions silently fell back to queueing a command a hidden
/// window can never drain. Asserted against the source because the failure only
/// shows up with a real window, which a unit test cannot create.
#[test]
fn window_lookup_does_not_filter_on_class_name() {
    let src = include_str!("tray.rs");
    let finder = src
        .split("pub fn find_own_window")
        .nth(1)
        .and_then(|s| s.split("/// Whether the app's own window").next())
        .expect("find_own_window must exist");
    assert!(
        !finder.contains("GetClassName"),
        "matching on the window class name broke the tray: winit uses 'Window Class'"
    );
    assert!(
        finder.contains("GetWindowTextW"),
        "the title is what identifies our window"
    );
}
