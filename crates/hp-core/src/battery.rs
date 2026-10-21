//! Battery telemetry: health, wear, charge state, cycle count.
//! Uses the `wmi` crate for root\wmi + CIMV2 queries.

use serde::Deserialize;
use wmi::{COMLibrary, WMIConnection};

#[derive(Debug, Clone, Default)]
pub struct BatteryInfo {
    pub full_charge_mwh: Option<u32>,
    pub design_mwh: Option<u32>,
    pub health_pct: Option<u8>,
    pub charge_pct: Option<u8>,
    pub cycles: Option<u32>,
    pub voltage_mv: Option<u32>,
    pub rate_mw: Option<i32>,
    pub ac_online: bool,
    pub charging: bool,
    pub temp_c: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryFullChargedCapacity")]
struct WmiFullCharge {
    #[serde(rename = "FullChargedCapacity")]
    full: u32,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryStaticData")]
struct WmiStatic {
    #[serde(rename = "DesignedCapacity")]
    designed: u32,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryCycleCount")]
struct WmiCycles {
    #[serde(rename = "CycleCount")]
    cycles: u32,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryStatus")]
struct WmiStatus {
    #[serde(rename = "Voltage")]
    voltage: u32,
    #[serde(rename = "ChargeRate")]
    charge_rate: i32,
    #[serde(rename = "DischargeRate")]
    discharge_rate: i32,
    #[serde(rename = "Charging")]
    charging: bool,
    #[serde(rename = "PowerOnline")]
    power_online: bool,
    #[serde(rename = "RemainingCapacity")]
    remaining_capacity: u32,
}

#[derive(Deserialize)]
#[serde(rename = "BatteryTemperature")]
struct WmiTemp {
    #[serde(rename = "Temperature")]
    temperature: u32,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_Battery")]
struct Win32Battery {
    #[serde(rename = "EstimatedChargeRemaining")]
    charge_pct: u8,
}

fn first<T>(res: Result<Vec<T>, wmi::WMIError>) -> Option<T> {
    res.ok().and_then(|v| v.into_iter().next())
}

/// Read all battery telemetry. Never panics; missing sources are None.
pub fn read_battery() -> BatteryInfo {
    let mut info = BatteryInfo::default();

    // root\wmi connection (battery classes) — separate COM init per call is
    // acceptable for CLI cadence; the daemon reuses one connection.
    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(_) => return info,
    };
    let wmi_ns = match WMIConnection::with_namespace_path("ROOT\\WMI", com) {
        Ok(s) => s,
        Err(_) => return info,
    };

    info.full_charge_mwh = first(wmi_ns.raw_query::<WmiFullCharge>("SELECT FullChargedCapacity FROM BatteryFullChargedCapacity")).map(|b| b.full);
    info.design_mwh = first(wmi_ns.raw_query::<WmiStatic>("SELECT DesignedCapacity FROM BatteryStaticData")).map(|b| b.designed);
    info.cycles = first(wmi_ns.raw_query::<WmiCycles>("SELECT CycleCount FROM BatteryCycleCount")).map(|b| b.cycles);
    if let Some(st) = first(wmi_ns.raw_query::<WmiStatus>("SELECT Voltage, ChargeRate, DischargeRate, Charging, PowerOnline, RemainingCapacity FROM BatteryStatus")) {
        info.voltage_mv = Some(st.voltage);
        // rate: positive = charging, negative = draining
        info.rate_mw = Some(if st.charging { st.charge_rate.max(st.discharge_rate) } else { -st.discharge_rate });
        info.charging = st.charging;
        info.ac_online = st.power_online;
        // charge % from remaining vs full
        if let Some(f) = info.full_charge_mwh {
            if f > 0 {
                info.charge_pct = Some(((st.remaining_capacity as f64 / f as f64) * 100.0).min(100.0) as u8);
            }
        }
    }
    if let Some(t) = first(wmi_ns.raw_query::<WmiTemp>("SELECT Temperature FROM BatteryTemperature")) {
        info.temp_c = Some((t.temperature as f32 - 2731.0) / 10.0);
    }

    // charge % + cimv2 fallback
    if let Ok(com2) = COMLibrary::new() {
        if let Ok(cimv2) = WMIConnection::with_namespace_path("ROOT\\CIMV2", com2) {
            if let Some(b) = first(cimv2.raw_query::<Win32Battery>("SELECT EstimatedChargeRemaining FROM Win32_Battery")) {
                info.charge_pct = Some(b.charge_pct);
            }
        }
    }

    if let (Some(f), Some(d)) = (info.full_charge_mwh, info.design_mwh) {
        if d > 0 {
            info.health_pct = Some(((f as f64 / d as f64) * 100.0).min(100.0) as u8);
        }
    }
    info
}
