//! Background sampler thread: shared state between engine and GUI.

use hp_core::battery;
use hp_core::sensors::ThermalSample;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    pub last_write: Option<String>,
}

/// A control write requested by the UI, executed on the sampler thread so the
/// GUI never blocks on WMI and there is only one owner of the BIOS handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRequest {
    /// Set the CPU power cap, in watts. Clamped by the API to the proven window.
    SetPowerCap(u8),
    /// Switch thermal mode.
    SetThermal(hp_wmi::ThermalMode),
    /// Put the cap back to the firmware default.
    RestoreDefault,
}

pub struct Sampler {
    pub state: Arc<Mutex<SharedState>>,
    pub history: Arc<Mutex<History>>,
    /// Pending control request, consumed by the sampler thread.
    pub request: Arc<Mutex<Option<ControlRequest>>>,
    stop: Arc<AtomicBool>,
}

/// Rolling thermal history for sparkline (last N cpu/gpu temps).
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
    pub fn push(&mut self, cpu: Option<f32>, gpu: Option<f32>) {
        self.cpu.push(cpu.unwrap_or(0.0));
        self.gpu.push(gpu.unwrap_or(0.0));
        if self.cpu.len() > self.capacity {
            self.cpu.remove(0);
            self.gpu.remove(0);
        }
    }
}

impl Sampler {
    pub fn spawn(poll_ms: u64) -> Self {
        let state = Arc::new(Mutex::new(SharedState::default()));
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
                    while !stop.load(Ordering::Relaxed) {
                        // ---- consume any pending control request first ----
                        let req = request.lock().take();
                        if let (Some(b), Some(r)) = (&bios, req) {
                            let msg = apply_control(b, r);
                            state.lock().last_write = Some(msg);
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
                            }
                            None => {
                                st.bios_ok = false;
                                st.last_error = Some("WMI BIOS interface unavailable (run elevated for full telemetry)".into());
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
}

/// Execute a control write and return a message describing exactly what the
/// firmware did — including when it refused.
fn apply_control(b: &hp_wmi::Bios, req: ControlRequest) -> String {
    match req {
        ControlRequest::SetPowerCap(w) => {
            let applied = match b.set_power_limit(w) {
                Ok(v) => v,
                Err(e) => return format!("cap {w} W failed: {e}"),
            };
            std::thread::sleep(std::time::Duration::from_millis(600));
            match b.get_cpu_power() {
                Ok(l) => {
                    if l.pl1 == applied {
                        if applied == w {
                            format!("cap set to {applied} W (verified)")
                        } else {
                            format!("cap {w} W clamped to {applied} W by firmware")
                        }
                    } else {
                        format!("cap {applied} W not held (reads back {} W)", l.pl1)
                    }
                }
                Err(e) => format!("cap {applied} W written, readback failed: {e}"),
            }
        }
        ControlRequest::SetThermal(m) => {
            match b.set_fan_mode_verified(m) {
                Ok(observed) => {
                    let expected = m.expected_status();
                    if observed == expected {
                        format!("thermal mode applied, firmware status {observed:#04X} (verified)")
                    } else {
                        format!("thermal mode not applied: status {observed:#04X}, expected {expected:#04X}")
                    }
                }
                Err(e) => format!("thermal mode failed: {e}"),
            }
        }
        ControlRequest::RestoreDefault => {
            match b.set_power_limit(hp_wmi::CpuPowerLimits::MAX_WATTS) {
                Ok(v) => format!("cap restored to {v} W (firmware default)"),
                Err(e) => format!("restore failed: {e}"),
            }
        }
    }
}

/// Live CPU clock in MHz. Uses the same counter the power-cap experiment used;
/// `\Processor Frequency` is the nominal 3801 MHz and is useless for this.
fn read_clock_mhz() -> Option<f32> {
    let out = std::process::Command::new("typeperf")
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
