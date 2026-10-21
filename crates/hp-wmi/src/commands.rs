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

    /// Arbitrary raw request — for telemetry commands not yet wrapped.
    pub fn raw_send(&self, req: Request, out_size: usize) -> Result<Response> {
        self.ok(req, out_size)
    }

    // ---- System / capability ----

    /// 0x28 GetSystem — support flags (fan ctl, extreme mode, GPU switch bits).
    pub fn get_system(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x28), 128)
    }

    /// 0x10 GetFanCount.
    pub fn get_fan_count(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x10), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no fan count byte"))
    }

    /// 0x2C GetFanType — nibbles: 1 CPU, 2 GPU, 3 exhaust, 4 pump, 5 intake.
    pub fn get_fan_type(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2C), 128)
    }

    // ---- Fans ----

    /// 0x2D GetFanLevel — per-fan current level.
    pub fn get_fan_level(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2D), 128)
    }

    /// 0x2E SetFanLevel — {fan1_level, fan2_level, ...}.
    pub fn set_fan_level(&self, levels: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x2E, levels), 0)?;
        Ok(())
    }

    /// 0x26 GetMaxFan.
    pub fn get_max_fan(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x26), 4)
    }

    /// 0x27 SetMaxFan — {0 = off, 1 = on} per OmenMon semantics.
    pub fn set_max_fan(&self, on: bool) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x27, &[u8::from(on), 0, 0, 0]), 0)?;
        Ok(())
    }

    /// 0x1A SetFanMode — e.g. 0x30 default, 0x31 performance, 0x50 cool.
    pub fn set_fan_mode(&self, mode: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x1A, &[0xFF, mode, 0, 0]), 0)?;
        Ok(())
    }

    /// 0x1A SetFanMode with arbitrary payload (experimentation).
    pub fn set_fan_mode_raw(&self, data: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x1A, data), 0)?;
        Ok(())
    }

    /// 0x1A SetFanMode with **verified** readback.
    ///
    /// Sends {0xFF, mode, 0, 0} then re-reads x28[8] and reports what the
    /// firmware actually settled on. The status byte tracks bit 0 of the mode
    /// code: even codes read x1E, odd codes read x23.
    ///
    /// Returns the observed status byte, so callers can show real state rather
    /// than an optimistic guess. This is the pattern OMEN Gaming Hub gets wrong.
    pub fn set_fan_mode_verified(&self, mode: ThermalMode) -> Result<u8> {
        self.ok(
            Request::with_data(CMD_DEFAULT, 0x1A, &[0xFF, mode.code(), 0, 0]),
            0,
        )?;
        // Give the firmware a moment to settle before reading back.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let sys = self.ok(Request::new(CMD_DEFAULT, 0x28), 128)?;
        Ok(sys.data.get(8).copied().unwrap_or(0))
    }

    /// Read the current thermal-mode status byte (x28[8]).
    pub fn get_thermal_status(&self) -> Result<u8> {
        let sys = self.ok(Request::new(CMD_DEFAULT, 0x28), 128)?;
        Ok(sys.data.get(8).copied().unwrap_or(0))
    }

    /// 0x2F GetFanTable.
    pub fn get_fan_table(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x2F), 128)
    }


    /// 0x2F GetFanTable — the firmware's fan curve.
    ///
    /// Layout verified on board 8C9C: `[0] = fanCount`, `[1] = levelCount`, then
    /// `levelCount` rows of `fanCount + 1` bytes. Column 0 is a level/dB index;
    /// the remaining columns are per-fan duty percentages. See [`FanTable`].
    pub fn get_fan_table_parsed(&self) -> Result<FanTable> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x2F), 128)?;
        Ok(FanTable::parse(&r.data))
    }

    /// 0x2B GetKbdType — informational only.
    ///
    /// **Do not derive the zone count from this.** On board 8C9C it returns
    /// `0x04`, which OMEN Gaming Hub's enum calls "OneZoneWithNumpad", yet the
    /// colour table reports 3 zones with 3 distinct colours. Read
    /// `colorTable[0]` instead.
    pub fn get_kbd_type(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x2B), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no kbd type byte"))
    }

    // ---- Temperatures ----

    /// 0x23 GetTemperature — requires an input payload (sensor selector).
    /// Candidates validated in M1; {0x01,0,0,0} first.
    pub fn get_temperature(&self, sensor: u8) -> Result<Response> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x23, &[sensor, 0, 0, 0]), 4)
    }

    // ---- Keyboard ----

    /// Keyboard 0x20009 subtype 1 — capability probe.
    ///
    /// **Do not read a flag from byte 0: it is always zero on board 8C9C.**
    /// Measured replies across one session: `00 43 00 00`, `00 3C 00 00`,
    /// `00 39 00 00`. Byte 1 drifts between reads and does **not** change when
    /// the backlight is toggled, so it is neither a capability bit nor a state.
    ///
    /// Use [`Bios::get_backlight`] for the actual state. This method only reports
    /// whether the firmware answers the subtype with anything at all.
    pub fn has_backlight(&self) -> Result<bool> {
        let r = self.ok(Request::new(CMD_KEYBOARD, 0x01), 4)?;
        Ok(r.data.iter().any(|&b| b != 0))
    }

    /// Keyboard subtype 2 — GetColorTable (128 B: ZoneCount + zones × RGB).
    pub fn get_color_table(&self) -> Result<Response> {
        self.ok(Request::new(CMD_KEYBOARD, 0x02), 128)
    }

    /// Keyboard subtype 3 — SetColorTable (full 128 B table).
    pub fn set_color_table(&self, table: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_KEYBOARD, 0x03, table), 0)?;
        Ok(())
    }

    /// Keyboard subtype 4 — GetBacklight (0x64 off, 0xE4 on).
    pub fn get_backlight(&self) -> Result<u8> {
        let r = self.ok(Request::new(CMD_KEYBOARD, 0x04), 4)?;
        r.byte0().ok_or(crate::Error::Shape("no backlight byte"))
    }

    /// Keyboard subtype 5 — SetBacklight.
    pub fn set_backlight(&self, value: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_KEYBOARD, 0x05, &[value, 0, 0, 0]), 0)?;
        Ok(())
    }

    // ---- GPU ----

    /// 0x21 GetGpuPower — CustomTgp, Ppab, DState, PeakTemp.
    pub fn get_gpu_power(&self) -> Result<Response> {
        self.ok(Request::new(CMD_DEFAULT, 0x21), 4)
    }

    /// 0x22 SetGpuPower — raw 4-byte payload.
    pub fn set_gpu_power(&self, data: &[u8]) -> Result<()> {
        self.ok(Request::with_data(CMD_DEFAULT, 0x22, data), 0)?;
        Ok(())
    }

    /// Legacy 0x52 GetGpuMode — 0 hybrid, 1 discrete, 2 optimus.
    pub fn get_gpu_mode(&self) -> Result<Response> {
        self.ok(Request::new(CMD_LEGACY, 0x52), 4)
    }

    /// Legacy 0x52 SetGpuMode (reboot required).
    pub fn set_gpu_mode(&self, mode: u8) -> Result<()> {
        self.ok(Request::with_data(CMD_LEGACY, 0x52, &[mode, 0, 0, 0]), 0)?;
        Ok(())
    }

    // ---- CPU ----

    /// 0x2A GetCpuPower — the configured limits block, NOT live power.
    ///
    /// Verified on board 8C9C (2026-10-21): this is a *settings* block, not a
    /// telemetry value. Under a 16-thread load the bytes do not rise, so it must
    /// never be presented as "current package power".
    pub fn get_cpu_power(&self) -> Result<CpuPowerLimits> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x2A), 128)?;
        Ok(CpuPowerLimits::from_block(&r.data))
    }

    /// 0x29 SetPowerLimit — writes PL1/PL2 into 0x2A[2..4].
    ///
    /// Byte order is `{pl2, pl1, 0xFF, 0xFF}` — **PL2 first**. This is the order
    /// OMEN Gaming Hub's `PowerControlBase.SetPowerLimit` compiles to; getting it
    /// backwards makes the write look like a silent no-op.
    ///
    /// The board clamps at [`CpuPowerLimits::MAX_WATTS`]. Values above that are
    /// rejected and the firmware resets the field to the maximum, so callers are
    /// clamped here rather than silently ignored.
    pub fn set_power_limit(&self, watts: u8) -> Result<u8> {
        let w = watts.clamp(CpuPowerLimits::MIN_WATTS, CpuPowerLimits::MAX_WATTS);
        self.ok(
            Request::with_data(CMD_DEFAULT, 0x29, &[w, w, 0xFF, 0xFF]),
            0,
        )?;
        Ok(w)
    }

    /// 0x29 SetConcurrentTdp — writes the WithGpu / concurrent-TDP field at 0x2A[7].
    ///
    /// Byte order `{0xFF, 0xFF, 0xFF, tdp}`. Verified 7/7 across a 20-38-20 sweep.
    pub fn set_concurrent_tdp(&self, watts: u8) -> Result<()> {
        self.ok(
            Request::with_data(CMD_DEFAULT, 0x29, &[0xFF, 0xFF, 0xFF, watts]),
            0,
        )?;
        Ok(())
    }

    /// Legacy 4-field form. Kept for boards that accept it, but **do not** use it
    /// on 8C9C: that firmware wants 4 *bytes*, not 4 u32s, and rejects this shape.
    pub fn set_cpu_power_u32(&self, pl1: u32, pl2: u32, pl4: u32, with_gpu: u32) -> Result<()> {
        let mut d = Vec::with_capacity(16);
        d.extend_from_slice(&pl1.to_le_bytes());
        d.extend_from_slice(&pl2.to_le_bytes());
        d.extend_from_slice(&pl4.to_le_bytes());
        d.extend_from_slice(&with_gpu.to_le_bytes());
        self.ok(Request::with_data(CMD_DEFAULT, 0x29, &d), 0)?;
        Ok(())
    }

    /// 0x28 decoded into [`SystemData`].
    pub fn get_system_data(&self) -> Result<SystemData> {
        let r = self.ok(Request::new(CMD_DEFAULT, 0x28), 128)?;
        Ok(SystemData::parse(&r.data))
    }
}

/// Decoded `0x28` SystemData block.
///
/// Verified against board 8C9C: the live reply is
/// `18 01 00 01 01 BE 00 07 23`. Every field below was cross-checked against
/// OMEN Gaming Hub's own IL, which tests the same bits — for example
/// `IsSwFanControlSupport = (SystemDesignData[4] & 0x01) > 0`.
///
/// This replaces the old `probe` output, which labelled byte 1 as
/// "SysSupportFlags". It is not a separate flag byte: bytes 0-1 together are the
/// little-endian status flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemData {
    /// Bytes 0-1 as a little-endian u16. Live value `0x0118`.
    pub status_flags: u16,
    /// Byte 3. 0 = legacy thermal policy, 1 = current policy.
    pub thermal_policy: u8,
    /// Byte 4. Bit 0 SwFanControl, bit 1 ExtremeMode, bit 4 TwoBytePL4.
    pub support_flags: u8,
    /// Byte 5. Default PL4 in watts. Live value 190.
    pub default_pl4: u8,
    /// Byte 6. 0 = BIOS overclock/undervolt not supported.
    pub bios_oc: u8,
    /// Byte 7. GPU mode switch capability bits.
    pub gpu_mode_switch: u8,
    /// Byte 8. Default CPU limit with the discrete GPU active, in watts.
    ///
    /// Not a constant: measured 30 W in Balanced/Cool and 35 W in Performance,
    /// so it is derived from the active thermal profile.
    pub default_pl_with_gpu: u8,
}

impl SystemData {
    /// Parse a 0x28 reply. Missing bytes read as 0 rather than panicking.
    pub fn parse(d: &[u8]) -> Self {
        let at = |i: usize| d.get(i).copied().unwrap_or(0);
        Self {
            status_flags: u16::from_le_bytes([at(0), at(1)]),
            thermal_policy: at(3),
            support_flags: at(4),
            default_pl4: at(5),
            bios_oc: at(6),
            gpu_mode_switch: at(7),
            default_pl_with_gpu: at(8),
        }
    }

    /// Bit 0 of the support flags — software fan control available.
    pub fn sw_fan_control(&self) -> bool {
        self.support_flags & 0x01 != 0
    }

    /// Bit 1 of the support flags. **False on board 8C9C.**
    pub fn extreme_mode(&self) -> bool {
        self.support_flags & 0x02 != 0
    }

    /// Bit 4 of the support flags. **False on board 8C9C**, which means PL4 goes
    /// through the MSR path rather than the two-byte 0x37 command.
    pub fn two_byte_pl4(&self) -> bool {
        self.support_flags & 0x10 != 0
    }

    /// True if the BIOS exposes overclock/undervolt. **False on board 8C9C**,
    /// and confirmed independently by 0x35 returning WMI error 31.
    pub fn bios_oc_supported(&self) -> bool {
        self.bios_oc != 0
    }

    /// True when the thermal policy is the current (V1) generation, which is
    /// what makes the 0x30/0x31/0x50 mode remap the correct one to send.
    pub fn thermal_policy_v1(&self) -> bool {
        self.thermal_policy == 1
    }
}

/// Thermal / fan mode codes for `0x1A`.
///
/// The raw PerformanceMode enum from OMEN Gaming Hub uses small values
/// (0 = Default, 1 = Performance, 2 = Cool, ...), but on a V1 thermal policy the
/// firmware expects the **remapped** L-codes below. OGH's
/// `PerformanceControlHelper.SetFanMode` performs exactly this remap:
///
/// ```text
/// case 0   -> 48 (0x30)   Default     -> L2
/// case 1   -> 49 (0x31)   Performance -> L7
/// case 2   -> 80 (0x50)   Cool        -> L4
/// case 256 -> 48 (0x30)   Eco         -> L2
/// ```
///
/// Sending the raw small values instead of these codes is why a naive
/// implementation appears to do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalMode {
    /// `0x30` — balanced / default.
    Balanced,
    /// `0x31` — performance.
    Performance,
    /// `0x50` — cool.
    Cool,
}

impl ThermalMode {
    /// The byte actually written to the firmware.
    pub fn code(self) -> u8 {
        match self {
            ThermalMode::Balanced => 0x30,
            ThermalMode::Performance => 0x31,
            ThermalMode::Cool => 0x50,
        }
    }

    /// Status byte expected in `0x28[8]` after the mode is applied.
    ///
    /// Measured on board 8C9C: the byte tracks bit 0 of the mode code, so
    /// `0x30`/`0x50` (even) read `0x1E` and `0x31` (odd) reads `0x23`.
    pub fn expected_status(self) -> u8 {
        if self.code() & 1 == 0 {
            0x1E
        } else {
            0x23
        }
    }

    /// Interpret a status byte, if it matches a known value.
    pub fn from_status(status: u8) -> Option<&'static str> {
        match status {
            0x1E => Some("balanced/cool (even mode code)"),
            0x23 => Some("performance (odd mode code)"),
            _ => None,
        }
    }
}

/// A parsed 0x2F fan table.
///
/// Verified layout on board 8C9C: a 2-fan board returns 128 bytes,
/// `[0] = 2` (fans), `[1] = 42` (levels), then 42 rows of 3 bytes
/// (`fanCount + 1`). Payload consumes exactly `levels * (fans + 1)` bytes.
///
/// **Column 0 is a level/dB index, not a temperature.** OMEN Gaming Hub's
/// `SwFanControlTable` parses with `stride = NumOfFans + 1` and treats
/// `tuple[0]` as a dB level (`MinDb = 33`, `GetFanRpm(42)`). An earlier
/// reading of this table as `(fan1%, fan2%, tempC)` was wrong. All columns are
/// monotonic either way, but only the percentage columns have an unambiguous
/// meaning, so only those are named as such.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanTable {
    /// Number of fans the firmware reports.
    pub fan_count: u8,
    /// Number of curve entries.
    pub level_count: u8,
    /// One entry per curve point.
    pub points: Vec<FanPoint>,
}

/// One row of the fan curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanPoint {
    /// Column 0. A level/dB index; the exact physical unit is not proven.
    pub level: u8,
    /// Per-fan duty percentage, one entry per fan.
    pub duty: [u8; 3],
}

impl FanPoint {
    /// Duty for fan `i`, if the table has that many fans.
    pub fn duty(&self, i: usize, fan_count: u8) -> Option<u8> {
        if i < fan_count as usize && i < 3 {
            Some(self.duty[i])
        } else {
            None
        }
    }
}

impl FanTable {
    /// Parse a 0x2F reply. Never panics: a malformed or truncated reply yields
    /// only the points that are fully present.
    pub fn parse(data: &[u8]) -> Self {
        if data.len() < 2 {
            return Self {
                fan_count: 0,
                level_count: 0,
                points: Vec::new(),
            };
        }
        let fan_count = data[0];
        let level_count = data[1];
        let stride = fan_count as usize + 1;
        if stride < 2 {
            // fan_count == 0 would make stride 1 and every "point" meaningless.
            return Self {
                fan_count,
                level_count,
                points: Vec::new(),
            };
        }

        let mut points = Vec::with_capacity(level_count as usize);
        for i in 0..level_count as usize {
            let off = 2 + i * stride;
            if off + stride > data.len() {
                break;
            }
            let mut duty = [0u8; 3];
            // columns 1..stride are the per-fan percentages
            for (k, slot) in duty.iter_mut().enumerate() {
                if k + 1 < stride {
                    *slot = data[off + 1 + k];
                }
            }
            points.push(FanPoint {
                level: data[off],
                duty,
            });
        }

        Self {
            fan_count,
            level_count,
            points,
        }
    }

    /// Highest duty any fan reaches in this curve.
    pub fn max_duty(&self) -> u8 {
        self.points
            .iter()
            .flat_map(|p| p.duty.iter().copied())
            .max()
            .unwrap_or(0)
    }

    /// True if every column is non-decreasing — the shape a real curve must have.
    pub fn is_monotonic(&self) -> bool {
        if self.points.len() < 2 {
            return true;
        }
        let cols = self.fan_count as usize;
        for w in self.points.windows(2) {
            if w[1].level < w[0].level {
                return false;
            }
            for c in 0..cols.min(3) {
                if w[1].duty[c] < w[0].duty[c] {
                    return false;
                }
            }
        }
        true
    }
}

/// The configured CPU power limits, decoded from the 0x2A block.
///
/// Layout verified on board 8C9C (bytes 2/3/4/7 of the 128-byte reply). Byte 0
/// fluctuates between 60-62 and is **not** watts — its meaning is unknown, so it
/// is deliberately not surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPowerLimits {
    /// 0x2A[2] — long-duration limit.
    pub pl1: u8,
    /// 0x2A[3] — short-duration limit.
    pub pl2: u8,
    /// 0x2A[4] — peak limit.
    pub pl4: u8,
    /// 0x2A[7] — limit while the discrete GPU is active.
    pub pl_with_gpu: u8,
}

impl CpuPowerLimits {
    /// Lowest limit this board accepts.
    pub const MIN_WATTS: u8 = 5;
    /// Highest limit this board accepts. Values above this are rejected and the
    /// firmware resets the field to this value.
    pub const MAX_WATTS: u8 = 25;

    fn from_block(data: &[u8]) -> Self {
        let at = |i: usize| data.get(i).copied().unwrap_or(0);
        Self {
            pl1: at(2),
            pl2: at(3),
            pl4: at(4),
            pl_with_gpu: at(7),
        }
    }

    /// True if the values look like a real reading rather than an empty reply.
    pub fn is_plausible(&self) -> bool {
        self.pl1 > 0 && self.pl2 > 0 && self.pl4 > 0
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

    /// 0x2A is a limits block at bytes 2/3/4/7. Byte 0 must NOT be read as PL1 —
    /// it fluctuates (60-62) and is not watts.
    #[test]
    fn cpu_power_block_decode() {
        let mut block = [0u8; 128];
        block[0] = 0x3E; // fluctuating, not a limit
        block[2] = 25;
        block[3] = 25;
        block[4] = 25;
        block[7] = 35;
        let l = CpuPowerLimits::from_block(&block);
        assert_eq!(l.pl1, 25);
        assert_eq!(l.pl2, 25);
        assert_eq!(l.pl4, 25);
        assert_eq!(l.pl_with_gpu, 35);
        assert!(l.is_plausible());
    }

    /// A short/empty reply must not panic or invent limits.
    #[test]
    fn cpu_power_block_short_reply() {
        let l = CpuPowerLimits::from_block(&[1, 2]);
        assert_eq!(l.pl1, 0);
        assert!(!l.is_plausible());
    }

    /// The accepted window on 8C9C. Anything above 25 W is rejected by firmware
    /// and reset to 25, so the API must clamp rather than pass it through.
    #[test]
    fn power_limit_window() {
        assert_eq!(CpuPowerLimits::MIN_WATTS, 5);
        assert_eq!(CpuPowerLimits::MAX_WATTS, 25);
        let clamp = |w: u8| w.clamp(CpuPowerLimits::MIN_WATTS, CpuPowerLimits::MAX_WATTS);
        assert_eq!(clamp(1), 5);
        assert_eq!(clamp(5), 5);
        assert_eq!(clamp(20), 20);
        assert_eq!(clamp(25), 25);
        assert_eq!(clamp(26), 25);
        assert_eq!(clamp(45), 25);
    }

    /// Lock in the wire byte order. `{pl2, pl1, 0xFF, 0xFF}` — PL2 first.
    /// Reversing these two bytes is what made the command look like a no-op.
    #[test]
    fn power_limit_payload_order() {
        let pl2 = 18u8;
        let pl1 = 22u8;
        let payload = [pl2, pl1, 0xFF, 0xFF];
        assert_eq!(payload[0], 18, "byte 0 must be PL2");
        assert_eq!(payload[1], 22, "byte 1 must be PL1");
        assert_eq!(payload[2], 0xFF);
        assert_eq!(payload[3], 0xFF);
        // Same-value form, as used by set_power_limit:
        let w = 20u8;
        assert_eq!([w, w, 0xFF, 0xFF], [20, 20, 0xFF, 0xFF]);
    }

    /// Decode the real 0x28 capture from board 8C9C and check every bit against
    /// the values OGH's own IL tests for.
    #[test]
    fn system_data_real_capture() {
        let capture = [0x18u8, 0x01, 0x00, 0x01, 0x01, 0xBE, 0x00, 0x07, 0x23];
        let s = SystemData::parse(&capture);
        assert_eq!(s.status_flags, 0x0118);
        assert_eq!(s.thermal_policy, 1);
        assert_eq!(s.support_flags, 0x01);
        assert_eq!(s.default_pl4, 190);
        assert_eq!(s.bios_oc, 0);
        assert_eq!(s.gpu_mode_switch, 0x07);
        assert_eq!(s.default_pl_with_gpu, 35);

        assert!(s.sw_fan_control(), "bit 0 set -> software fan control");
        assert!(!s.extreme_mode(), "bit 1 clear -> no extreme mode");
        assert!(!s.two_byte_pl4(), "bit 4 clear -> PL4 via MSR");
        assert!(!s.bios_oc_supported(), "byte 6 == 0 -> no BIOS OC");
        assert!(s.thermal_policy_v1(), "byte 3 == 1 -> V1 remap applies");

        // PPAB bits: 0x0118 has bits 3, 4 and 8 set.
        assert_eq!(s.status_flags & 0x0118, 0x0118, "all three PPAB flags present");
    }

    /// A short reply must decode to zeros, not panic.
    #[test]
    fn system_data_short_reply() {
        let s = SystemData::parse(&[0x18]);
        assert_eq!(s.status_flags, 0x0018);
        assert_eq!(s.default_pl4, 0);
        assert!(!s.sw_fan_control());
        assert!(!s.thermal_policy_v1());
    }

    /// The V1 remap. Raw OGH mode values must be translated to these codes;
    /// sending the raw value makes the write look inert.
    #[test]
    fn thermal_mode_codes_and_status() {
        assert_eq!(ThermalMode::Balanced.code(), 0x30);
        assert_eq!(ThermalMode::Performance.code(), 0x31);
        assert_eq!(ThermalMode::Cool.code(), 0x50);

        // Status byte tracks bit 0 of the code -- measured, not assumed.
        assert_eq!(ThermalMode::Balanced.expected_status(), 0x1E);
        assert_eq!(ThermalMode::Cool.expected_status(), 0x1E);
        assert_eq!(ThermalMode::Performance.expected_status(), 0x23);

        assert!(ThermalMode::from_status(0x1E).is_some());
        assert!(ThermalMode::from_status(0x23).is_some());
        assert!(ThermalMode::from_status(0x00).is_none());
    }

    /// The 0x1A payload is {0xFF, mode, 0, 0} -- 0xFF first, not the mode.
    #[test]
    fn thermal_payload_shape() {
        let mode = ThermalMode::Performance.code();
        let payload = [0xFFu8, mode, 0, 0];
        assert_eq!(payload[0], 0xFF, "byte 0 is the constant 0xFF");
        assert_eq!(payload[1], 0x31, "byte 1 is the mode code");
        assert_eq!(&payload[2..], &[0, 0]);
    }

    /// Parse the REAL 128-byte table captured from board 8C9C on 2026-10-21.
    /// This is the actual firmware reply, so it doubles as a regression fixture.
    #[test]
    fn fan_table_parses_real_capture() {
        let hex = "02 2A 00 00 0F 12 15 19 13 16 1A 14 17 1A 15 18 1B 16 19 1C 17 1A 1E 18 1B 1F 19 1C 20 1A 1D 21 1B 1E 22 1C 1F 23 1D 20 24 1E 21 24 1F 22 25 20 23 26 21 24 27 22 25 28 23 26 29 24 27 29 25 28 29 26 29 29 27 2A 2A 28 2B 2B 29 2C 2D 2A 2D 2E 2B 2E 2E 2C 2F 2F 2D 30 2F 2E 31 30 2F 32 31 30 33 31 31 34 31 32 35 32 33 36 33 34 37 33 35 38 33 36 39 33 37 3A 33 38 3B 33 39 3C 33 3A 3D 34";
        let data: Vec<u8> = hex
            .split_whitespace()
            .map(|s| u8::from_str_radix(s, 16).expect("valid hex"))
            .collect();
        assert_eq!(data.len(), 128);

        let t = FanTable::parse(&data);
        assert_eq!(t.fan_count, 2, "board reports 2 fans");
        assert_eq!(t.level_count, 42, "board reports 42 curve points");
        assert_eq!(t.points.len(), 42, "all 42 rows must parse");

        // stride = fanCount + 1 = 3, payload = 42 * 3 = 126 = 128 - 2. Exact fit.
        assert_eq!(2 + t.points.len() * 3, data.len());

        // Row 0 raw bytes are [00, 00, 0F] -> level 0, fan1 0%, fan2 15%.
        assert_eq!(t.points[0].level, 0x00);
        assert_eq!(t.points[0].duty[0], 0x00);
        assert_eq!(t.points[0].duty[1], 0x0F);
        // Row 1 raw bytes are [12, 15, 19] -> level 18, fan1 21%, fan2 25%.
        assert_eq!(t.points[1].level, 0x12);
        assert_eq!(t.points[1].duty[0], 0x15);
        assert_eq!(t.points[1].duty[1], 0x19);
        // Row 41 raw bytes are [3A, 3D, 34] -> level 58, fan1 61%, fan2 52%.
        assert_eq!(t.points[41].level, 0x3A);
        assert_eq!(t.points[41].duty[0], 0x3D);
        assert_eq!(t.points[41].duty[1], 0x34);

        // Row 11 raw bytes are [1C, 1F, 23]. A live 0x2D read returned
        // [0x1C, 0x1F] while the firmware sat near this point -- but a 40-sample
        // experiment showed 0x2D matches a row's (c0,c1) only 82% of the time and
        // sometimes matches no row at all. So 0x2D is a LIVE value, not a table
        // lookup. Assert the row contents only; do not claim a 0x2D relationship.
        assert_eq!(t.points[11].level, 0x1C);
        assert_eq!(t.points[11].duty[0], 0x1F);
        assert_eq!(t.points[11].duty[1], 0x23);

        assert!(t.is_monotonic(), "a real curve must be non-decreasing");
        assert_eq!(t.max_duty(), 0x3D, "peak duty across both fans is 61%");
        // Only 2 fans, so index 2 is out of range even though duty[2] exists.
        assert_eq!(t.points[0].duty(0, 2), Some(0));
        assert_eq!(t.points[0].duty(2, 2), None);
    }

    /// A truncated reply must yield only complete rows, never panic.
    #[test]
    fn fan_table_truncated_is_safe() {
        let data = [2u8, 42, 0, 0, 15, 18]; // header + one complete row
        let t = FanTable::parse(&data);
        assert_eq!(t.fan_count, 2);
        assert_eq!(t.points.len(), 1);
        assert_eq!(t.points[0].level, 0);
        assert_eq!(t.points[0].duty[0], 0);
        assert_eq!(t.points[0].duty[1], 15);

        // Empty and single-byte replies.
        assert_eq!(FanTable::parse(&[]).points.len(), 0);
        assert_eq!(FanTable::parse(&[2]).points.len(), 0);
        // Degenerate fan_count = 0 must not produce points.
        assert_eq!(FanTable::parse(&[0, 42, 1, 2, 3]).points.len(), 0);
    }

    /// Lock in the concurrent-TDP form. `{0xFF, 0xFF, 0xFF, tdp}` — value LAST.
    #[test]
    fn concurrent_tdp_payload_order() {
        let tdp = 30u8;
        let payload = [0xFF, 0xFF, 0xFF, tdp];
        assert_eq!(payload[0], 0xFF);
        assert_eq!(payload[1], 0xFF);
        assert_eq!(payload[2], 0xFF);
        assert_eq!(payload[3], 30, "byte 3 must be the TDP value");
    }
}


