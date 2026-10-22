//! Thermal sampling: dual-source (WMI commands + direct EC), merged.

use hp_ec::ecio::EcIo;
use hp_wmi::Bios;

#[derive(Debug, Clone, Copy, Default)]
pub struct ThermalSample {
    /// CPU °C (WMI sensor 0 / EC 0xB7)
    pub cpu: Option<f32>,
    /// GPU °C (WMI sensor 1 / EC 0xB2)
    pub gpu: Option<f32>,
    /// GPU hotspot candidate (EC 0xB0)
    pub gpu_hotspot: Option<f32>,
    /// WMI sensors 0..4 raw
    pub wmi_sensors: [u8; 5],
    /// Fan level percent (WMI)
    pub fan_level_pct: Option<u8>,
}

/// Read thermals via WMI only (no admin, no driver needed).
pub fn read_thermal_wmi(bios: &Bios) -> ThermalSample {
    let mut s = ThermalSample::default();
    for i in 0..5u8 {
        if let Ok(r) = bios.get_temperature(i) {
            if let Some(b) = r.byte0() {
                if b > 0 && b < 120 {
                    s.wmi_sensors[i as usize] = b;
                }
            }
        }
    }
    s.cpu = Some(s.wmi_sensors[0] as f32);
    s.gpu = Some(s.wmi_sensors[1] as f32);
    if let Ok(r) = bios.get_fan_level() {
        if let Some(b) = r.byte0() {
            s.fan_level_pct = Some(b);
        }
    }
    s
}

/// Read thermals via direct EC (PawnIO) — fast, no WMI round-trip.
/// Falls back silently if PawnIO unavailable.
pub fn read_thermal_ec(io: &EcIo) -> Option<ThermalSample> {
    let mut s = ThermalSample::default();
    io.with_lock(2000, |io| {
        let eram = hp_ec::eram::Eram::new(io);
        s.cpu = eram.read(0xB7).ok().map(|v| v as f32);
        s.gpu = eram.read(0xB2).ok().map(|v| v as f32);
        s.gpu_hotspot = eram.read(0xB0).ok().map(|v| v as f32);
        // NOTE: EC 0xC4/0xC5 (fan tachometer word) is deliberately NOT read:
        // on board 8C9C 0xC4 is a frozen register that reports a constant
        // 1249 rpm, and nothing consumed the value.
        Ok::<(), hp_ec::Error>(())
    })
    .ok()?;
    Some(s)
}
