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
    /// Set the thermal profile.
    SetThermal(hp_wmi::ThermalMode),
    /// Spin the fans to maximum (OGH's "Max" mode).
    SetMaxFan(bool),
    /// Hand the fans back to the firmware after max mode.
    ///
    /// Separate from `SetMaxFan(false)` because that alone does not release them
    /// on this board: the level stays pinned at maximum until a 0x2E write lands.
    ReleaseMaxFan,
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
    /// For a power-cap write, the wattage the firmware actually held. `None`
    /// for every other channel. Persistence saves this, not the request, so a
    /// cap the firmware clamped is not replayed at its unclamped value.
    pub held_watts: Option<u8>,
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
    /// True when the battery reports "discharging" — used by the UI to
    /// stretch the repaint floor (laptop battery saver).
    pub on_battery: bool,

    // ---- control state (verified channels only) ----
    /// The 0x2A limits block as the firmware reports it.
    pub limits: Option<hp_wmi::CpuPowerLimits>,
    /// 0x28[8] thermal status byte.
    pub thermal_status: u8,
    /// Firmware fan curve (read-only).
    ///
    /// Reading the curve works (0x2F); *writing* it (0x32) does not. Fan speed
    /// itself is controllable through 0x27 (max) and 0x2E (manual) — see
    /// [`ControlRequest::SetMaxFan`].
    pub fan_table: Option<hp_wmi::FanTable>,
    /// Fan count from 0x10. Shown instead of an RPM, because the EC tachometer at
    /// 0xC4 is frozen on this board and a stale number would be a lie.
    pub fan_count: Option<u8>,
    /// Live clock, MHz. Sampled cheaply so the cap's effect is visible.
    pub clock_mhz: f32,
    /// Latched clock-sampling failure. Once typeperf fails, it is not retried;
    /// the UI shows this instead of silently decaying to 0 MHz.
    pub clock_error: Option<String>,
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

/// True when a battery reading has no meaningful data at all — the shape
/// `read_battery` returns when its COM init fails. Such a reading is a
/// sampling failure, not a report that the battery is empty.
fn battery_is_empty(b: &battery::BatteryInfo) -> bool {
    b.full_charge_mwh.is_none()
        && b.design_mwh.is_none()
        && b.health_pct.is_none()
        && b.charge_pct.is_none()
        && b.cycles.is_none()
        && b.voltage_mv.is_none()
        && b.rate_mw.is_none()
        && !b.ac_online
        && !b.charging
        && b.temp_c.is_none()
}

impl Sampler {
    /// Like [`spawn`], but returns `Err` instead of panicking when the thread
    /// cannot be created, so the caller can degrade gracefully.
    pub fn try_spawn(poll_ms: u64, elevated: bool) -> std::io::Result<Self> {
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
                    sampler_loop(poll_ms, state, history, request, stop);
                })?;
        }
        Ok(Self { state, history, request, stop })
    }

    /// Build a dead sampler around pre-made state — used when the thread could
    /// not be started and the UI must still render a described failure.
    pub fn from_parts(state: Arc<Mutex<SharedState>>) -> Self {
        Self {
            state,
            history: Arc::new(Mutex::new(History::new(240))),
            request: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    #[allow(dead_code)] // convenience constructor kept for tests/tools
    pub fn spawn(poll_ms: u64, elevated: bool) -> Self {
        Self::try_spawn(poll_ms, elevated).expect("spawn sampler")
    }
}

/// The sampler thread body: telemetry, control writes, and publishing.
fn sampler_loop(
    poll_ms: u64,
    state: Arc<Mutex<SharedState>>,
    history: Arc<Mutex<History>>,
    request: Arc<Mutex<Option<ControlRequest>>>,
    stop: Arc<AtomicBool>,
) {
    {
        let cfg = hp_core::Config::load();
        let bios = hp_wmi::Bios::connect().ok();
        let io = hp_ec::ecio::EcIo::connect().ok();
        let engine = hp_core::engine::Engine::new(cfg);
        let mut tick: u64 = 0;
                    // The pristine RGB table is captured once, on the first
                    // successful read, and never overwritten: it is the only
                    // byte-exact restore source. It is only meaningful while
                    // no RGB write has run this session — after one, the
                    // "pristine" read is already our own output.
                    let mut pristine_rgb: Option<Vec<u8>> = None;
                    let mut rgb_touched: bool = false;
                    // Live clock. Kept locally and published, so a failed
                    // sample keeps the last good value.
                    let mut clock_mhz: f32 = 0.0;
                    // Latched clock-sampling failure: typeperf is re-spawned
                    // every tick, so one broken counter must not turn into a
                    // process a second. Latched once, reported forever, and
                    // the display keeps the last good MHz.
                    let mut clock_error: Option<String> = None;

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
                                let report =
                                    apply_control(b, r, &mut pristine_rgb, &mut rgb_touched, &stop);
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
                                    held_watts: None,
                                });
                            }
                        }

                        // ---- device I/O happens OUTSIDE the state lock ----
                        // A WMI round trip is tens of milliseconds and some
                        // calls sleep; holding `state` across them freezes
                        // every UI reader for the duration.
                        match &bios {
                            Some(b) => {
                                let s = engine.poll(b, io.as_ref());
                                history.lock().push(s.sample.cpu, s.sample.gpu);

                                // Control state. Read on a slower cadence than the
                                // thermal sample: these change rarely and each is a
                                // separate WMI round trip.
                                let mut limits = None;
                                let mut thermal_status = None;
                                if tick % 2 == 0 {
                                    limits = b.get_cpu_power().ok();
                                    thermal_status = Some(b.get_thermal_status().unwrap_or(0));
                                }
                                let mut fan_table = None;
                                let mut fan_count = None;
                                if tick % 20 == 0 {
                                    fan_table = b.get_fan_table_parsed().ok();
                                    fan_count = b.get_fan_count().ok();
                                }
                                if tick % 2 == 1 && clock_error.is_none() {
                                    match read_clock_mhz() {
                                        Some(v) => clock_mhz = v,
                                        None => {
                                            clock_error = Some(
                                                "clock sampling failed (typeperf); \
                                                 keeping the last reading"
                                                    .into(),
                                            );
                                        }
                                    }
                                }
                                // RGB: slow cadence, and only when nothing else is
                                // queued. 128-byte reads are the most expensive
                                // call on this channel.
                                let mut rgb_read: Option<RgbState> = None;
                                if tick % 10 == 3 {
                                    if let Ok(r) = b.get_color_table() {
                                        if r.data.len() >= 0x2A {
                                            let mut rgb = RgbState::parse(&r.data);
                                            if pristine_rgb.is_none() && !rgb_touched {
                                                pristine_rgb = Some(r.data.clone());
                                            }
                                            rgb.pristine = pristine_rgb.clone();
                                            rgb.backlight = b.get_backlight().ok();
                                            rgb_read = Some(rgb);
                                        }
                                    }
                                }

                                // ---- one brief lock to publish everything ----
                                let mut st = state.lock();
                                st.sample = Some(s.sample);
                                st.target_fan_pct = s.target_fan_pct;
                                st.bios_ok = true;
                                st.ec_ok = io.is_some();
                                st.last_error = None;
                                if let Some(l) = limits {
                                    st.limits = Some(l);
                                }
                                if let Some(t) = thermal_status {
                                    st.thermal_status = t;
                                }
                                if let Some(f) = fan_table {
                                    st.fan_table = Some(f);
                                }
                                if let Some(f) = fan_count {
                                    st.fan_count = Some(f);
                                }
                                st.clock_mhz = clock_mhz;
                                st.clock_error = clock_error.clone();
                                st.rgb = rgb_read;
                                // A request queued while a long apply_control is
                                // still running (or since this tick started) must
                                // keep the UI in its pending state.
                                st.write_pending = st.write_pending || request.lock().is_some();
                            }
                            None => {
                                let mut st = state.lock();
                                st.bios_ok = false;
                                st.last_error = Some(
                                    "WMI BIOS interface unavailable — run elevated for telemetry and control"
                                        .into(),
                                );
                            }
                        }
                        if tick % 10 == 0 {
                            let b = battery::read_battery();
                            // An all-empty reading means the COM init failed, not
                            // that the battery vanished; keep the last real one.
                            let mut st = state.lock();
                            if st.battery.is_none() || !battery_is_empty(&b) {
                                st.battery = Some(b.clone());
                            }
                            // Discharging flag drives the UI's idle repaint
                            // floor (battery saver). Cheap to publish here.
                            st.on_battery = !b.ac_online;
                        }
                        tick += 1;
                        std::thread::sleep(Duration::from_millis(poll_ms.max(250)));
                    }
    }
}

impl Sampler {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Queue a write. Last request wins: a slider drag produces many, and only
    /// the final position is a real intention.
    ///
    /// Overwriting a *different* channel's request is not silent: the displaced
    /// request gets a `Dropped` report so the UI shows that it was superseded
    /// rather than vanishing. Two requests on the same channel (two cap writes,
    /// say) overwrite quietly — the older one is genuinely obsolete.
    pub fn send(&self, req: ControlRequest) {
        let mut q = self.request.lock();
        if let Some(prev) = q.take() {
            if channel(&prev) != channel(&req) {
                let mut st = self.state.lock();
                st.last_write = Some(WriteReport {
                    message: format!("{} — superseded by a newer request", describe(&prev)),
                    outcome: WriteOutcome::Failed,
                    at: Instant::now(),
                    held_watts: None,
                });
            }
        }
        *q = Some(req);
    }
}

/// Which control channel a request belongs to. Requests on the same channel
/// supersede each other silently; different channels are reported when dropped.
fn channel(r: &ControlRequest) -> u8 {
    match r {
        ControlRequest::SetPowerCap(_) | ControlRequest::RestoreDefault => 0,
        ControlRequest::SetThermal(_) => 1,
        ControlRequest::SetMaxFan(_) | ControlRequest::ReleaseMaxFan => 2,
        ControlRequest::SetZone { .. } | ControlRequest::RestoreRgb => 3,
        ControlRequest::SetBacklight(_) => 4,
        ControlRequest::SetBios { .. } => 5,
    }
}

/// Short human description of a request, for a "superseded" report.
fn describe(r: &ControlRequest) -> String {
    match r {
        ControlRequest::SetPowerCap(w) => format!("cap {w} W"),
        ControlRequest::RestoreDefault => "restore stock limits".into(),
        ControlRequest::SetThermal(m) => format!("thermal profile {m:?}"),
        ControlRequest::SetMaxFan(on) => format!("max fan {}", if *on { "on" } else { "off" }),
        ControlRequest::ReleaseMaxFan => "hand fans back to firmware".into(),
        ControlRequest::SetZone { zone, rgb } => {
            format!("zone {zone} -> #{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
        }
        ControlRequest::RestoreRgb => "restore colour table".into(),
        ControlRequest::SetBacklight(v) => format!("backlight {v:#04X}"),
        ControlRequest::SetBios { name, value } => format!("{name} <- {value:?}"),
    }
}

fn ok(msg: impl Into<String>) -> WriteReport {    WriteReport {
        message: msg.into(),
        outcome: WriteOutcome::Verified,
        at: Instant::now(),
        held_watts: None,
    }
}

fn adjusted(msg: impl Into<String>) -> WriteReport {
    WriteReport {
        message: msg.into(),
        outcome: WriteOutcome::Adjusted,
        at: Instant::now(),
        held_watts: None,
    }
}

fn failed(msg: impl Into<String>) -> WriteReport {
    WriteReport {
        message: msg.into(),
        outcome: WriteOutcome::Failed,
        at: Instant::now(),
        held_watts: None,
    }
}

/// Execute a control write and describe exactly what the firmware did.
fn apply_control(
    b: &hp_wmi::Bios,
    req: ControlRequest,
    pristine_rgb: &mut Option<Vec<u8>>,
    rgb_touched: &mut bool,
    stop: &AtomicBool,
) -> WriteReport {
    // Any executed RGB write makes the current table ours; a later read is no
    // longer a pristine snapshot.
    if matches!(
        req,
        ControlRequest::SetZone { .. } | ControlRequest::RestoreRgb
    ) {
        *rgb_touched = true;
    }
    // A write that has a config representation must persist it, or the GUI and
    // the CLI disagree about what the machine is set to and the logon task
    // re-applies a stale value. Persistence is keyed on the *verified* outcome,
    // not on the request, so a refused write never becomes a saved preference.
    let mut report = match &req {
        ControlRequest::SetPowerCap(w) => set_cap(b, *w),
        ControlRequest::RestoreDefault => restore_stock(b),
        ControlRequest::SetThermal(m) => set_thermal(b, *m),
        ControlRequest::SetMaxFan(on) => set_max_fan(b, *on, stop),
        ControlRequest::ReleaseMaxFan => release_max_fan(b),
        ControlRequest::SetZone { zone, rgb } => set_zone(b, *zone, *rgb),
        ControlRequest::RestoreRgb => restore_rgb(b, pristine_rgb),
        ControlRequest::SetBacklight(v) => set_backlight(b, *v),
        ControlRequest::SetBios { name, value } => set_bios(b, name, value),
    };
    if let Some(note) = persist(&req, &report) {
        report.message.push_str(&note);
    }
    report
}

/// Persist the config half of a write that succeeded.
///
/// Returns a suffix for the write-feedback message so the user is told whether
/// the choice will survive a reboot — silence about persistence is how a user
/// ends up with a machine that is throttled after a restart with no explanation.
fn persist(req: &ControlRequest, report: &WriteReport) -> Option<String> {
    let ok = matches!(
        report.outcome,
        WriteOutcome::Verified | WriteOutcome::Adjusted
    );
    if !ok {
        return None;
    }
    let mut cfg = hp_core::Config::load();
    match req {
        ControlRequest::SetPowerCap(_) => {
            // Only a *verified* readback may be persisted. `held_watts: None`
            // means the readback failed, and the firmware's actual state is
            // unknown: recording the requested value would replay a number
            // the machine may not hold at the next login. Nothing is saved
            // instead, and the previous preference stays in place.
            let Some(held) = report.held_watts else {
                return Some(
                    " · readback failed — nothing saved; the previous saved cap stands"
                        .to_string(),
                );
            };
            Some(match cfg.record_power_cap(Some(held)) {
                Ok(()) => format!(" · saved {held} W for next login"),
                Err(e) => format!(" · WARNING: could not save the cap: {e}"),
            })
        }
        ControlRequest::RestoreDefault => Some(
            match cfg.record_power_cap(None) {
                Ok(()) => " · cleared the saved cap".to_string(),
                Err(e) => format!(" · WARNING: could not clear the saved cap: {e}"),
            },
        ),
        ControlRequest::SetThermal(m) => {
            let name = match m {
                hp_wmi::ThermalMode::Balanced => "balanced",
                hp_wmi::ThermalMode::Performance => "performance",
                hp_wmi::ThermalMode::Cool => "cool",
            };
            Some(match cfg.record_thermal_mode(Some(name)) {
                Ok(()) => format!(" · saved thermal profile '{name}'"),
                Err(e) => format!(" · WARNING: could not save the profile: {e}"),
            })
        }
        // The colour table and BIOS settings have no config representation: they
        // live in the firmware and survive a reboot on their own.
        _ => None,
    }
}

/// Spin the fans to maximum and confirm the level actually rose.
///
/// Verification is against 0x2D, which is a live read. It is **not** against the
/// EC tachometer at 0xC4: that register reads a frozen 1249 rpm on this board, so
/// a check based on it would report success or failure at random.
fn set_max_fan(b: &hp_wmi::Bios, on: bool, stop: &AtomicBool) -> WriteReport {
    let before = b.fan_levels().map(|(a, _)| a).unwrap_or(0);
    if let Err(e) = b.set_max_fan(on) {
        return failed(format!("max fan failed: {e}"));
    }
    // The fans take several seconds to spool. Sampling too early reads the old
    // level and would report a working control as broken. The stop flag is
    // checked every round so shutdown is not held up for the full 7 seconds.
    let mut peak = before;
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(900));
        if stop.load(Ordering::Relaxed) {
            return failed("max fan applied but verification was cut short by shutdown");
        }
        if let Ok((a, _)) = b.fan_levels() {
            peak = peak.max(a);
        }
    }
    if !on {
        // Turning the flag off is not enough on this board; say so rather than
        // reporting a success the user can hear is not true.
        return adjusted(
            "max-fan flag cleared, but the fans stay pinned until you press \
             \"Hand back to firmware\""
                .to_string(),
        );
    }
    if peak > before.saturating_add(5) {
        ok(format!("fans at maximum (level {before} → {peak}, verified)"))
    } else {
        failed(format!(
            "max fan accepted but level did not rise ({before} → {peak})"
        ))
    }
}

/// Release max mode and hand the fans back to the firmware.
fn release_max_fan(b: &hp_wmi::Bios) -> WriteReport {
    let before = b.fan_levels().map(|(a, _)| a).unwrap_or(0);
    match b.release_max_fan() {
        Ok(after) => {
            // Sample again: one read right after the write can still catch the old
            // value while the fans spool down.
            std::thread::sleep(Duration::from_millis(1500));
            let settled = b.fan_levels().map(|(a, _)| a).unwrap_or(after);
            if settled < before || settled <= 30 {
                ok(format!(
                    "fans handed back to firmware (level {before} → {settled}, verified)"
                ))
            } else {
                adjusted(format!(
                    "release sent, but level is still {settled} (was {before})"
                ))
            }
        }
        Err(e) => failed(format!("release failed: {e}")),
    }
}

/// Set the thermal profile and verify it against the firmware's own status byte.
fn set_thermal(b: &hp_wmi::Bios, m: hp_wmi::ThermalMode) -> WriteReport {
    match b.set_fan_mode_verified(m) {
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
    }
}

fn restore_rgb(b: &hp_wmi::Bios, pristine_rgb: &Option<Vec<u8>>) -> WriteReport {
    match pristine_rgb.clone() {
        Some(t) => {
            // The rollback is the only copy of the original table: one failed
            // write is retried before admitting the table may be left partial.
            let mut last_err = None;
            let mut wrote = false;
            for _ in 0..2 {
                match b.set_color_table(&t) {
                    Ok(()) => {
                        wrote = true;
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            if !wrote {
                let e = last_err.expect("at least one attempt was made");
                return failed(format!(
                    "restore FAILED after a retry — the colour table may be partial: {e}"
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
            match b.get_color_table() {
                Ok(r) if r.data == t => ok("colour table restored byte-for-byte (verified)"),
                Ok(_) => failed("restore written but readback differs"),
                Err(e) => failed(format!("restore written, readback failed: {e}")),
            }
        }
        None => failed("no original colour table captured yet"),
    }
}

fn set_backlight(b: &hp_wmi::Bios, v: u8) -> WriteReport {
    match b.set_backlight(v) {
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
    // A marker write failure here is only advisory (a restore in progress), but
    // it must not be silent: without the marker a crash mid-restore loses the
    // recovery banner.
    let marker_warn = match crate::safety::write(&crate::safety::Pending::now(
        0,
        crate::safety::Phase::Restoring,
    )) {
        Ok(()) => None,
        Err(e) => Some(format!(" WARNING: crash-recovery marker failed: {e}")),
    };
    if let Err(e) = b.reset_power_limits() {
        crate::safety::clear();
        let mut r = failed(format!("restore failed: {e}"));
        if let Some(w) = marker_warn {
            r.message.push_str(&w);
        }
        return r;
    }
    // Try the readback twice before giving up: the first read can catch the
    // firmware mid-transition. But the reset *was* sent either way, so the
    // marker is always cleared — leaving `Restoring` stranded would pin the
    // recovery banner until the next restart for no recoverable fault.
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
            let mut r = ok(msg);
            if let Some(w) = marker_warn {
                r.message.push_str(&w);
            }
            r
        }
        Err(_first) => {
            std::thread::sleep(std::time::Duration::from_millis(800));
            match b.get_cpu_power() {
                Ok(l) => {
                    crate::safety::clear();
                    ok(format!(
                        "reset sent; settled readback: PL1={} PL2={} PL4={} W",
                        l.pl1, l.pl2, l.pl4
                    ))
                }
                Err(e) => {
                    crate::safety::clear();
                    failed(format!("reset sent; readback failed: {e}"))
                }
            }
        }
    }
}

/// Write the cap and verify by readback. Records the crash marker around it.
fn set_cap(b: &hp_wmi::Bios, w: u8) -> WriteReport {
    // Intent first: if this process dies between here and `Settled`, the next
    // start knows a cap may still be live. A marker write failure is folded
    // into the report — a crash without the marker leaves the cap applied with
    // no recovery path, so the user must be told.
    if let Err(e) = crate::safety::write(&crate::safety::Pending::now(
        w,
        crate::safety::Phase::Written,
    )) {
        return failed(format!(
            "cap not attempted: crash-recovery marker could not be written: {e}"
        ));
    }

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
                let _ = crate::safety::write(&crate::safety::Pending::settled(w, applied));
                let mut r = if applied == w {
                    ok(format!("cap set to {applied} W (verified)"))
                } else {
                    adjusted(format!("cap {w} W clamped to {applied} W by firmware"))
                };
                // The readback is the truth, so this is what gets persisted.
                r.held_watts = Some(l.pl1);
                r
            } else {
                let _ = crate::safety::write(
                    &crate::safety::Pending::now(w, crate::safety::Phase::Written)
                        .with_observed(l.pl1),
                );
                failed(format!("cap {applied} W not held (reads back {} W)", l.pl1))
            }
        }
        Err(e) => {
            let _ = crate::safety::write(
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
                // Put the original back rather than leaving a half-applied
                // table. The rollback is the only copy, so a failed write is
                // retried once before admitting the table may be partial.
                let mut rolled_back = false;
                for _ in 0..2 {
                    if b.set_color_table(&orig).is_ok() {
                        rolled_back = true;
                        break;
                    }
                }
                if rolled_back {
                    failed("colour write not verified — original table restored")
                } else {
                    failed("colour write not verified — restore FAILED, table may be partial")
                }
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
