# OmenMon-rs

Native Rust replacement for HP OMEN Gaming Hub — built for the HP Victus 16
(board 8C9C) and compatible in principle with other HP laptops exposing the
`hpqBIntM` WMI BIOS interface.

**Status: v0.5 — telemetry release.** Temps, fan monitoring, battery health,
config, self-installer. Manual fan control is implemented but awaits a signed
kernel module (see [Fan control status](#fan-control-status)).

## Features

| Area | What you get |
|------|--------------|
| Temps | CPU / GPU / GPU-hotspot via direct EC (PawnIO) with WMI fallback |
| Fans | RPM + level monitoring; curve engine targeting ready |
| Battery | Health %, wear grade, charge rate, cycles, temperature — richer than OGH |
| Config | TOML profiles with interpolation curves |
| Install | Single-exe self-installer: Program Files copy, PATH, autostart, ARP entry |

## Install

1. Install [PawnIO](https://pawnio.eu) (signed kernel driver, required for EC access).
2. Download `omenmon-setup.exe` from Releases and run it as administrator.
3. `omenmon status` — done.

## Commands

```
omenmon probe      hardware capability matrix
omenmon temps      temperature sensors
omenmon fans       fan count/type/level
omenmon battery    health, wear grade, charge rate, cycles, temperature
omenmon status     one-shot dashboard (temps + fan + battery)
```

Run elevated for temps/fans (WMI BIOS + EC require admin); `battery` works
unelevated.

## Fan control status

This firmware (Insyde, Victus 8C9C, BIOS F.17) gates manual fan writes behind
an SMM handler. The full write protocol has been reverse-engineered:

- WMI `hpqBIntM` commands build a mailbox buffer and call `GHMI(0, 0xFD)`
- The SMM handler (SMI 0xD7) processes the mailbox and currently *ignores*
  fan-level writes unless OEM software state is present
- The underlying EC command path (`ERIB`/`ERCD` handshake over the ECMP MMIO
  page at 0xFC7E0000) is decoded and works, but requires a signed kernel
  module to reach MMIO from user mode

A PawnIO module implementing the ECMMIO handshake has been drafted for
submission to the [PawnIO.Modules](https://github.com/namazso/PawnIO.Modules)
signing pipeline. Once merged, `omenmon fans set <pct>` activates.

## Building

```
cargo build --release
```

Binaries land in `target/release/` (`omenmon.exe`, `omenmon-setup.exe`).
The installer copies `omenmon.exe`, registers a logon telemetry task, adds an
Add/Remove Programs entry, and provides `omenmon-setup uninstall`.

## License

GPL-3.0-or-later. Derived from
[OmenMon](https://github.com/OmenMon/OmenMon) (GPL-3) hardware knowledge;
all Rust code is original.
