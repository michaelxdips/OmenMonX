//! EC port I/O over PawnIO's LpcACPIEC module.
//!
//! The module whitelists ports 0x62 (data) and 0x66 (command) only, and the
//! Windows ACPI driver serializes EC access with the `Access_EC` mutant —
//! we acquire it around every register transaction.

use windows::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, ReleaseMutex, WaitForSingleObject,
};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};

use crate::error::{Error, Result};
use crate::pawnio::{PawnIo, is_installed};

pub const EC_DATA: u16 = 0x62;
pub const EC_CMD: u16 = 0x66;
const MUTEX_NAME: windows::core::PCWSTR = windows::core::w!(r"Access_EC");

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
            // Open the existing EC mutex (created by the Windows ACPI driver).
            let mutex = match OpenMutexW(
                windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS(
                    windows::Win32::System::Threading::MUTEX_ALL_ACCESS.0,
                ),
                false,
                MUTEX_NAME,
            ) {
                Ok(h) => h,
                Err(_) => {
                    // Not yet created? create it (defensive; normally ACPI owns it)
                    CreateMutexW(None, false, MUTEX_NAME).map_err(|e| Error::MutexFailed(e.to_string()))?
                }
            };
            Ok(Self { pawn, mutex })
        }
    }

    /// Acquire Access_EC. Blocks up to `timeout_ms` for the ACPI driver to yield.
    pub fn lock(&self, timeout_ms: u32) -> Result<EcMutexGuard<'_>> {
        unsafe {
            let r = WaitForSingleObject(self.mutex, timeout_ms);
            if r != WAIT_OBJECT_0 {
                return Err(Error::MutexFailed(format!(
                    "WaitForSingleObject returned 0x{:08X} after {timeout_ms} ms",
                    r.0
                )));
            }
            Ok(EcMutexGuard { io: self })
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
