# OmenMon-rs

Native Rust replacement for HP OMEN Gaming Hub — built for the HP Victus 16
(board 8C9C) and compatible in principle with other HP laptops exposing the
`hpqBIntM` WMI BIOS interface.

**Status: v0.3.0 — working fan control.** Telemetry, fan speed control, power cap,
thermal profiles, keyboard RGB, BIOS settings and a self-installer. Everything in
the table below was measured on the reference machine; nothing is aspirational.

## Features

| Area | What you get |
|------|--------------|
| Temps | CPU / GPU / GPU-hotspot via direct EC (PawnIO) with WMI fallback |
| **Fans** | **Max mode, manual speed, and hand-back-to-firmware — all verified** |
| Power | CPU power cap 5–65 W on the 230 W brick, 5–25 W on USB-C |
| Thermal | Balanced / Performance / Cool, read back from the firmware |
| Keyboard | Per-zone RGB with byte-exact restore |
| BIOS | Read and write BIOS settings through the WMI instance path |
| Battery | Health %, wear grade, charge rate, cycles, temperature — richer than OGH |
| Config | TOML profiles; saved cap and profile re-applied at logon |
| Install | Single-exe self-installer: Program Files copy, PATH, autostart, ARP entry |

## Install

1. Install [PawnIO](https://pawnio.eu) (signed kernel driver, required for EC access).
2. Download `omenmon-setup.exe` from Releases and run it as administrator.
3. `omenmon status` — done.

## Commands

```
omenmon status     one-shot dashboard (temps + fan + battery)
omenmon temps      temperature sensors
omenmon fans       fan count/type/level
omenmon fan max    spin the fans to maximum
omenmon fan auto   hand the fans back to the firmware
omenmon fan 45     hold a manual 45 % fan speed
omenmon cap 45     set the CPU power cap (5-65 W on the brick)
omenmon thermal performance|cool|balanced
omenmon kbd 0 FF0000        set one keyboard zone
omenmon bios list|get|set   BIOS settings
omenmon battery    health, wear, charge rate, cycles, temperature
omenmon probe      hardware capability matrix
omenmon apply      re-apply the saved cap and profile (used by the logon task)
```

Run elevated for anything that writes firmware. `status` and `battery` work
unelevated for the parts that do not need a token.

## Fan control

Fan speed control works on this board. The write is `0x2E`, whose first byte is
HP's own `FanSetScheme` enum:

| Scheme | Value | Effect (measured) |
|--------|-------|-------------------|
| `EWMA_ALGO` | 0 | Firmware's own algorithm. Level dips to 0 then ramps back — this is *handing control back*. |
| `SLIDER` | 1 | Holds the commanded percentage in bytes 1–3. Manual control. |
| `IDLE_AUTO` | 2 | Quiet, load-independent. |

`0x27 {1,0,0,0}` is the Max mode OGH exposes. **Turning it off does not release
the fans** — the level stays pinned at maximum. The release is `0x2E` with
`EWMA_ALGO`, which is why `omenmon fan auto` sends that rather than a low slider
value. A low slider value looks like a release but is a manual hold: measured
from max, it fell 59 → 34 and then latched at 39, never dipping.

An earlier release of this README claimed manual fan control "awaits a signed
kernel module". That was wrong: it was based on a probe that sent the fan table's
dB index where the firmware reads a scheme and a speed. The write channel was
open the whole time.

## What does not work

These are firmware-refused on 8C9C, verified rather than assumed:

| Feature | Why |
|---------|-----|
| Fan **curve** writing (`0x32`) | Rejected. Speed control and curve writing are different commands — do not conflate them. |
| Battery charge limit | `Battery Care Function` returns rc 5 for all 27 encodings. |
| GPU mode switch | `0x52` returns BIOS err 5. |
| BIOS undervolt / OC | `0x35` returns WMI err 31. |

The EC tachometer at `0xC4` reads a frozen 1249 rpm on this board, so it is never
displayed as a live RPM. Fan level (`0x2D`) is the live readout.

## Building

```
cargo build --release
```

Binaries land in `target/release/`: `omenmon.exe`, `omenmon-gui.exe`,
`omenmon-setup.exe`, `daemon.exe`.

The reverse-engineering probes are **not** built by default. They need
`--features probes` and are gated so they cannot leak into a release:

```
cargo build --release --features probes --bin fan_handoff_probe
```

## License

GPL-3.0-or-later. Derived from
[OmenMon](https://github.com/OmenMon/OmenMon) (GPL-3) hardware knowledge;
all Rust code is original.
