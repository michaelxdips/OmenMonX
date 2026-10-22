//! Polling engine: samples thermals, computes target fan %, logs battery
//! alerts. Fan WRITES activate once the ECMMIO path lands (PawnIO signing);
//! today the engine is monitor-only with the write hook stubbed and clearly
//! flagged in status output.

use crate::config::Config;
use crate::sensors::{read_thermal_ec, read_thermal_wmi, ThermalSample};
use hp_ec::ecio::EcIo;
use hp_wmi::Bios;

pub struct Engine {
    pub config: Config,
    /// Previous computed fan target. Held so a poll with no readable
    /// temperature can hold the last target instead of commanding 0%.
    last_target: std::cell::Cell<Option<u8>>,
}

#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub sample: ThermalSample,
    pub target_fan_pct: u8,
    pub fan_write_active: bool,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            last_target: std::cell::Cell::new(None),
        }
    }

    /// One poll iteration. Returns status. Fan write hook is gated on the
    /// ECMMIO capability (not yet signed) — target computed but not applied.
    pub fn poll(&self, bios: &Bios, io: Option<&EcIo>) -> EngineStatus {
        let mut sample = match io {
            Some(io) => read_thermal_ec(io).unwrap_or_else(|| read_thermal_wmi(bios)),
            None => read_thermal_wmi(bios),
        };

        // fill gaps from WMI when EC missing — but only with a value that is
        // actually present; a missing sensor must stay None, not become 0.0 °C
        if sample.cpu.is_none() && sample.wmi_sensors[0] > 0 {
            sample.cpu = Some(sample.wmi_sensors[0] as f32);
        }
        if sample.gpu.is_none() && sample.wmi_sensors[1] > 0 {
            sample.gpu = Some(sample.wmi_sensors[1] as f32);
        }

        // target fan % from the max relevant temp; if neither CPU nor GPU is
        // readable this cycle, hold the previous target rather than computing
        // against a 0 °C floor.
        let temp = match (sample.cpu, sample.gpu) {
            (Some(c), Some(g)) => Some(c.max(g)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        };
        let target = match temp {
            Some(t) => self.config.fan_pct_for(t),
            None => self.last_target.get().unwrap_or(self.config.fan_pct_for(0.0)),
        };
        self.last_target.set(Some(target));

        EngineStatus {
            sample,
            target_fan_pct: target,
            // TODO: flip to true when signed PawnIO ECMMIO module ships
            fan_write_active: false,
        }
    }
}
