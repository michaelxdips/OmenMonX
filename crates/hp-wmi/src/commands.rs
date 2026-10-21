//! Typed BIOS commands. Each maps to (Command, CommandType, in-payload, out-size).

use crate::payload::{Request, Response, CMD_DEFAULT, CMD_KEYBOARD, CMD_LEGACY};
use crate::Result;
use crate::session::HpWmi;

pub struct Bios {
    session: HpWmi,
}

impl Bios {
    pub fn connect() -> Result<Self> {
        Ok(Self { session: HpWmi::connect()? })
    }

    fn ok(&self, req: Request, out_size: usize) -> Result<Response> {
        let resp = self.session.send(&req, out_size)?;
        if !resp.is_ok() {
            return Err(crate::Error::BiosError {
                code: resp.return_code,
                command: req.command,
                command_type: req.command_type,
            });
        }
        Ok(resp)
    }

    // ---- System / capability ----

    /// 0x28 GetSystem â€” support flags (fan ctl, extreme mode, GPU switch bits).
    pub fn get_system(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x28), 128)
    }

    /// 0x10 GetFanCount.
    pub fn get_fan_count(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x10), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no fan count byte"))
    }

    /// 0x2C GetFanType â€” nibbles: 1 CPU, 2 GPU, 3 exhaust, 4 pump, 5 intake.
    pub fn get_fan_type(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2C), 128)
    }

    // ---- Fans ----

    /// 0x2D GetFanLevel â€” per-fan current level.
    pub fn get_fan_level(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2D), 128)
    }

    /// 0x2E SetFanLevel â€” {fan1_level, fan2_level, ...}.
    pub fn set_fan_level(&self, levels: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x2E, levels), 0)?;
        Ok(())
    }

    /// 0x26 GetMaxFan.
    pub fn get_max_fan(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x26), 4)
    }

    /// 0x27 SetMaxFan â€” {0 = off, 1 = on} per OmenMon semantics.
    pub fn set_max_fan(&self, on: bool) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x27, &[u8::from(on), 0, 0, 0]), 0)?;
        Ok(())
    }

    /// 0x1A SetFanMode â€” e.g. 0x30 default, 0x31 performance, 0x50 cool.
    pub fn set_fan_mode(&self, mode: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x1A, &[0xFF, mode, 0, 0]), 0)?;
        Ok(())
    }

    /// 0x1A SetFanMode with arbitrary payload (experimentation).
    pub fn set_fan_mode_raw(&self, data: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x1A, data), 0)?;
        Ok(())
    }

    /// 0x2F GetFanTable.
    pub fn get_fan_table(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2F), 128)
    }

    /// 0x2B GetKbdType â€” 0 std, 1 numpad, 2 TKL, 3 per-key RGB.
    pub fn get_kbd_type(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x2B), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no kbd type byte"))
    }

    // ---- Temperatures ----

    /// 0x23 GetTemperature â€” requires an input payload (sensor selector).
    /// Candidates validated in M1; {0x01,0,0,0} first.
    pub fn get_temperature(&self, sensor: u8) -> Result<Response> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x23, &[sensor, 0, 0, 0]), 4)
    }

    // ---- Keyboard ----

    /// Keyboard 0x20009 subtype 1 â€” HasBacklight (bit0 of byte0).
    pub fn has_backlight(&self) -> Result<bool> {
        let r = self.ok(Request::new(CMD_KEYBOARD, 0x01), 4)?;
        Ok(r.byte0().map(|b| b & 1 == 1).unwrap_or(false))
    }

    /// Keyboard subtype 2 â€” GetColorTable (128 B: ZoneCount + zones Ã— RGB).
    pub fn get_color_table(&self) -> Result<Response> {
        self.ok(Request::new(CMD_KEYBOARD, 0x02), 128)
    }

    /// Keyboard subtype 3 â€” SetColorTable (full 128 B table).
    pub fn set_color_table(&self, table: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_KEYBOARD, 0x03, table), 0)?;
        Ok(())
    }

    /// Keyboard subtype 4 â€” GetBacklight (0x64 off, 0xE4 on).
    pub fn get_backlight(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_KEYBOARD, 0x04), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no backlight byte"))
    }

    /// Keyboard subtype 5 â€” SetBacklight.
    pub fn set_backlight(&self, value: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_KEYBOARD, 0x05, &[value, 0, 0, 0]), 0)?;
        Ok(())
    }

    // ---- GPU ----

    /// 0x21 GetGpuPower â€” CustomTgp, Ppab, DState, PeakTemp.
    pub fn get_gpu_power(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x21), 4)
    }

    /// 0x22 SetGpuPower â€” raw 4-byte payload.
    pub fn set_gpu_power(&self, data: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x22, data), 0)?;
        Ok(())
    }

    /// Legacy 0x52 GetGpuMode â€” 0 hybrid, 1 discrete, 2 optimus.
    pub fn get_gpu_mode(&self) -> Result<Response> {
        self.ok(Request::new(CMD_LEGACY, 0x52), 4)
    }

    /// Legacy 0x52 SetGpuMode (reboot required).
    pub fn set_gpu_mode(&self, mode: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_LEGACY, 0x52, &[mode, 0, 0, 0]), 0)?;
        Ok(())
    }

    // ---- CPU ----

    /// 0x29 SetCpuPower â€” {PL1, PL2, PL4, WithGpu}.
    pub fn set_cpu_power(&self, pl1: u32, pl2: u32, pl4: u32, with_gpu: u32) -> Result<()> {
        let mut d = Vec::with_capacity(16);
        d.extend_from_slice(&pl1.to_le_bytes());
        d.extend_from_slice(&pl2.to_le_bytes());
        d.extend_from_slice(&pl4.to_le_bytes());
        d.extend_from_slice(&with_gpu.to_le_bytes());
        self.ok(Request::with_data(CMD_DEFAULT, 0x29, &d), 0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_size_mapping() {
        assert_eq!(crate::session::method_for_size(4), "hpqBIOSInt4");
        assert_eq!(crate::session::method_for_size(128), "hpqBIOSInt128");
    }
}


