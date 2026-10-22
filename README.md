# OmenMon-rs

A native Rust replacement for HP OMEN Gaming Hub — for the HP Victus 16
(board 8C9C), and in principle for other HP laptops exposing the `hpqBIntM`
WMI BIOS interface. A 7 MB portable GUI and a 1.4 MB CLI. No telemetry, no
accounts, no spinners: it talks to your firmware and shows you the answer.

**Status: v0.3.1.** Everything in the feature table below was measured on the
reference machine with real workloads — nothing is aspirational. What the
firmware refuses is documented too, with the evidence.

[![crates](https://img.shields.io/badge/crates-5-blue)](https://github.com/michaelxdips/OmenMonX) [![tests](https://img.shields.io/badge/tests-103%20passing-brightgreen)](https://github.com/michaelxdips/OmenMonX/actions) [![platform](https://img.shields.io/badge/Windows-10%2B-blueviolet)](https://github.com/michaelxdips/OmenMonX) [![license](https://img.shields.io/badge/license-GPL--3.0-orange)](LICENSE)

## What you get

| Area | What it does |
|------|--------------|
| **GUI** | Four-page console — Overview (live temps, fan curve plot, battery), Control (power cap, thermal, fans), Lighting (keyboard RGB), System (battery health, BIOS, updates, build info). Dark single-accent design, keyboard-navigable, works from 940 px window width. |
| **CLI** | One-shot `status`, temperature sensors, fan control, power cap, thermal profiles, RGB zones, BIOS read/write, battery health, hardware probe. |
| **Fans** | Max mode, manual percentage hold, and a verified hand-back-to-firmware. The release path sends HP's own `EWMA_ALGO` scheme — see [Fan control](#fan-control) for why that matters. |
| **Power** | CPU power cap 5–65 W on the 230 W brick, 5–25 W on USB-C. The ceiling is read from the firmware, never hardcoded. Every write is readback-verified; only the *verified* outcome is persisted. |
| **Thermal** | Balanced / Performance / Cool, written and read back from the firmware. |
| **Keyboard** | Per-zone RGB with byte-exact read-modify-write — a failed write restores the original table. |
| **BIOS** | Read and write BIOS settings through the WMI instance path (class-level calls fail with rc 5; this project got that right). |
| **Battery** | Health %, wear grade, charge rate, cycle count, temperature — more than OGH shows. |
| **Safety** | Saved cap and thermal profile re-applied at logon only after verification. Interrupted operations leave a recovery marker, not a broken state. Crash reports survive the crash that writes them. |
| **Self-updating** | Checks GitHub Releases, verifies a SHA-256 digest *before* renaming anything, and stages a per-asset install plan. Pre-release tags are never offered as stable updates. |

## Install

1. Install [PawnIO](https://pawnio.eu) — the signed kernel driver needed for
   direct EC access.
2. Download `omenmon-setup.exe` from
   [Releases](https://github.com/michaelxdips/OmenMonX/releases/latest) and run
   it as administrator. It copies the binaries to `C:\Program Files\OmenMon`,
   adds the folder to your user PATH, registers a logon task that re-applies
   your saved settings, and adds an Add/Remove Programs entry.

**Portable, no install:** download `OmenMonX-portable.zip`, unzip anywhere, run
`omenmon-gui.exe` or `omenmon.exe` from the folder. Nothing touches PATH,
autostart or the registry. The zip contains its own `SHA256SUMS.txt`; the
release page also publishes the digest of the zip itself.

**Upgrading:** the GUI checks GitHub Releases on launch and offers the update;
the digest is verified before anything is replaced. Or just overwrite with the
portable zip.

## CLI

```
omenmon status     one-shot dashboard (temps + fan + battery)
omenmon temps      temperature sensors
omenmon fans       fan count and per-fan level
omenmon fan max    spin the fans to maximum
omenmon fan auto   hand the fans back to the firmware
omenmon fan 45     hold a manual 45 % fan speed
omenmon cap 45     set the CPU power cap (supply-aware, readback-checked)
omenmon thermal performance|cool|balanced
omenmon kbd 0 --color FF8800   set one keyboard zone
omenmon bios list|get|set      BIOS settings
omenmon battery    health, wear, charge rate, cycles, temperature
omenmon probe      hardware capability matrix
omenmon apply      re-apply the saved cap and profile (logon task)
omenmon restore-defaults   clear saved settings, firmware re-applies its own
```

Writes need an elevated terminal. `status` and `battery` work unelevated for
the parts that do not need a token — the output says which parts degraded and
why. Usage errors are reported *before* elevation is attempted, so a typo does
not come back disguised as a permissions problem.

## Fan control

Fan speed control works on this board. The write channel is `0x2E`, whose
first byte is HP's own `FanSetScheme` enum (read from `PerformanceControl.dll`
by reflection, values confirmed):

| Scheme | Value | Effect (measured) |
|--------|-------|-------------------|
| `EWMA_ALGO` | 0 | The firmware's own algorithm. Level dips to 0 and ramps back — this is *handing control back*. |
| `SLIDER` | 1 | Holds the commanded percentage in bytes 1–3. Manual control. |
| `IDLE_AUTO` | 2 | Quiet, load-independent. |

`0x27 {1,0,0,0}` is Max mode. **Turning it off does not release the fans** —
the level stays pinned at maximum until a `0x2E` write arrives. That is why
`omenmon fan auto` sends `EWMA_ALGO`, not a low slider value: measured from
max, a low slider write fell 59 → 34 and then *latched* at 39, holding the
fans at a manual speed while looking like a release.

An earlier version of this README claimed manual fan control "awaits a signed
kernel module". That was wrong — the probe behind it sent the fan table's dB
index where the firmware reads a scheme byte. The write channel was open the
whole time. Check the vendor's own call site before declaring a channel dead.

## What does not work

Refused by the firmware on 8C9C — verified, not assumed:

| Feature | Why |
|---------|-----|
| Fan **curve** writing (`0x32`) | Rejected. Speed control and curve writing are different commands — do not conflate them. |
| Battery charge limit | `Battery Care Function` returns rc 5 for all 27 encodings tried. |
| GPU mode switch (`0x52`) | BIOS error 5. |
| BIOS undervolt / OC (`0x35`) | WMI error 31. |

The EC tachometer at `0xC4` reads a frozen 1249 rpm on this board, so it is
never displayed as live RPM — live fan level comes from `0x2D`. The EC mutex
(`Global\Access_EC`) serializes cooperating userland peers and every
transaction drains the status buffer first; it does not serialize WMI-side
firmware writes, and the code is honest about that.

## Architecture

Five crates:

| Crate | Role |
|-------|------|
| `hp-ec` | Direct EC access via PawnIO: port I/O, mutex discipline, bus resync |
| `hp-wmi` | The `hpqBIntM` WMI transport: payloads, instance-path calls, COM lifetime |
| `hp-core` | Config (atomic TOML with `.bak`), fan curves, battery, the engine |
| `omenmon-cli` | The CLI, the daemon (single-instance, atomic lockfile), the installer |
| `omenmon-gui` | The egui console, sampler, tray, updater, crash reporting |

CI enforces on every push: full test suite, clippy with warnings denied, and a
probe-leak gate proving a plain release build contains exactly the four shipped
binaries. Releases are tag-triggered and publish the four exes plus the
portable zip with SHA-256 checksums.

## Building

```
cargo build --release
```

You need the Windows MSVC toolchain; PawnIO is only needed at runtime, not to
compile. Binaries land in `target/release/`: `omenmon.exe`, `omenmon-gui.exe`,
`omenmon-setup.exe`, `daemon.exe`.

## License

GPL-3.0-or-later. Derived from
[OmenMon](https://github.com/OmenMon/OmenMon) (GPL-3) hardware knowledge;
all Rust code is original.
