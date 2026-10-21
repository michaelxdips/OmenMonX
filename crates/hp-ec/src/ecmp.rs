//! ECMP MMIO page (0xFC7E0000, 0x1000 bytes) — the EC RAM mirror.
//!
//! The LpcACPIEC PawnIO module only whitelists ports 0x62/0x66, so MMIO reads
//! are done through the ERAM interface (the EC mirrors the same logical layout)
//! rather than a separate memory driver. This module provides the field-map
//! API over ERAM addresses matching the ACPI ECMP offsets for the low page.

use crate::ecio::EcIo;
use crate::eram::Eram;
use crate::error::Result;
use crate::fields;

/// Reader for the fan/thermal field block of the EC mirror.
pub struct Ecmp<'a> {
    eram: Eram<'a>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ThermalSnapshot {
    pub cpu_temp: u8,
    pub gpu_temp: u8,
    pub gpu_hotspot: u8,
}

impl<'a> Ecmp<'a> {
    /// Caller must hold the EC mutex.
    pub fn new(io: &'a EcIo) -> Self {
        Self { eram: Eram::new(io) }
    }

    pub fn read_byte(&self, offset: u8) -> Result<u8> {
        self.eram.read(offset)
    }

    pub fn read_snapshot(&self) -> Result<ThermalSnapshot> {
        Ok(ThermalSnapshot {
            cpu_temp: self.eram.read(fields::CPTV as u8)?,
            gpu_temp: self.eram.read(fields::GPTV as u8)?,
            gpu_hotspot: self.eram.read(fields::GHSV as u8)?,
        })
    }
}
