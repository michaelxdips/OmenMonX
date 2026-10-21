# Hpecmp.p — PawnIO module for the HP ECMP (ECCD) MMIO page

User-mode access to the HP Victus/OMEN (Insyde firmware) EC command
interface mapped at **physical 0xFC7E0000, length 0x1000**, for the
[OmenMon-rs](https://github.com/michaelxdips/OmenMonX) fan-curve work.

## Why

The existing `LpcACPIEC` PawnIO module whitelists only ports 0x62/0x66.
The ERCD protocol's result registers (ERN1–8 @ page offsets 0x11–0x18)
and data registers (EDT1–5 @ 0x1A–0x1E) are **not** reachable through
those ports — verified empirically on the target machine by brute-force
port scan plus MMIO page diff. True physical-memory access is required,
hence this dedicated, tightly-whitelisted module.

## Verified evidence (from DSDT AML decode + live probes, 2026-10)

| Offset | Field | Evidence |
|---|---|---|
| 0x10 bit0 | ECTB — command busy/ownership flag | Handshake byte live-confirmed at ERAM port-index 0x10, bit0 = 1 while idle |
| 0x10 bit1 | ECTE — transfer event flag | Live-confirmed: ECTE clears to 0 when the firmware consumes a command |
| 0x11–0x18 | ERN1–ERN8 result registers | DSDT ECCD opregion field-unit decode |
| 0x19 | ECMD — command / payload length | DSDT decode |
| 0x1A–0x1E | EDT1–EDT5 data registers | DSDT decode |

ERCD(cmd) protocol: `ECTB=1 → ECMD=len → EDT1..n → ECTE=1 → poll ECTE→0 →
read ERN1..ERN6`.
`EIDR(reg)` = ERRD (`0x5F`) with ERIB = reg; `EIDW(reg,val)` = ERWT
(`0x5F`) with buffer `[0xB1, val, 0x5F]`.

## Exported ioctls (via PawnIO `Execute`, i64 args / results)

| Function | Args | Returns |
|---|---|---|
| `ioctl_read_page` | offset | `[err, byte]` |
| `ioctl_write_ercd` | len, d0..d4 | `[err, ern1..ern6]` |
| `ioctl_ercd_read` | reg | `[err, value]` |
| `ioctl_ercd_write` | reg, val | `[err]` |
| `ioctl_probe` | — | `[err, handshake]` (read-only sanity check) |

Error codes: `0` OK, `1` range (outside page), `2` bad payload length,
`3` ECTE poll timeout (2000 iterations), `4` interface busy.

Safety: every access is bounds-checked to the single 4 KiB page at
0xFC7E0000; no arbitrary physical memory is exposed. Timeout paths release
ECTB so the interface is never left locked.

## Build recipe

Requires the pawn compiler (pawn-4.1.7152.zip from
[compuphase](https://www.compuphase.com/pawn/pawn.htm#pawnlicense)) and the
`include/` directory from PawnIO.Modules.

**WSL / Linux:**
```sh
unzip pawn-4.1.7152.zip -d pawncc && chmod +x pawncc/pawncc
git clone https://github.com/namazso/PawnIO.Modules
cp Hpecmp.p PawnIO.Modules/
cd PawnIO.Modules
../pawncc/pawncc Hpecmp.p -iinclude -C64 '-;+' '-(+' -p
# -> Hpecmp.bin  (then verify: strings Hpecmp.bin | grep Hpecmp, size sane)
```

**CI (recommended — matches PawnIO.Modules ci.yml):**
1. Fork `namazso/PawnIO.Modules`, push `Hpecmp.p` on a branch (e.g.
   `hpecmp`), let the repo's GitHub Actions run `pawncc` on it — that
   reproduces the exact toolchain the sign-PR expects.
2. Download the `Hpecmp.bin` artifact, attach it to the PR (do not commit
   binaries), and reference the successful run in the PR description.

## PR text (for github.com/namazso/PawnIO.Modules)

**Title:** `Add Hpecmp module: HP Victus/OMEN EC command MMIO page (0xFC7E0000)`

**Description:**

> ### What
> `Hpecmp.p` exposes the Insyde-firmware EC command opregion (ECCD) mapped
> at physical 0xFC7E0000, 0x1000 bytes, to user mode: raw page byte read,
> raw ERCD transaction, and read/write single-register wrappers
> (EIDR/EIDW). Needed because the ERN/EDT result and data registers of the
> HP ERCD protocol are not reachable via ports 0x62/0x66 — verified on
> hardware by brute-force port scan + MMIO page diff — so `LpcACPIEC`
> cannot serve this interface.
>
> ### Evidence for the field map
> Decoded from the target machine's DSDT (HP Victus 16.1, board 8C9C,
> Insyde BIOS F.17) and live-verified: the handshake byte at page offset
> 0x10 shows ECTB (bit0) = 1 while the EC interface is idle and ECTE
> (bit1) clears when a fired command is consumed. ERN1–8 @ 0x11–0x18,
> ECMD @ 0x19, EDT1–5 @ 0x1A–0x1E. Protocol: ECTB=1; ECMD=len;
> EDT1..n; ECTE=1; poll ECTE→0; results in ERN1..6. EIDR/EIDW encode as
> ERRD(0x5F)/ERWT(0x5F, [0xB1, val, 0x5F]).
>
> ### Safety rationale
> - Hard whitelist: all accesses are bounds-checked to the single page
>   0xFC7E0000..0xFC7E0FFF; no arbitrary physical memory is exposed.
> - Bounded polling (2000 iterations) with timeout error; every exit path
>   (including timeout) releases ECTB so firmware ownership is never left
>   dangling.
> - Read-only passthrough of the page for the plain read ioctl; writes go
>   only through the decoded ERCD handshake, so callers cannot scribble
>   raw bytes into arbitrary offsets without going through the protocol.
> - Callers are expected to hold the Windows ACPI `Access_EC` mutex
>   around transactions to serialize with the OS EC driver (the
>   OmenMon-rs client does this; documented in the module header and
>   README).
>
> Binaries built via this repo's CI on branch `hpecmp` (run linked
> below); source-only in this PR. License GPL-3.0-or-later, upstream
> project OmenMonX.

**CI branch strategy:** develop on a fork branch (`hpecmp`), squash PR
into `PawnIO.Modules` main; module binary is produced by the repo's own
Actions runner (Linux + pawncc per ci.yml), never committed by hand.
After merge, the namazso signature lands in the distributed
`Hpecmp.bin`; OmenMon-rs picks it up via its release pipeline.

## Caller discipline

Hold the `Access_EC` mutex (same mutant the Windows ACPI driver uses)
around every transaction; see `crates/hp-ec/src/ecmp_mmio.rs` in
OmenMon-rs for the Rust-side pattern.
