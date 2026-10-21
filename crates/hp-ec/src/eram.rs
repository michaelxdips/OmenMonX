//! Classic ACPI EmbeddedController register access (ERAM, 256 B page 0).
//!
//! Protocol per register access (Insyde EC):
//!   wait IBF (status bit1) clear -> write reg index to EC_CMD (0x66) ->
//!   wait OBF/IBF -> read/write EC_DATA (0x62)

use crate::ecio::{EcIo, EC_CMD, EC_DATA};
use crate::error::Result;

/// Status register bits (read from 0x66 before it receives the index).
const OBF: u8 = 1 << 0; // output buffer full (data ready to read)
const IBF: u8 = 1 << 1; // input buffer full (EC busy)

const SPIN_MAX: u32 = 100_000;

pub struct Eram<'a> {
    io: &'a EcIo,
}

impl<'a> Eram<'a> {
    pub fn new(io: &'a EcIo) -> Self {
        Self { io }
    }

    fn spin_status(&self, mask: u8, want_clear: bool) -> Result<Option<u8>> {
        for _ in 0..SPIN_MAX {
            let st = self.io.port_read(EC_CMD)?;
            let busy = (st & mask) != 0;
            if busy == !want_clear && busy == false || (!busy) == want_clear {
                return Ok(Some(st));
            }
            if want_clear && (st & mask) == 0 {
                return Ok(Some(st));
            }
            if !want_clear && (st & mask) != 0 {
                return Ok(Some(st));
            }
        }
        Ok(None)
    }

    fn wait_ibf_clear(&self) -> Result<()> {
        for _ in 0..SPIN_MAX {
            if self.io.port_read(EC_CMD)? & IBF == 0 {
                return Ok(());
            }
        }
        Err(crate::Error::Timeout { what: "IBF clear", ms_ms: 1000 })
    }

    fn wait_obf_set(&self) -> Result<()> {
        for _ in 0..SPIN_MAX {
            if self.io.port_read(EC_CMD)? & OBF != 0 {
                return Ok(());
            }
        }
        Err(crate::Error::Timeout { what: "OBF set", ms_ms: 1000 })
    }

    /// Read one ERAM byte at `reg` (0x00–0xFF). Caller must hold the EC mutex.
    ///
    /// Uses the Insyde command-based protocol (RD_EC 0x80 to 0x66, index to
    /// 0x62) and retries up to 3 times on bus timeout.
    pub fn read(&self, reg: u8) -> Result<u8> {
        let mut last = crate::Error::Timeout { what: "OBF set", ms_ms: 0 };
        for _ in 0..3 {
            match self.read_once(reg) {
                Ok(v) => return Ok(v),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn read_once(&self, reg: u8) -> Result<u8> {
        // Command-based (Insyde/ITE style)
        self.wait_ibf_clear()?;
        self.io.port_write(EC_CMD, 0x80)?;
        self.wait_ibf_clear()?;
        self.io.port_write(EC_DATA, reg)?;
        self.wait_obf_set()?;
        self.io.port_read(EC_DATA)
    }

    /// Write one ERAM byte at `reg`. Caller must hold the EC mutex.
    pub fn write(&self, reg: u8, value: u8) -> Result<()> {
        // Command-based (WR_EC 0x81)
        self.wait_ibf_clear()?;
        self.io.port_write(EC_CMD, 0x81)?;
        self.wait_ibf_clear()?;
        self.io.port_write(EC_DATA, reg)?;
        self.wait_ibf_clear()?;
        self.io.port_write(EC_DATA, value)?;
        Ok(())
    }

    /// Read a range of registers (with per-byte mutex already held).
    pub fn read_range(&self, start: u8, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut reg = start;
        for _ in 0..len {
            out.push(self.read(reg)?);
            reg = reg.wrapping_add(1);
        }
        Ok(out)
    }
}
