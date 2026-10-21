//! hp-core: sensor aggregation, battery health, fan engine, config.

pub mod battery;
pub mod sensors;
pub mod config;
pub mod engine;

pub use battery::{BatteryInfo, read_battery};
pub use sensors::{ThermalSample, read_thermal_ec, read_thermal_wmi};
pub use config::Config;
