//! Insyde EC mailbox protocol, decoded from WQBC in SSD3.aml:
//!   1. Acquire mutex `MSMT`
//!   2. Write the register index to `EHWB`
//!   3. Set command `HWRC`
//!   4. Poll `ECST` until ready
//!   5. Read result from `ECPC`
//!   6. Release `MSMT`
//!
//! On Victus 8C9C the mailbox registers live in the ERAM low page; the
//! offsets here follow the ACPI ECMP field map (ECST/ECPC/EHWB/HWRC are
//! validated empirically in M2.2 probe — adjust via probe if rwret mismatches).

use crate::ecio::EcIo;
use crate::eram::Eram;
use crate::error::{Error, Result};

// Register indices (ERAM addresses) — provisional, verify live in M2.2 probe.
pub const REG_EHWB: u8 = 0x6E; // mailbox address latch
pub const REG_HWRC: u8 = 0x6F; // command register
pub const REG_ECPC: u8 = 0x63; // mailbox data port
pub const REG_ECST: u8 = 0x62; // mailbox status port

// ECST values observed in ACPI; poll until status matches.
pub const ECST_READY: u8 = 0x00;

pub struct EcMailbox<'a> {
    io: &'a EcIo,
}

#[derive(Debug, Clone, Copy)]
pub struct MailboxResult {
    pub status: u8,
    pub data: u8,
}

impl<'a> EcMailbox<'a> {
    pub fn new(io: &'a EcIo) -> Self {
        Self { io }
    }

    /// Single transaction: write reg -> issue HWRC cmd -> poll -> read ECPC.
    /// Holds the EC mutex for the entire sequence.
    pub fn transact(&self, reg_index: u8, cmd: u8) -> Result<MailboxResult> {
        self.io.with_lock(2000, |io| {
            let eram = Eram::new(io);
            // 2: write reg index to EHWB
            eram.write(REG_EHWB, reg_index)?;
            // 3: set command
            eram.write(REG_HWRC, cmd)?;
            // 4: poll ECST
            let mut status = ECST_READY;
            for _ in 0..1_000_000 {
                let st = eram.read(REG_ECST)?;
                if st == ECST_READY {
                    status = st;
                    break;
                }
                status = st;
            }
            // 5: read ECPC
            let data = eram.read(REG_ECPC)?;
            Ok(MailboxResult { status, data })
        })
    }

    /// Convenience: read a byte field through the mailbox.
    pub fn read_field(&self, reg_index: u8) -> Result<u8> {
        let r = self.transact(reg_index, 0x00 /* read cmd */)?;
        if r.status != ECST_READY {
            return Err(Error::CommandRejected { cmd: reg_index, status: r.status });
        }
        Ok(r.data)
    }
}
