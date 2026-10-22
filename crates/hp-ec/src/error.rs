use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("PawnIO is not installed (HKLM\\...\\Uninstall\\PawnIO missing). Install PawnIO_setup.exe")]
    PawnIoNotInstalled,
    #[error("failed to open \\\\?\\GLOBALROOT\\Device\\PawnIO: {0}")]
    OpenDevice(windows::core::Error),
    #[error("PawnIO Execute('{1}') failed: {0}")]
    Execute(windows::core::Error, &'static str),
    #[error("PawnIO module returned NTSTATUS 0x{0:08X} from '{1}'")]
    NtStatus(u32, &'static str),
    #[error("EC timeout: {what} did not become ready in {ms_ms} ms")]
    Timeout { what: &'static str, ms_ms: u64 },
    #[error("Access_EC mutex abandoned by previous owner (interrupted EC transaction, state unknown)")]
    MutexAbandoned,
    #[error("Access_EC mutex acquire failed: {0}")]
    MutexFailed(String),
    #[error("invalid register {reg:#04X}: out of ERAM range")]
    RegisterRange { reg: u16 },
}
