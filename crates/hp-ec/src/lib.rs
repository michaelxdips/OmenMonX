//! hp-ec: Embedded Controller access for HP Victus (Insyde firmware).
//!
//! Transport: PawnIO (signed kernel driver, https://pawnio.eu) running the
//! `LpcACPIEC` module, which mediates byte port I/O restricted to ports
//! 0x62/0x66 (the classic ACPI EmbeddedControl space). WinRing0 was rejected:
//! WinRing0x64.sys is on the Windows 11 vulnerable-driver blocklist.
//!
//! Layers (bottom-up):
//! 1. `pawnio::PawnIo` — raw IOCTL transport, module loading
//! 2. `ecio::EcIo` — port reads/writes with `Access_EC` mutex held
//! 3. `eram` — classic 256 B ERAM register access (0x62/0x66 command/data)
//! 4. `mailbox` — Insyde EC mailbox protocol (WQBC-decoded):
//!    acquire MSMT mutex -> write reg index to EHWB -> set HWRC command ->
//!    poll ECST -> read ECPC -> release

mod error;
pub mod pawnio;
pub mod ecio;
pub mod eram;
pub mod mailbox;
pub mod ecmp;

pub use error::{Error, Result};
pub use mailbox::EcMailbox;
pub use eram::Eram;

/// Empirically-locked Victus 8C9C register map (live correlation vs WMI
/// sensors, 2026-10-20): 0xB7 tracks CPU temp, 0xB2 tracks GPU temp.
/// The ACPI ECMP field-map names do not apply to the ERAM port view.
pub mod fields {
    /// CPU temperature (WMI sensor 0) — confirmed live
    pub const CPTV: usize = 0xB7;
    /// GPU temperature (WMI sensor 1) — confirmed live
    pub const GPTV: usize = 0xB2;
    /// GPU hotspot candidate (fluctuates faster than GPTV)
    pub const GHSV: usize = 0xB0;
    /// ERAM bank/page selector (observed changing)
    pub const PAGE: usize = 0x06;
}
