//! Raw PawnIO transport: open device, load module, execute functions.
//!
//! IOCTL contract (mirrors LibreHardwareMonitor PawnIo.cs):
//!   device: \\?\GLOBALROOT\Device\PawnIO
//!   LoadBinary: CTL  41394<<16 | 0x821<<2, in = module bytes
//!   Execute:    CTL  41394<<16 | 0x841<<2,
//!               in  = [32-byte NUL-padded fn name][i64 args...]
//!               out = [i64 results...]

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

use crate::error::{Error, Result};

const DEVICE_TYPE: u32 = 41_394 << 16;
const FN_NAME_LENGTH: usize = 32;
const IOCTL_PIO_LOAD_BINARY: u32 = 0x821 << 2;
const IOCTL_PIO_EXECUTE_FN: u32 = 0x841 << 2;
const CTL_LOAD_BINARY: u32 = DEVICE_TYPE | IOCTL_PIO_LOAD_BINARY;
const CTL_EXECUTE: u32 = DEVICE_TYPE | IOCTL_PIO_EXECUTE_FN;

const DEVICE_PATH: windows::core::PCWSTR =
    windows::core::w!("\\\\?\\GLOBALROOT\\Device\\PawnIO");

pub struct PawnIo {
    handle: HANDLE,
}

unsafe impl Send for PawnIo {}
unsafe impl Sync for PawnIo {}

/// True if PawnIO is installed (registry marker present).
pub fn is_installed() -> bool {
    // Read via crate `windows` registry API if available; otherwise probe the
    // device file presence as a cheap fallback.
    unsafe {
        windows::Win32::Storage::FileSystem::GetFileAttributesW(
            windows::core::w!(r"C:\Windows\System32\drivers\PawnIO.sys"),
        ) != windows::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES
    }
}

impl PawnIo {
    /// Open the PawnIO device and load the LpcACPIEC module bytes.
    pub fn with_module(module: &[u8]) -> Result<Self> {
        if !is_installed() {
            return Err(Error::PawnIoNotInstalled);
        }
        unsafe {
            let handle = CreateFileW(
                DEVICE_PATH,
                (windows::Win32::Storage::FileSystem::FILE_GENERIC_READ
                    | windows::Win32::Storage::FileSystem::FILE_GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .map_err(Error::OpenDevice)?;

            let this = Self { handle };
            this.raw_ioctl(CTL_LOAD_BINARY, module, &mut [])?;
            Ok(this)
        }
    }

    /// Execute a module function: NUL-padded 32 B name + i64 args, i64 results.
    pub fn execute(&self, name: &str, args: &[i64], out_len: usize) -> Result<Vec<i64>> {
        let mut in_buf = [0u8; FN_NAME_LENGTH];
        let bytes = name.as_bytes();
        if bytes.len() >= FN_NAME_LENGTH {
            return Err(Error::NtStatus(0xC000_000D /* INVALID_PARAMETER */, "fn name too long"));
        }
        in_buf[..bytes.len()].copy_from_slice(bytes);

        // pack args after the name
        let mut full = Vec::with_capacity(FN_NAME_LENGTH + args.len() * 8);
        full.extend_from_slice(&in_buf);
        for a in args {
            full.extend_from_slice(&a.to_le_bytes());
        }

        let mut out_buf = vec![0u8; out_len * 8];
        self.raw_ioctl(CTL_EXECUTE, &full, &mut out_buf)?;

        Ok(out_buf
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
            .collect())
    }

    fn raw_ioctl(&self, ctl: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        unsafe {
            let mut ret = 0u32;
            DeviceIoControl(
                self.handle,
                ctl,
                Some(input.as_ptr().cast()),
                input.len() as u32,
                Some(output.as_mut_ptr().cast()),
                output.len() as u32,
                Some(&mut ret),
                None,
            )
            .map_err(|e| Error::Execute(e, "execute_ioctl"))?;
            Ok(ret as usize)
        }
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}
