//! Payload construction for hpqBDataIn / response parsing from hpqBDataOut*.

/// Magic "SECU" signature required by the HP BIOS WMI dispatcher.
pub const SIGN: [u8; 4] = [0x53, 0x45, 0x43, 0x55];

/// Command group identifiers.
pub const CMD_DEFAULT: u32 = 0x20008;
pub const CMD_KEYBOARD: u32 = 0x20009;
pub const CMD_LEGACY: u32 = 0x00001;

/// A request to the BIOS: command group, sub-command type and payload bytes.
#[derive(Debug, Clone)]
pub struct Request {
    pub command: u32,
    pub command_type: u32,
    pub data: Vec<u8>,
}

impl Request {
    pub fn new(command: u32, command_type: u32) -> Self {
        Self { command, command_type, data: Vec::new() }
    }

    pub fn with_data(command: u32, command_type: u32, data: &[u8]) -> Self {
        Self { command, command_type, data: data.to_vec() }
    }

    /// Size field semantics (validated live on Victus 8C9C):
    /// - no payload → 0
    /// - with payload → payload length
    pub fn size(&self) -> u32 {
        if self.data.is_empty() { 0 } else { self.data.len() as u32 }
    }
}

/// Parsed BIOS response.
#[derive(Debug, Clone)]
pub struct Response {
    /// rwReturnCode from the embedded out-instance (0 = success,
    /// 3 = command unavailable, 4 = unknown, 5 = buffer size).
    pub return_code: u32,
    /// OutData bytes from the embedded out-instance.
    pub data: Vec<u8>,
}

impl Response {
    pub fn is_ok(&self) -> bool {
        self.return_code == 0
    }

    /// Interpret data[0] as u8.
    pub fn byte0(&self) -> Option<u8> {
        self.data.first().copied()
    }

    /// Interpret data[0..2] as little-endian u16.
    pub fn u16_at(&self, idx: usize) -> Option<u16> {
        if self.data.len() < idx + 2 { return None; }
        Some(u16::from_le_bytes([self.data[idx], self.data[idx + 1]]))
    }

    /// Interpret data[0..4] as little-endian u32.
    pub fn u32_at(&self, idx: usize) -> Option<u32> {
        if self.data.len() < idx + 4 { return None; }
        Some(u32::from_le_bytes([
            self.data[idx],
            self.data[idx + 1],
            self.data[idx + 2],
            self.data[idx + 3],
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_is_secu() {
        assert_eq!(&SIGN, b"SECU");
    }

    #[test]
    fn size_empty_is_zero() {
        assert_eq!(Request::new(CMD_DEFAULT, 0x10).size(), 0);
    }

    #[test]
    fn size_with_data() {
        let r = Request::with_data(CMD_DEFAULT, 0x23, &[0x01, 0x00, 0x00, 0x00]);
        assert_eq!(r.size(), 4);
    }

    #[test]
    fn parse_helpers() {
        let resp = Response { return_code: 0, data: vec![0x2C, 0x01, 0x37, 0x00] };
        assert_eq!(resp.byte0(), Some(0x2C));
        assert_eq!(resp.u16_at(0), Some(0x012C));
        assert_eq!(resp.u32_at(2), Some(0x37));
    }
}
