//! Background sampler thread: the only owner of the BIOS handle.
//!
//! Every control write is a [`ControlRequest`] consumed here, never on the UI
//! thread. A WMI round trip on this board costs tens of milliseconds and the
//! thermal-mode path sleeps 500 ms waiting for the firmware to settle; doing that
//! inside `update()` would freeze the window for the duration.
//!
//! The thread also owns the write *outcome*, so the UI can show what the firmware
//! did rather than what it asked for. Each request is answered with a
//! [`WriteReport`] that carries the verified readback.

use hp_core::battery;
use hp_core::sensors::ThermalSample;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A control write requested by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRequest {
    /// Set the CPU power cap, in watts. Clamped by the API to the proven window.
    SetPowerCap(u8),
    /// Switch thermal mode.
    SetThermal(hp_wmi::ThermalMode),
    /// Put the cap back to the firmware default.
    RestoreDefault,
    /// Write one zone's colour. Read-modify-write inside the sampler.
    SetZone { zone: u8, rgb: [u8; 3] },
    /// Restore the colour table captured before the first write.
    RestoreRgb,
    /// Keyboard backlight brightness, 0..=255 as the firmware reports it.
    SetBacklight(u8),
    /// Write one BIOS setting by name.
    SetBios { name: String, value: String },
}

/// How a write ended, for the UI's flash indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Firmware readback matched the request.
    Verified,
    /// Written, but the firmware settled on a different value. Not a failure —
    /// a clamp is the firmware being honest.
    Adjusted,
    /// The write was refused or the readback contradicted it.
    Failed,
}

/// One finished write.
#[derive(Debug, Clone)]
pub struct WriteReport {
    pub message: String,
    pub outcome: WriteOutcome,
    /// When it finished, so the UI can flash and then fade.
    pub at: Instant,
}

/// The colour table as the firmware holds it, plus the pristine copy.
#[derive(Debug, Clone, Default)]
pub struct RgbState {
    /// Zone count, from `colorTable[0]`. Never from `0x2B`.
    pub zones: u8,
    /// Per-zone RGB, parsed from `0x1A + 3*i`.
    pub colors: Vec<[u8; 3]>,
    /// The table as first read this session; `RestoreRgb` writes this back.
    pub pristine: Option<Vec<u8>>,
    /// Backlight byte from keyboard subtype 4.
    pub backlight: Option<u8>,
}

impl RgbState {
    /// Parse a 128-byte colour table.
    ///
    /// The zone count is `table[0]`. On board 8C9C `0x2B` reports `0x04`, which
    /// OGH's enum calls "OneZoneWithNumpad", while the table says 3 zones with 3
    /// distinct colours. The table is the authority.
    pub fn parse(table: &[u8]) -> Self {
        let zones = table.first().copied().unwrap_or(0).min(8);
        let mut colors = Vec::with_capacity(zones as usize);
        for i in 0..zones as usize {
            let o = 0x1A + i * 3;
            if o + 2 < table.len() {
                colors.push([table[o], table[o + 1], table[o + 2]]);
            }
        }
        Self {
            zones,
            colors,
            pristine: None,
            backlight: None,
        }
    }

    /// Build the table that would result from setting one zone, without writing.
    ///
    /// The zone must be **below the declared zone count** as well as inside the
    /// buffer. Checking only the buffer length is not enough: `0x1A + 3*zone`
    /// stays within 128 bytes for zones 0..37, and the bytes past the declared
    /// count are other table fields. Writing one of those would corrupt the
    /// colour table in a way that still reads back as a successful write.
    pub fn with_zone(table: &[u8], zone: u8, rgb: [u8; 3]) -> Option<Vec<u8>> {
        let declared = table.first().copied()?;
        if zone >= declared {
            return None;
        }
        let o = 0x1A + zone as usize * 3;
        if o + 2 >= table.len() {
            return None;
        }
        let mut t = table.to_vec();
        t[o] = rgb[0];
        t[o + 1] = rgb[1];
        t[o + 2] = rgb[2];
        Some(t)
    }
}

/// A BIOS setting as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosRow {
    pub name: String,
    pub current: String,
    pub possible: Vec<String>,
    pub read_only: bool,
    /// Last write result for this row, e.g. "accepted (rc 0)".
    pub last_write: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SharedState {
    pub sample: Option<ThermalSample>,
    pub battery: Option<battery::BatteryInfo>,
    pub bios_ok: bool,
    pub ec_ok: bool,
    pub target_fan_pct: u8,
    pub last_error: Option<String>,

    // ---- control state (verified channels only) ----
    /// The 0x2A limits block as the firmware reports it.
    pub limits: Option<hp_wmi::CpuPowerLimits>,
    /// 0x28[8] thermal status byte.
    pub thermal_status: u8,
    /// Firmware fan curve (read-only; 0x2E/0x32 are no-ops on this board).
    pub fan_table: Option<hp_wmi::FanTable>,
    /// Live clock, MHz. Sampled cheaply so the cap's effect is visible.
    pub clock_mhz: f32,
    /// Result of the most recent control write, for the UI to show verbatim.
    pub last_write: Option<WriteReport>,
    /// A write is in flight; the UI shows a pending state.
    pub write_pending: bool,

    // ---- RGB ----
    pub rgb: Option<RgbState>,

    // ---- BIOS ----
    pub bios_settings: Option<Vec<BiosRow>>,
    /// Why the BIOS list is empty, when it is.
    pub bios_error: Option<String>,
    pub bios_loading: bool,

    // ---- identity ----
    /// Elevated token: writes are possible.
    pub elevated: bool,
}

pub struct Sampler {
    pub state: Arc<Mutex<SharedState>>,
    pub history: Arc<Mutex<History>>,
    /// Pending control request, consumed by the sampler thread.
    pub request: Arc<Mutex<Option<ControlRequest>>>,
    stop: Arc<AtomicBool>,
}

/// Rolling thermal history for the sparkline (last N cpu/gpu temps).
#[derive(Debug, Clone)]
pub struct History {
    pub cpu: Vec<f32>,
    pub gpu: Vec<f32>,
    pub capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self { cpu: Vec::new(), gpu: Vec::new(), capacity }
    }

    /// Record one sample.
    ///
    /// Samples are only recorded when a real reading exists. Padding a missing
    /// reading with 0.0 would draw a flat 0 °C line and make "no data" look like
    /// "the machine is freezing", which is the worst possible lie for a thermal
    /// monitor.
    pub fn push(&mut self, cpu: Option<f32>, gpu: Option<f32>) {
        let (Some(cpu), Some(gpu)) = (cpu, gpu) else {
            return;
        };
        self.cpu.push(cpu);
        self.gpu.push(gpu);
        if self.cpu.len() > self.capacity {
            self.cpu.remove(0);
            self.gpu.remove(0);
        }
    }
}

impl Sampler {
    pub fn spawn(poll_ms: u64, elevated: bool) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            elevated,
            ..Default::default()
        }));
        let history = Arc::new(Mutex::new(History::new(240)));
        let request: Arc<Mutex<Option<ControlRequest>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let state = state.clone();
            let history = history.clone();
            let request = request.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("sampler".into())
                .spawn(move || {
                    let cfg = hp_core::Config::load();
                    let bios = hp_wmi::Bios::connect().ok();
                    let io = hp_ec::ecio::EcIo::connect().ok();
                    let engine = hp_core::engine::Engine::new(cfg);
                    let mut tick: u64 = 0;
                    // The pristine RGB table is captured once, on the first
                    // successful read, and never overwritten: it is the only
                    // byte-exact restore source.
                    let mut pristine_rgb: Option<Vec<u8>> = None;

                    // Tick cadence is expressed as `% N` rather than
                    // `is_multiple_of`: the counters are a schedule, and the
                    // modulo reads as one.
                    #[allow(clippy::manual_is_multiple_of)]
                    while !stop.load(Ordering::Relaxed) {
                        // ---- consume any pending control request first ----
                        let req = request.lock().take();
                        if let Some(r) = req {
                            if let Some(b) = &bios {
                                state.lock().write_pending = true;
                                let report = apply_control(b, r, &mut pristine_rgb);
                                let mut st = state.lock();
                                st.write_pending = false;
                                st.last_write = Some(report);
                            } else {
                                let mut st = state.lock();
                                st.write_pending = false;
                                st.last_write = Some(WriteReport {
                                    message: "no BIOS interface: cannot write (run elevated)".into(),
                                    outcome: WriteOutcome::Failed,
                                    at: Instant::now(),
                                });
                            }
                        }

                        let mut st = state.lock();
                        match &bios {
                            Some(b) => {
                                let s = engine.poll(b, io.as_ref());
                                st.sample = Some(s.sample);
                                st.target_fan_pct = s.target_fan_pct;
                                st.bios_ok = true;
                                st.ec_ok = io.is_some();
                                st.last_error = None;
                                history.lock().push(s.sample.cpu, s.sample.gpu);

                                // Control state. Read on a slower cadence than the
                                // thermal sample: these change rarely and each is a
                                // separate WMI round trip.
                                if tick % 2 == 0 {
                                    st.limits = b.get_cpu_power().ok();
                                    st.thermal_status = b.get_thermal_status().unwrap_or(0);
                                }
                                if tick % 20 == 0 {
                                    st.fan_table = b.get_fan_table_parsed().ok();
                                }
                                if tick % 2 == 1 {
                                    st.clock_mhz = read_clock_mhz().unwrap_or(0.0);
                                }
                                // RGB: slow cadence, and only when nothing else is
                                // queued. 128-byte reads are the most expensive
                                // call on this channel.
                                if tick % 10 == 3 {
                                    if let Ok(r) = b.get_color_table() {
                                        if r.data.len() >= 0x2A {
                                            let mut rgb = RgbState::parse(&r.data);
                                            if pristine_rgb.is_none() {
                                                pristine_rgb = Some(r.data.clone());
                                            }
                                            rgb.pristine = pristine_rgb.clone();
                                            rgb.backlight = b.get_backlight().ok();
                                            st.rgb = Some(rgb);
                                        }
                                    }
                                }
                            }
                            None => {
                                st.bios_ok = false;
                                st.last_error = Some(
                                    "WMI BIOS interface unavailable — run elevated for telemetry and control"
                                        .into(),
                                );
                            }
                        }
                        if tick % 10 == 0 {
                            st.battery = Some(battery::read_battery());
                        }
                        drop(st);
                        tick += 1;
                        std::thread::sleep(Duration::from_millis(poll_ms.max(250)));
                    }
                })
                .expect("spawn sampler");
        }
        Self { state, history, request, stop }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Queue a write. Last request wins: a slider drag produces many, and only
    /// the final position is a real intention.
    pub fn send(&self, req: ControlRequest) {
        *self.request.lock() = Some(req);
    }
}

fn ok(msg: impl Into<String>) -> WriteReport {
    WriteReport { message: msg.into(), outcome: WriteOutcome::Verified, at: Instant::now() }
}

fn adjusted(msg: impl Into<String>) -> WriteReport {
    WriteReport { message: msg.into(), outcome: WriteOutcome::Adjusted, at: Instant::now() }
}

fn failed(msg: impl Into<String>) -> WriteReport {
    WriteReport { message: msg.into(), outcome: WriteOutcome::Failed, at: Instant::now() }
}

/// Execute a control write and describe exactly what the firmware did.
fn apply_control(
    b: &hp_wmi::Bios,
    req: ControlRequest,
    pristine_rgb: &mut Option<Vec<u8>>,
) -> WriteReport {
    match req {
        ControlRequest::SetPowerCap(w) => set_cap(b, w),
        ControlRequest::RestoreDefault => restore_stock(b),
        ControlRequest::SetThermal(m) => match b.set_fan_mode_verified(m) {
            Ok(observed) => {
                let expected = m.expected_status();
                if observed == expected {
                    ok(format!("thermal mode applied, firmware status {observed:#04X} (verified)"))
                } else {
                    failed(format!(
                        "thermal mode not applied: status {observed:#04X}, expected {expected:#04X}"
                    ))
                }
            }
            Err(e) => failed(format!("thermal mode failed: {e}")),
        },
        ControlRequest::SetZone { zone, rgb } => set_zone(b, zone, rgb),
        ControlRequest::RestoreRgb => match pristine_rgb.clone() {
            Some(t) => match b.set_color_table(&t) {
                Ok(()) => {
                    std::thread::sleep(Duration::from_millis(250));
                    match b.get_color_table() {
                        Ok(r) if r.data == t => ok("colour table restored byte-for-byte (verified)"),
                        Ok(_) => failed("restore written but readback differs"),
                        Err(e) => failed(format!("restore written, readback failed: {e}")),
                    }
                }
                Err(e) => failed(format!("restore failed: {e}")),
            },
            None => failed("no original colour table captured yet"),
        },
        ControlRequest::SetBacklight(v) => match b.set_backlight(v) {
            Ok(()) => {
                std::thread::sleep(Duration::from_millis(200));
                match b.get_backlight() {
                    Ok(now) if now == v => ok(format!("backlight set to {v:#04X} (verified)")),
                    Ok(now) => adjusted(format!(
                        "backlight {v:#04X} read back as {now:#04X} — firmware adjusted it"
                    )),
                    Err(e) => failed(format!("backlight written, readback failed: {e}")),
                }
            }
            Err(e) => failed(format!("backlight failed: {e}")),
        },
        ControlRequest::SetBios { name, value } => set_bios(b, &name, &value),
    }
}

/// Restore the firmware's own power block.
///
/// Writes an over-limit value and lets the firmware re-apply its stock block.
/// This is the only reset that is correct on both supplies — 65/55/55 on the
/// 230 W brick, 25/25/25 on USB-C PD — and it is why the UI's restore button does
/// not write a number of its own.
fn restore_stock(b: &hp_wmi::Bios) -> WriteReport {
    let before = b.get_cpu_power().ok();
    crate::safety::write(&crate::safety::Pending::now(0, crate::safety::Phase::Restoring));
    if let Err(e) = b.reset_power_limits() {
        crate::safety::clear();
        return failed(format!("restore failed: {e}"));
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    match b.get_cpu_power() {
        Ok(l) => {
            crate::safety::clear();
            let changed = before.map(|x| x.pl1 != l.pl1).unwrap_or(true);
            let msg = if changed {
                format!(
                    "firmware stock restored: PL1={} PL2={} PL4={} W",
                    l.pl1, l.pl2, l.pl4
                )
            } else {
                format!("already at the firmware stock block (PL1={} W)", l.pl1)
            };
            ok(msg)
        }
        Err(e) => failed(format!("reset sent, readback failed: {e}")),
    }
}

/// Write the cap and verify by readback. Records the crash marker around it.
fn set_cap(b: &hp_wmi::Bios, w: u8) -> WriteReport {
    // Intent first: if this process dies between here and `Settled`, the next
    // start knows a cap may still be live.
    crate::safety::write(&crate::safety::Pending::now(w, crate::safety::Phase::Written));

    let applied = match b.set_power_limit(w) {
        Ok(v) => v,
        Err(e) => {
            crate::safety::clear();
            return failed(format!("cap {w} W failed: {e}"));
        }
    };
    std::thread::sleep(Duration::from_millis(600));
    match b.get_cpu_power() {
        Ok(l) => {
            if l.pl1 == applied {
                crate::safety::write(&crate::safety::Pending::settled(w, applied));
                if applied == w {
                    ok(format!("cap set to {applied} W (verified)"))
                } else {
                    adjusted(format!("cap {w} W clamped to {applied} W by firmware"))
                }
            } else {
                crate::safety::write(
                    &crate::safety::Pending::now(w, crate::safety::Phase::Written)
                        .with_observed(l.pl1),
                );
                failed(format!("cap {applied} W not held (reads back {} W)", l.pl1))
            }
        }
        Err(e) => {
            crate::safety::write(
                &crate::safety::Pending::now(w, crate::safety::Phase::Written),
            );
            adjusted(format!("cap {applied} W written, readback failed: {e}"))
        }
    }
}

fn set_zone(b: &hp_wmi::Bios, zone: u8, rgb: [u8; 3]) -> WriteReport {
    let orig = match b.get_color_table() {
        Ok(r) => r.data,
        Err(e) => return failed(format!("colour read failed: {e}")),
    };
    let Some(tbl) = RgbState::with_zone(&orig, zone, rgb) else {
        return failed(format!("zone {zone} is outside the table"));
    };
    if let Err(e) = b.set_color_table(&tbl) {
        return failed(format!("colour write failed: {e}"));
    }
    std::thread::sleep(Duration::from_millis(300));
    match b.get_color_table() {
        Ok(now) => {
            let o = 0x1A + zone as usize * 3;
            if now.data.len() >= o + 3 && now.data[o..o + 3] == tbl[o..o + 3] {
                ok(format!(
                    "zone {zone} set to #{:02X}{:02X}{:02X} (verified)",
                    rgb[0], rgb[1], rgb[2]
                ))
            } else {
                // Put the original back rather than leaving a half-applied table.
                let _ = b.set_color_table(&orig);
                failed("colour write not verified — original table restored")
            }
        }
        Err(e) => failed(format!("colour written, readback failed: {e}")),
    }
}

fn set_bios(b: &hp_wmi::Bios, name: &str, value: &str) -> WriteReport {
    let _ = b; // the settings channel is a separate COM namespace
    match hp_wmi::BiosSettings::connect() {
        Ok(s) => match s.set(name, value) {
            Ok(r) => {
                // Always show the code: "accepted" vs "value rejected" is the
                // whole advantage over OMEN Gaming Hub, which reports success
                // either way.
                let msg = format!("{name} <- {value:?}: {r} (rc {})", r.code());
                if r.is_ok() {
                    ok(msg)
                } else {
                    failed(msg)
                }
            }
            Err(e) => failed(format!("{name}: write failed: {e}")),
        },
        Err(e) => failed(format!("BIOS settings namespace unavailable: {e}")),
    }
}

/// Read the full BIOS setting list. Blocking; called from a worker thread.
pub fn load_bios_settings() -> Result<Vec<BiosRow>, String> {
    let s = hp_wmi::BiosSettings::connect().map_err(|e| format!("{e}"))?;
    let all = s.list().map_err(|e| format!("{e}"))?;
    Ok(all
        .into_iter()
        .map(|x| BiosRow {
            name: x.name,
            current: x.current,
            possible: x.possible,
            read_only: x.read_only,
            last_write: None,
        })
        .collect())
}

/// Live CPU clock in MHz.
///
/// `\Processor Frequency` is the nominal 3801 MHz and is useless for watching a
/// power cap bite. `Actual Frequency` is the counter the cap experiment used.
fn read_clock_mhz() -> Option<f32> {
    let out = crate::elevate::hidden_command("typeperf")
        .args([
            r"\Processor Information(_Total)\Actual Frequency",
            "-sc",
            "1",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        if line.starts_with('"') && line.matches('"').count() >= 4 {
            for p in line.split('"').rev() {
                let t = p.trim_matches(',').trim();
                if let Ok(v) = t.parse::<f64>() {
                    return Some(v as f32);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 128-byte table shape from board 8C9C: zoneCount = 3, three
    /// distinct colours at 0x1A/0x1D/0x20. A 4th slot has data but the header
    /// says 3, so the header wins.
    fn real_table() -> Vec<u8> {
        let mut t = vec![0u8; 128];
        t[0] = 3;
        t[0x1A..0x1D].copy_from_slice(&[0xFF, 0xFF, 0x07]);
        t[0x1D..0x20].copy_from_slice(&[0x07, 0x07, 0x00]);
        t[0x20..0x23].copy_from_slice(&[0x00, 0xFF, 0x07]);
        // A populated 4th slot that must be ignored.
        t[0x23..0x26].copy_from_slice(&[0x07, 0xFF, 0x00]);
        t
    }

    #[test]
    fn zone_count_comes_from_the_table_not_0x2b() {
        let s = RgbState::parse(&real_table());
        assert_eq!(s.zones, 3, "table[0] says 3 zones");
        assert_eq!(s.colors.len(), 3, "the 4th populated slot is not a zone");
        assert_eq!(s.colors[0], [0xFF, 0xFF, 0x07]);
        assert_eq!(s.colors[1], [0x07, 0x07, 0x00]);
        assert_eq!(s.colors[2], [0x00, 0xFF, 0x07]);
    }

    #[test]
    fn zone_write_touches_exactly_three_bytes() {
        let t = real_table();
        let out = RgbState::with_zone(&t, 1, [0x11, 0x22, 0x33]).expect("zone 1 exists");
        assert_eq!(out.len(), t.len(), "table width is preserved");
        let diffs: Vec<usize> = (0..t.len()).filter(|&i| t[i] != out[i]).collect();
        assert_eq!(diffs, vec![0x1D, 0x1E, 0x1F], "only zone 1's RGB moved");
        assert_eq!(&out[0x1D..0x20], &[0x11, 0x22, 0x33]);
    }

    #[test]
    fn zone_write_preserves_every_other_byte_including_the_4th_slot() {
        let t = real_table();
        let out = RgbState::with_zone(&t, 0, [0, 0, 0]).unwrap();
        assert_eq!(&out[0x23..0x26], &[0x07, 0xFF, 0x00], "4th slot untouched");
        assert_eq!(out[0], 3, "header untouched");
    }

    #[test]
    fn zone_outside_the_table_is_refused_not_panicked() {
        let t = real_table();
        // 8 is past the declared 3 zones but still inside 128 bytes; writing it
        // would land on unrelated fields.
        assert!(RgbState::with_zone(&t, 8, [1, 2, 3]).is_none());
        assert!(RgbState::with_zone(&t, 3, [1, 2, 3]).is_none(), "zone == count is out of range");
        assert!(RgbState::with_zone(&t, 2, [1, 2, 3]).is_some(), "last declared zone is writable");
        // A short table must also be refused rather than index out of bounds.
        assert!(RgbState::with_zone(&[0u8; 8], 0, [1, 2, 3]).is_none());
        // An empty table has no declared count at all.
        assert!(RgbState::with_zone(&[], 0, [1, 2, 3]).is_none());
    }

    /// A declared count larger than the buffer must not let a write run off the
    /// end: both bounds have to hold.
    #[test]
    fn declared_count_larger_than_buffer_is_still_bounds_checked() {
        let mut t = vec![0u8; 32];
        t[0] = 8; // claims 8 zones; 0x1A + 3*7 = 0x2F, past a 32-byte buffer
        assert!(RgbState::with_zone(&t, 7, [1, 2, 3]).is_none());
        assert!(RgbState::with_zone(&t, 0, [1, 2, 3]).is_some());
    }

    #[test]
    fn empty_table_parses_to_no_zones() {
        let s = RgbState::parse(&[]);
        assert_eq!(s.zones, 0);
        assert!(s.colors.is_empty());
    }

    /// A missing reading must not be recorded as 0 °C: that draws a flat line at
    /// the bottom of the chart and reads as "the CPU is at zero degrees".
    #[test]
    fn history_skips_missing_readings() {
        let mut h = History::new(4);
        h.push(Some(50.0), Some(45.0));
        h.push(None, None);
        h.push(Some(52.0), None);
        assert_eq!(h.cpu, vec![50.0], "only complete samples are recorded");
        assert_eq!(h.gpu, vec![45.0]);
        h.push(Some(54.0), Some(48.0));
        assert_eq!(h.cpu, vec![50.0, 54.0]);
    }

    #[test]
    fn history_evicts_oldest_at_capacity() {
        let mut h = History::new(3);
        for t in [40.0, 41.0, 42.0, 43.0] {
            h.push(Some(t), Some(t - 5.0));
        }
        assert_eq!(h.cpu, vec![41.0, 42.0, 43.0], "oldest sample is dropped");
        assert_eq!(h.gpu, vec![36.0, 37.0, 38.0]);
        assert_eq!(h.cpu.len(), h.gpu.len(), "channels stay aligned");
    }

    #[test]
    fn zero_zone_header_is_respected() {
        let mut t = vec![0u8; 128];
        t[0] = 0;
        t[0x1A..0x1D].copy_from_slice(&[9, 9, 9]);
        let s = RgbState::parse(&t);
        assert_eq!(s.zones, 0, "a 0 header means no zones, whatever the payload holds");
        assert!(s.colors.is_empty());
    }
}
