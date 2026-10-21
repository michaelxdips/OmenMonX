//! ECMP MMIO access via the (draft) PawnIO `Hpecmp` module.
//!
//! NOTE (draft, not wired): this file is intentionally UNREFERENCED — lib.rs
//! does not declare it yet. The module binary (Hpecmp.bin) does not exist
//! until the PawnIO.Modules PR is signed and merged; see
//! pawnio-hpecmp/README.md for the build recipe and PR text. Once
//! Hpecmp.bin is dropped next to LpcACPIEC.bin, add `pub mod ecmp_mmio;`
//! to lib.rs and flip `engine.rs`'s fan_write_active (see the comment at
//! the bottom of this file).
//!
//! Physical facts (verified, see pawnio-hpecmp/Hpecmp.p header):
//!   - ECMP page = physical 0xFC7E0000, 0x1000 bytes (ECCD opregion)
//!   - handshake @ 0x10: bit0 ECTB (1 = idle), bit1 ECTE (fires, clears)
//!   - ERN1-8 @ 0x11-0x18, ECMD @ 0x19, EDT1-5 @ 0x1A-0x1E
//!   - ERCD protocol: ECTB=1; ECMD=len; EDT1..n; ECTE=1; poll ECTE->0;
//!     results in ERN1..6. NOT reachable via ports 0x62/0x66 (verified).

use crate::error::{Error, Result};
use crate::pawnio::{PawnIo, is_installed};

/// Module binary name (drop the namazso-signed Hpecmp.bin into
/// crates/hp-ec/ alongside LpcACPIEC.bin once merged upstream).
const MODULE_BYTES: &[u8] = include_bytes!("..\\Hpecmp.bin");

/// Error codes returned by the module as the first result.
const HPECMP_OK: i64 = 0;
const HPECMP_E_RANGE: i64 = 1;
const HPECMP_E_LEN: i64 = 2;
const HPECMP_E_TIMEOUT: i64 = 3;
const HPECMP_E_BUSY: i64 = 4;

/// Abstraction over single-register EC MMIO access so engine code does not
/// depend on the transport (ports vs MMIO).
pub trait EcMmioWriter {
    /// EIDR(reg): read one EC register through the ERCD MMIO protocol.
    fn ercd_read(&self, reg: u8) -> Result<u8>;
    /// EIDW(reg, val): write one EC register through the ERCD MMIO protocol.
    fn ercd_write(&self, reg: u8, val: u8) -> Result<()>;
}

/// PawnIO-backed implementation. Owns the loaded Hpecmp module instance.
pub struct EcmpMmio {
    pawn: PawnIo,
}

unsafe impl Send for EcmpMmio {}
unsafe impl Sync for EcmpMmio {}

impl EcmpMmio {
    /// Load Hpecmp.bin into PawnIO and sanity-probe the mapping.
    pub fn connect() -> Result<Self> {
        if !is_installed() {
            return Err(Error::PawnIoNotInstalled);
        }
        let pawn = PawnIo::with_module(MODULE_BYTES)?;

        // Sanity check: probe returns the handshake byte at page offset
        // 0x10. ECTB (bit0) should read 1 while the interface is idle
        // (live-verified). A zero byte means the mapping is wrong.
        let out = pawn.execute("ioctl_probe", &[], 2)?;
        if out[0] != HPECMP_OK || (out[1] & 0x01) == 0 {
            return Err(Error::NtStatus(
                0xC000_009C, // STATUS_DEVICE_DATA_ERROR equivalent
                "Hpecmp probe failed: handshake byte not idle",
            ));
        }
        Ok(Self { pawn })
    }

    /// Raw one-byte read from the whitelisted page.
    pub fn read_page(&self, offset: u16) -> Result<u8> {
        let out = self.pawn.execute("ioctl_read_page", &[offset as i64], 2)?;
        check_err(out[0], "ioctl_read_page")?;
        Ok((out[1] & 0xFF) as u8)
    }

    /// Raw ERCD transaction; returns ERN1..ERN6 (results). Caller should
    /// hold the Access_EC mutex (see ecio::EcIo::with_lock).
    pub fn write_ercd(&self, payload: &[u8]) -> Result<[u8; 6]> {
        if payload.is_empty() || payload.len() > 5 {
            return Err(Error::RegisterRange { reg: payload.len() as u16 });
        }
        let mut args = vec![payload.len() as i64];
        for i in 0..5 {
            args.push(*payload.get(i).unwrap_or(&0) as i64);
        }
        let out = self.pawn.execute("ioctl_write_ercd", &args, 7)?;
        check_err(out[0], "ioctl_write_ercd")?;
        Ok([
            (out[1] & 0xFF) as u8,
            (out[2] & 0xFF) as u8,
            (out[3] & 0xFF) as u8,
            (out[4] & 0xFF) as u8,
            (out[5] & 0xFF) as u8,
            (out[6] & 0xFF) as u8,
        ])
    }

    /// Handshake byte at page offset 0x10 (diagnostics).
    pub fn handshake(&self) -> Result<u8> {
        let out = self.pawn.execute("ioctl_probe", &[], 2)?;
        check_err(out[0], "ioctl_probe")?;
        Ok((out[1] & 0xFF) as u8)
    }
}

impl EcMmioWriter for EcmpMmio {
    fn ercd_read(&self, reg: u8) -> Result<u8> {
        let out = self.pawn.execute("ioctl_ercd_read", &[reg as i64], 2)?;
        check_err(out[0], "ioctl_ercd_read")?;
        Ok((out[1] & 0xFF) as u8)
    }

    fn ercd_write(&self, reg: u8, val: u8) -> Result<()> {
        let out = self.pawn.execute("ioctl_ercd_write", &[reg as i64, val as i64], 1)?;
        check_err(out[0], "ioctl_ercd_write")
    }
}

fn check_err(code: i64, what: &'static str) -> Result<()> {
    match code {
        HPECMP_OK => Ok(()),
        HPECMP_E_RANGE => Err(Error::NtStatus(0xC000_000D /* INVALID_PARAMETER */, what)),
        HPECMP_E_LEN => Err(Error::NtStatus(0xC000_000D, what)),
        HPECMP_E_TIMEOUT => Err(Error::Timeout { what, ms_ms: 2000 }),
        HPECMP_E_BUSY => Err(Error::MutexFailed(format!("{what}: EC interface busy (ECTB set)"))),
        other => Err(Error::NtStatus(
            u32::try_from(other).unwrap_or(0xC000_0001),
            what,
        )),
    }
}

// ---------------------------------------------------------------------------
// WIRING PLAN (do not apply until Hpecmp.bin is signed and shipped):
//
// 1. lib.rs: add `pub mod ecmp_mmio;` next to `pub mod ecmp;` and re-export
//    `pub use ecmp_mmio::{EcMmioWriter, EcmpMmio};`.
//
// 2. engine.rs (fan-curve engine): where the write path currently no-ops,
//    i.e. where the safe-write protocol (read -> validate -> write ->
//    verify -> rollback) would act on EC 0x11 (fan level mirror) and
//    0xC1/0xC3 (duty mirrors), flip the capability gate:
//
//        let fan_write_active = match EcmpMmio::connect() {
//            Ok(mmio) => { engine.mmio = Some(mmio); true }
//            Err(_) => false, // module not installed/signed: read-only mode
//        };
//
//    All ercd_write calls must run inside ecio.with_lock(Access_EC) — the
//    MMIO page shares firmware state with the port EC, so the same mutex
//    discipline applies (README caller discipline).
// ---------------------------------------------------------------------------
