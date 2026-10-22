//! Debug-only frame instrumentation, enabled by the `OMENMON_PERF=1`
//! environment variable.
//!
//! The goal of the perf pass is measured claims, not vibes: this prints one
//! line per measurement window with the observed repaint rate and a rough
//! per-frame wall time, so "the GUI idles at N% CPU" can be checked against
//! real numbers in focused/unfocused, AC/battery and idle/animating states.
//!
//! Disabled by default and compiled out of the math entirely when the env
//! var is absent, so a normal run pays one `Option` check per frame and no
//! allocation.

use std::time::{Duration, Instant};

/// Measurement window. Long enough that one busy frame cannot skew the
/// average; short enough that a state change (focus, page switch) shows up
/// within seconds.
const WINDOW: Duration = Duration::from_secs(5);

pub struct PerfHud {
    enabled: bool,
    frames: u32,
    window_start: Instant,
    /// Wall time of the previous frame's end, for an average frame interval.
    last_frame_end: Option<Instant>,
    /// Sum of per-frame durations measured as time spent inside `update()`.
    /// Approximation by design: it includes layout and paint submission but
    /// not the GPU present wait.
    busy: Duration,
    frame_start: Option<Instant>,
}

impl PerfHud {
    pub fn from_env() -> Self {
        let enabled = std::env::var("OMENMON_PERF")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Self {
            enabled,
            frames: 0,
            window_start: Instant::now(),
            last_frame_end: None,
            busy: Duration::ZERO,
            frame_start: None,
        }
    }

    #[allow(dead_code)] // kept for future HUD gating; tests use it
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Call at the very top of the frame loop.
    pub fn frame_begin(&mut self) {
        if self.enabled {
            self.frame_start = Some(Instant::now());
        }
    }

    /// Call at the end of the frame loop. Accumulates and, once per window,
    /// prints `repaint rate`, `avg frame ms` and `avg busy ms`.
    pub fn frame_end(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if let Some(start) = self.frame_start.take() {
            self.busy += now - start;
        }
        self.frames += 1;

        if now.duration_since(self.window_start) >= WINDOW {
            let secs = now.duration_since(self.window_start).as_secs_f64();
            let rate = self.frames as f64 / secs;
            let interval_ms = if self.frames > 1 {
                // Interval between frame *starts* is the repaint cadence;
                // use frame count minus one over the busy-free span.
                let span = now
                    .duration_since(self.window_start)
                    .as_secs_f64();
                span / (self.frames as f64 - 1.0) * 1000.0
            } else {
                0.0
            };
            let busy_ms = if self.frames > 0 {
                self.busy.as_secs_f64() / self.frames as f64 * 1000.0
            } else {
                0.0
            };
            let _ = self.last_frame_end.insert(now);
            println!(
                "[omenmon-perf] frames={} window={:.1}s repaint={:.1}/s interval={:.1}ms busy/frame={:.2}ms",
                self.frames, secs, rate, interval_ms, busy_ms
            );
            self.frames = 0;
            self.window_start = now;
            self.busy = Duration::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_without_env() {
        // Not set in the test environment; if a developer exports it, skip.
        if std::env::var("OMENMON_PERF").is_ok() {
            return;
        }
        let mut hud = PerfHud::from_env();
        assert!(!hud.enabled());
        hud.frame_begin();
        hud.frame_end(); // must be a no-op, not a panic
    }

    #[test]
    fn window_resets_state() {
        let mut hud = PerfHud {
            enabled: true,
            frames: 10,
            window_start: Instant::now() - WINDOW,
            last_frame_end: None,
            busy: Duration::from_millis(50),
            frame_start: None,
        };
        hud.frame_end();
        assert_eq!(hud.frames, 0, "window flushed");
        assert_eq!(hud.busy, Duration::ZERO, "busy accumulator reset");
    }
}
