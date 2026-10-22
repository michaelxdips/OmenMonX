//! The diagnostics block a user pastes into a bug report.
//!
//! One function, deliberately separate from `main.rs`: it is the only place that
//! decides what counts as evidence, and keeping it beside the frame loop made
//! `main.rs` longer than the pages it drives.

use crate::sampler::SharedState;

/// Render every readback this app has into one plain-text block.
///
/// Plain text on purpose: a bug report is pasted into a chat window, so nothing
/// here may depend on a renderer. Every value is the raw readback or the parsed
/// value next to its source, never a rounded display string.
pub fn build(st: &SharedState, tray_error: Option<&str>, zoom: f32, theme: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("OmenMon-rs {}\n", env!("CARGO_PKG_VERSION")));
    s.push_str(&format!("build: {}\n", crate::build_hash()));
    s.push_str(&format!(
        "exe: {}\n",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    ));
    s.push_str(&format!("elevated: {}\n", st.elevated));
    s.push_str(&format!("bios wmi: {}\n", st.bios_ok));
    s.push_str(&format!("ec (pawnio): {}\n", st.ec_ok));
    s.push_str(&format!("thermal status: {:#04X}\n", st.thermal_status));
    match st.limits {
        Some(l) => s.push_str(&format!(
            "limits block 0x2A: PL1={} PL2={} PL4={} PLwGpu={}\n",
            l.pl1, l.pl2, l.pl4, l.pl_with_gpu
        )),
        None => s.push_str("limits block 0x2A: unreadable\n"),
    }
    if let Some(t) = &st.fan_table {
        s.push_str(&format!(
            "fan table: {} fans, {} points, peak {}%, monotonic {}\n",
            t.fan_count,
            t.level_count,
            t.max_duty(),
            t.is_monotonic()
        ));
    }
    if let Some(r) = &st.rgb {
        s.push_str(&format!("rgb: {} zones (from colorTable[0])\n", r.zones));
        for (i, c) in r.colors.iter().enumerate() {
            s.push_str(&format!("  zone {i}: #{:02X}{:02X}{:02X}\n", c[0], c[1], c[2]));
        }
    }
    if let Some(smp) = &st.sample {
        s.push_str(&format!(
            "temps: cpu {:?} gpu {:?} hotspot {:?}\n",
            smp.cpu, smp.gpu, smp.gpu_hotspot
        ));
    }
    s.push_str(&format!("clock: {:.0} MHz\n", st.clock_mhz));
    if let Some(b) = &st.battery {
        s.push_str(&format!(
            "battery: {:?}% health {:?}% cycles {:?} ac {}\n",
            b.charge_pct, b.health_pct, b.cycles, b.ac_online
        ));
    }
    if let Some(e) = &st.last_error {
        s.push_str(&format!("last error: {e}\n"));
    }
    if let Some(wr) = &st.last_write {
        s.push_str(&format!("last write: {:?} {}\n", wr.outcome, wr.message));
    }
    if let Some(e) = tray_error {
        s.push_str(&format!("tray: {e}\n"));
    }
    s.push_str(&format!("ui: {theme} zoom {zoom}\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block must always name the build and the elevation state, because
    /// those two answers resolve most reports before anything else is read.
    #[test]
    fn diag_always_names_build_and_elevation() {
        let st = SharedState::default();
        let text = build(&st, None, 1.0, "Dark");
        assert!(text.contains("OmenMon-rs "), "version line is mandatory");
        assert!(text.contains("build: "), "commit line is mandatory");
        assert!(text.contains("elevated: "), "elevation decides what can work");
    }

    /// A missing readback must be stated, not omitted: a silently absent line
    /// reads as "fine" when the truth is "unreadable".
    #[test]
    fn unreadable_blocks_are_stated_not_omitted() {
        let st = SharedState::default();
        let text = build(&st, None, 1.0, "Dark");
        assert!(text.contains("limits block 0x2A: unreadable"));
        assert!(!text.contains("limits block 0x2A: PL1"), "no invented numbers");
    }

    /// The tray error and zoom are part of the UI state a report needs.
    #[test]
    fn diag_includes_ui_state_and_tray_failure() {
        let st = SharedState::default();
        let text = build(&st, Some("no shell"), 1.25, "Light");
        assert!(text.contains("tray: no shell"));
        assert!(text.contains("zoom 1.25"));
        assert!(text.contains("Light"));
    }
}
