//! Polling engine: samples thermals, computes target fan %, logs battery
//! alerts. Fan WRITES activate once the ECMMIO path lands (PawnIO signing);
//! today the engine is monitor-only with the write hook stubbed and clearly
//! flagged in status output.

use crate::config::Config;
use crate::sensors::{read_thermal_ec, read_thermal_wmi, ThermalSample};
use hp_ec::ecio::EcIo;
use hp_wmi::Bios;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct Engine {
    pub config: Config,
    last_sample: parking_lot::Mutex<Option<ThermalSample>>,
    stop: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub sample: ThermalSample,
    pub target_fan_pct: u8,
    pub fan_write_active: bool,
    pub battery_alert: Option<String>,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            last_sample: parking_lot::Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// One poll iteration. Returns status. Fan write hook is gated on the
    /// ECMMIO capability (not yet signed) — target computed but not applied.
    pub fn poll(&self, bios: &Bios, io: Option<&EcIo>) -> EngineStatus {
        let mut sample = match io {
            Some(io) => read_thermal_ec(io).unwrap_or_else(|| read_thermal_wmi(bios)),
            None => read_thermal_wmi(bios),
        };

        // fill gaps from WMI when EC missing
        if sample.cpu.is_none() {
            sample.cpu = Some(sample.wmi_sensors[0] as f32);
        }
        if sample.gpu.is_none() {
            sample.gpu = Some(sample.wmi_sensors[1] as f32);
        }

        // target fan % from the max relevant temp
        let temp = sample.cpu.unwrap_or(0.0).max(sample.gpu.unwrap_or(0.0));
        let target = self.config.fan_pct_for(temp);

        // battery alert check (cheap; done in poll caller periodically)
        let battery_alert = None;

        *self.last_sample.lock() = Some(sample);
        EngineStatus {
            sample,
            target_fan_pct: target,
            // TODO: flip to true when signed PawnIO ECMMIO module ships
            fan_write_active: false,
            battery_alert,
        }
    }

    /// Run loop; calls `on_status` each tick until stopped.
    pub fn run<F: Fn(&EngineStatus)>(self, bios: Bios, io: Option<EcIo>, on_status: F) {
        let poll_ms = self.config.profile.poll_ms.max(250);
        while !self.stop.load(Ordering::Relaxed) {
            let st = self.poll(&bios, io.as_ref());
            on_status(&st);
            std::thread::sleep(Duration::from_millis(poll_ms as u64));
        }
    }
}
