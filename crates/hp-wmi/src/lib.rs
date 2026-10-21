//! hp-wmi: HP WMI BIOS interface (hpqBIntM) — the primary hardware control path.
//!
//! Proven working on HP Victus 16 (board 8C9C, BIOS F.17) and compatible with
//! HP Omen boards (8A13/8A14) that OmenMon targets.

mod error;
pub mod bios_settings;
pub mod payload;
mod commands;
pub mod session;

pub use bios_settings::{BiosSetting, BiosSettings, SetResult};
pub use commands::*;
pub use error::{Error, Result};
pub use session::HpWmi;
