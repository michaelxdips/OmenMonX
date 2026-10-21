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
}

pub struct Sampler {
    pub state: Arc<Mutex<SharedState>>,
    pub history: Arc<Mutex<History>>,
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
        let stop = Arc::new(AtomicBool::new(false));
        {
            let state = state.clone();
            let history = history.clone();
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
        Self { state, history, stop }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
