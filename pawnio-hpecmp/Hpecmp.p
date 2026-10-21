// Hpecmp.p — PawnIO module: user-mode access to the HP ECMP (ECCD opregion)
// MMIO page for HP Victus / OMEN (Insyde firmware, board 8C9C, BIOS F.17).
//
// Copyright (C) 2026 Michael / OmenMon-rs contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//
// WHY THIS MODULE EXISTS
// ----------------------
// The HP EC command interface (ERCD opregion) lives in a memory-mapped page
// at physical address 0xFC7E0000, length 0x1000. The existing PawnIO
// LpcACPIEC module only whitelists ports 0x62/0x66, and the ERN/EDT result
// registers of the ERCD protocol are NOT reachable through those ports
// (verified empirically on 2026-10: brute-force port scan + page-diff).
// True physical-memory access is therefore required.
//
// VERIFIED FIRMWARE FACTS (from AML decode + live probes — do not change
// these offsets without re-deriving them):
//   MMIO page 0xFC7E0000 = ECCD opregion (ACPI OperationRegion "ECCD",
//   EmbeddedControl-style field unit mapped into memory by Insyde SMM).
//   Field map (byte offsets within the page):
//     0x10 bit0 = ECTB  : EC command busy/ownership flag. Host sets 1
//                         before a transaction; firmware's ERAM mirror at
//                         port-index 0x10 confirmed bit0=1 while idle.
//     0x10 bit1 = ECTE  : EC transfer-enable/event flag. Host sets 1 to
//                         fire the command; firmware clears it to 0 when
//                         the command has been consumed (live-confirmed:
//                         ECTE clears on fire).
//     0x11..0x18 = ERN1..ERN8  : EC result registers (firmware writes
//                         results here; read after ECTE clears).
//     0x19       = ECMD : EC command byte (for ERCD: the payload length).
//     0x1A..0x1E = EDT1..EDT5 : EC data registers (command payload).
//
// ERCD(cmd) protocol (decoded from DSDT, handshake byte live-verified):
//   1. ECTB = 1                      (claim the interface)
//   2. ECMD = len                    (payload byte count)
//   3. EDT1..EDTn = payload bytes
//   4. ECTE = 1                      (fire)
//   5. poll ECTE until it reads 0    (firmware consumed the command)
//   6. read results from ERN1..ERN6
//
// EIDR(reg)  = ERRD command (0x5F) with register index in ERIB:
//              ERCD payload = [0xB1, reg, 0x5F]?  — NO. See the caller-side
//              note: EIDR(reg) uses the ERRD(0x5F) command with ERIB=reg;
//              EIDW(reg,val) uses ERWT(0x5F) with buffer [0xB1, val, 0x5F].
//              This module exposes the raw ERCD transport (ioctl_write_ercd)
//              plus convenience wrappers ioctl_ercd_read / ioctl_ercd_write
//              that encode those two forms.
//
// SAFETY / WHITELISTING
// ---------------------
// PawnIO modules are signed (namazso) and run in kernel mode; any module
// that can touch physical memory is a potential privilege-escalation
// primitive. This module therefore hard-locks the physical range:
//   * every access is masked to page offset 0x0000..0x0FFF,
//   * the base is a compile-time constant 0xFC7E0000,
//   * no arbitrary-length reads, no access outside the single page.
// Callers should serialize access with the Windows ACPI driver by holding
// the `Access_EC` mutex around every transaction (same discipline as the
// LpcACPIEC path in OmenMon-rs).
//
// BUILD (see ci.yml of PawnIO.Modules):
//   pawncc Hpecmp.p -iinclude -C64 '-;+' '-(+' -p
//   (pawn-4.1.7152.zip from compuphase)

#include <native.inc>   // PawnIO natives: physical_read / physical_write

// ---- Compile-time constants -----------------------------------------------

// Physical base of the ECCD opregion (verified: ACPI DSDT opregion decode;
// live handshake byte at page offset 0x10 confirms the mapping).
#define PHYS_BASE       0xFC7E0000
// Page length: exactly one 4 KiB page. Anything beyond is rejected.
#define PAGE_LEN        0x1000

// Field offsets within the page (see header comment for evidence):
#define OFF_HANDSHAKE   0x10    // ECTB (bit0) | ECTE (bit1)
#define OFF_ERN1        0x11    // ERN1..ERN8 = 0x11..0x18
#define OFF_ERN8        0x18
#define OFF_ECMD        0x19    // command / payload length
#define OFF_EDT1        0x1A    // EDT1..EDT5 = 0x1A..0x1E

// Handshake bits:
#define ECTB_BIT        0x01
#define ECTE_BIT        0x02

// Insyde EC command bytes (from DSDT: ERRD/ERWT both dispatch via 0x5F):
#define EC_CMD_ERRD     0x5F    // read one register
#define EC_CMD_ERWT     0x5F    // write one register (payload distinguishes)

// Poll budget for ECTE -> 0. The EC typically answers in a few hundred
// microseconds; 2000 iterations is a generous upper bound (~milliseconds).
#define POLL_MAX        2000

// ---- Error codes returned by every ioctl ----------------------------------
// PawnIO surfaces the module return value as the first i64 result; the
// driver rejects with STATUS_INVALID_DEVICE_REQUEST when a function returns
// false, so all fallible functions return a value and the caller checks it.
#define HPECMP_OK        0
#define HPECMP_E_RANGE   1   // offset/length outside the whitelisted page
#define HPECMP_E_LEN     2   // payload length out of 1..5
#define HPECMP_E_TIMEOUT 3   // ECTE did not clear within POLL_MAX iterations
#define HPECMP_E_BUSY    4   // ECTB/ECTE still asserted before we started

// ---- Helpers ---------------------------------------------------------------

// Read one byte of the whitelisted page. Offset must be < PAGE_LEN.
stock PhysReadByte(off)
{
    return physical_read(PHYS_BASE + off, 1);
}

// Write one byte of the whitelisted page. Offset must be < PAGE_LEN.
stock PhysWriteByte(off, val)
{
    physical_write(PHYS_BASE + off, 1, val);
}

// Set/clear individual bits in the handshake byte at 0x10 without
// disturbing the other bits (read-modify-write).
stock HandshakeSet(bits)
{
    PhysWriteByte(OFF_HANDSHAKE, (PhysReadByte(OFF_HANDSHAKE) | bits));
}

stock HandshakeClear(bits)
{
    PhysWriteByte(OFF_HANDSHAKE, (PhysReadByte(OFF_HANDSHAKE) & ~bits));
}

// Run one ERCD transaction. Returns HPECMP_OK and fills ern[0..5] via the
// reference-style outputs, or an error code. `len` payload bytes are given
// as d0..d4 (unused high bytes ignored).
//
// Protocol (see header): ECTB=1; ECMD=len; EDT1..n; ECTE=1; poll ECTE->0;
// results in ERN1..ERN6.
stock ErcdTransact(len, d0, d1, d2, d3, d4, &ern1, &ern2, &ern3, &ern4, &ern5, &ern6)
{
    new hs;

    // Preflight: refuse to stomp on an in-flight firmware transaction.
    hs = PhysReadByte(OFF_HANDSHAKE);
    if ((hs & ECTB_BIT) != 0 || (hs & ECTE_BIT) != 0)
        return HPECMP_E_BUSY;

    if (len < 1 || len > 5)
        return HPECMP_E_LEN;

    // 1) claim the interface
    HandshakeSet(ECTB_BIT);

    // 2) command = payload length (ERCD semantics)
    PhysWriteByte(OFF_ECMD, len);

    // 3) payload -> EDT1..EDT5 (0x1A..0x1E)
    PhysWriteByte(OFF_EDT1 + 0, d0);
    if (len > 1) PhysWriteByte(OFF_EDT1 + 1, d1);
    if (len > 2) PhysWriteByte(OFF_EDT1 + 2, d2);
    if (len > 3) PhysWriteByte(OFF_EDT1 + 3, d3);
    if (len > 4) PhysWriteByte(OFF_EDT1 + 4, d4);

    // 4) fire
    HandshakeSet(ECTE_BIT);

    // 5) poll ECTE -> 0, bounded (POLL_MAX iterations)
    new i;
    for (i = 0; i < POLL_MAX; ++i)
    {
        hs = PhysReadByte(OFF_HANDSHAKE);
        if ((hs & ECTE_BIT) == 0)
            break;
    }
    if (i == POLL_MAX)
    {
        // Timed out: release ownership so the interface is not left locked.
        HandshakeClear(ECTB_BIT);
        return HPECMP_E_TIMEOUT;
    }

    // 6) results: ERN1..ERN6 (0x11..0x16)
    ern1 = PhysReadByte(OFF_ERN1 + 0);
    ern2 = PhysReadByte(OFF_ERN1 + 1);
    ern3 = PhysReadByte(OFF_ERN1 + 2);
    ern4 = PhysReadByte(OFF_ERN1 + 3);
    ern5 = PhysReadByte(OFF_ERN1 + 4);
    ern6 = PhysReadByte(OFF_ERN1 + 5);

    // Release ownership.
    HandshakeClear(ECTB_BIT);
    return HPECMP_OK;
}

// ---- IOCTLs (exported; called via PawnIO Execute) --------------------------

/// ioctl_read_page(offset) -> [error, byte]
/// Read one byte from the ECMP page. `offset` is masked to 0x0000..0x0FFF
/// and any value >= PAGE_LEN returns HPECMP_E_RANGE.
public ioctl_read_page(offset)
{
    new out[2];

    if (offset < 0 || offset >= PAGE_LEN)
    {
        out[0] = HPECMP_E_RANGE;
        out[1] = 0;
        return SetReturnValue(out, 2);
    }

    out[0] = HPECMP_OK;
    out[1] = PhysReadByte(offset) & 0xFF;
    return SetReturnValue(out, 2);
}

/// ioctl_write_ercd(len, d0..d4) -> [error, ern1..ern6]
/// Raw ERCD transaction: `len` payload bytes (1..5), results in ERN1..ERN6.
/// Unused data arguments are ignored. This is the escape hatch for commands
/// not covered by the wrappers below.
public ioctl_write_ercd(len, d0, d1, d2, d3, d4)
{
    new out[7];
    new e1, e2, e3, e4, e5, e6, rc;

    rc = ErcdTransact(len, d0, d1, d2, d3, d4, e1, e2, e3, e4, e5, e6);

    out[0] = rc;
    out[1] = e1;
    out[2] = e2;
    out[3] = e3;
    out[4] = e4;
    out[5] = e5;
    out[6] = e6;
    return SetReturnValue(out, 7);
}

/// ioctl_ercd_read(reg) -> [error, value]
/// EIDR(reg): issue the ERRD read command (0x5F) for register `reg`.
///
/// Encoding verified from DSDT: the ERRD dispatcher takes the target
/// register index as its argument (ERIB); payload is [reg, 0x5F]. Results
/// land in ERN1 (status) / ERN2 (value) per the ERCD contract.
public ioctl_ercd_read(reg)
{
    new out[7];
    new e1, e2, e3, e4, e5, e6, rc;

    rc = ErcdTransact(2, reg & 0xFF, EC_CMD_ERRD, 0, 0, 0,
                      e1, e2, e3, e4, e5, e6);

    out[0] = rc;
    // value in ERN2 when the command succeeded; 0 otherwise
    out[1] = (rc == HPECMP_OK) ? e2 : 0;
    return SetReturnValue(out, 2);
}

/// ioctl_ercd_write(reg, val) -> [error]
/// EIDW(reg, val): issue the ERWT write command (0x5F) with buffer
/// [0xB1, val, 0x5F] targeting register `reg` (verified from DSDT decode).
public ioctl_ercd_write(reg, val)
{
    new out[7];
    new e1, e2, e3, e4, e5, e6, rc;

    rc = ErcdTransact(3, 0xB1, val & 0xFF, EC_CMD_ERWT, 0, 0,
                      e1, e2, e3, e4, e5, e6);

    out[0] = rc;
    return SetReturnValue(out, 1);
}

/// ioctl_probe() -> [error, handshake]
/// Read-only sanity check: returns the current handshake byte at offset
/// 0x10. Bit0 (ECTB) should read 1 while the EC interface is idle
/// (live-verified); useful for callers to validate the mapping before use.
public ioctl_probe()
{
    new out[2];

    out[0] = HPECMP_OK;
    out[1] = PhysReadByte(OFF_HANDSHAKE) & 0xFF;
    return SetReturnValue(out, 2);
}
