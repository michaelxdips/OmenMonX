//! EC port I/O over PawnIO's LpcACPIEC module.
//!
//! The module whitelists ports 0x62 (data) and 0x66 (command) only.
//!
//! ## Mutex scope and semantics
//!
//! We acquire the `Global\Access_EC` mutant around every register
//! transaction. Note carefully what this does and does not guarantee:
//!
//! * It serializes only **cooperating userland peers** that follow the same
//!   LibreHardwareMonitor convention of opening `Global\Access_EC`.
//! * **WMI-side EC mutations are NOT serialized by it** — the Windows ACPI
//!   driver's own EC access (e.g. methods invoked via the WMI BIOS interface)
//!   does not pass through this mutant.
//! * `Access_EC` is a userland naming convention inherited from
//!   LibreHardwareMonitor, not a kernel ACPI object owned by the ACPI driver.
//!
//! If the mutex cannot be opened we fail hard: a self-created mutex would
//! serialize nothing against other EC users and only create false safety.

use windows::Win32::System::Threading::{OpenMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};

use crate::error::{Error, Result};
use crate::pawnio::{PawnIo, is_installed};

pub const EC_DATA: u16 = 0x62;
pub const EC_CMD: u16 = 0x66;
const MUTEX_NAME: windows::core::PCWSTR = windows::core::w!(r"Global\Access_EC");

/// Owns the PawnIO module handle and the Access_EC mutex handle.
pub struct EcIo {
    pawn: PawnIo,
    mutex: HANDLE,
}

unsafe impl Send for EcIo {}
unsafe impl Sync for EcIo {}

/// Opaque guard: releases Access_EC on drop.
pub struct EcMutexGuard<'a> {
    io: &'a EcIo,
}

impl Drop for EcMutexGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.io.mutex);
        }
    }
}

impl EcIo {
    /// Load LpcACPIEC.bin (compiled from PawnIO.Modules, LGPL-2.1) into PawnIO.
    pub fn connect() -> Result<Self> {
        if !is_installed() {
            return Err(Error::PawnIoNotInstalled);
        }
        static MODULE: &[u8] = include_bytes!("..\\LpcACPIEC.bin");
        let pawn = PawnIo::with_module(MODULE)?;

        unsafe {
            // Open the existing EC mutex. Fail hard if absent: creating our own
            // would not serialize against anything else touching the EC.
            let mutex = OpenMutexW(
                windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS(
                    windows::Win32::System::Threading::MUTEX_ALL_ACCESS.0,
                ),
                false,
                MUTEX_NAME,
            )
            .map_err(|e| Error::MutexFailed(format!("OpenMutexW(Global\\Access_EC): {e}")))?;
            Ok(Self { pawn, mutex })
        }
    }

    /// Acquire Access_EC. Blocks up to `timeout_ms` for a peer to release.
    pub fn lock(&self, timeout_ms: u32) -> Result<EcMutexGuard<'_>> {
        unsafe {
            let r = WaitForSingleObject(self.mutex, timeout_ms);
            if r == WAIT_OBJECT_0 {
                return Ok(EcMutexGuard { io: self });
            }
            // WAIT_ABANDONED (0x80): the previous owner terminated without
            // releasing. The state of any interrupted EC transaction is
            // unknown, so surface this as its own error rather than treating
            // it as a generic failure.
            if r == windows::Win32::Foundation::WAIT_ABANDONED {
                return Err(Error::MutexAbandoned);
            }
            Err(Error::MutexFailed(format!(
                "WaitForSingleObject returned 0x{:08X} after {timeout_ms} ms",
                r.0
            )))
        }
    }

    /// Raw port byte read (port must be 0x62/0x66 per module whitelist).
    /// Mutex should be held; module enforces whitelist regardless.
    pub fn port_read(&self, port: u16) -> Result<u8> {
        let out = self.pawn.execute("ioctl_pio_read", &[port as i64], 1)?;
        Ok((out[0] & 0xFF) as u8)
    }

    /// Raw port byte write.
    pub fn port_write(&self, port: u16, value: u8) -> Result<()> {
        self.pawn
            .execute("ioctl_pio_write", &[port as i64, value as i64], 0)?;
        Ok(())
    }

    /// Execute `f` while holding the EC mutex.
    pub fn with_lock<T>(&self, timeout_ms: u32, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let _g = self.lock(timeout_ms)?;
        f(self)
    }
}
