use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("COM initialization failed: {0}")]
    ComInit(#[source] windows::core::Error),

    #[error("WMI connect to namespace '{namespace}' failed: {source}")]
    Connect {
        namespace: String,
        #[source]
        source: windows::core::Error,
    },

    #[error("hpqBIntM instance 'ACPI\\PNP0C14\\0_0' not found — HP WMI BIOS ACPI device missing")]
    NoInstance,

    #[error("WMI call {method} failed: {source}")]
    Invoke {
        method: String,
        #[source]
        source: windows::core::Error,
    },

    #[error("WMI call {method} returned false")]
    ReturnedFalse { method: String },

    #[error("missing out-parameter '{name}' in response")]
    MissingOutParam { name: String },

    #[error("missing '{name}' in response — no return code, command result unknown (never treat as success)")]
    MissingReturnCode { name: String },

    #[error("hpqBIntM instance found but its __PATH is empty — cannot invoke methods on it")]
    EmptyInstancePath,

    #[error("method {method} returned an empty payload where {expected} bytes were expected — not a valid zero response")]
    EmptyResponse { method: String, expected: usize },

    #[error("BIOS returned error code {code} for command 0x{command:X} type 0x{command_type:X}")]
    BiosError {
        code: u32,
        command: u32,
        command_type: u32,
    },

    #[error("unexpected response shape: {0}")]
    Shape(&'static str),

    #[error("Windows error: {0}")]
    Windows(#[from] windows::core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
