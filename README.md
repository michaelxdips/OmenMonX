# OmenMon-rs

A native Rust replacement for HP OMEN Gaming Hub — built and measured on the
HP **Victus 16 (board 8C9C)**, and in principle usable on other HP laptops
exposing the `hpqBIntM` WMI BIOS interface. One 7 MB portable GUI, one 1.4 MB
CLI. No telemetry, no accounts, no background services phoning home: it talks
to your firmware and shows you the answer.

**Status: v0.3.2.** Every claim in this document was measured on the reference
machine with real workloads — nothing is aspirational, and what the firmware
*refuses* is documented too, with the evidence. Where a value is unverified,
the doc says so (see [Power limits](#power-limits-the-honest-version)).

[![tests](https://img.shields.io/badge/tests-105%20passing-brightgreen)](https://github.com/michaelxdips/OmenMonX/actions)
[![platform](https://img.shields.io/badge/Windows-10%2B-blueviolet)](https://github.com/michaelxdips/OmenMonX)
[![license](https://img.shields.io/badge/license-GPL--3.0-orange)](LICENSE)

---

## Table of contents

- [What you get](#what-you-get)
- [Install](#install)
  - [Installer](#installer)
  - [Portable](#portable-no-install)
  - [SmartScreen and unsigned executables](#smartscreen-and-unsigned-executables)
  - [Upgrading](#upgrading)
- [CLI reference](#cli-reference)
- [GUI reference](#gui-reference)
- [How the hardware actually works here](#how-the-hardware-actually-works-here)
  - [Fan control](#fan-control)
  - [Power limits (the honest version)](#power-limits-the-honest-version)
  - [Thermal profiles](#thermal-profiles)
  - [Keyboard RGB](#keyboard-rgb)
  - [Battery](#battery)
- [What does not work](#what-does-not-work)
- [Safety model](#safety-model)
- [Performance](#performance)
- [Architecture](#architecture)
- [Building from source](#building-from-source)
- [Hardware compatibility](#hardware-compatibility)
- [Troubleshooting](#troubleshooting)
- [License](#license)

## What you get

| Area | What it does |
|------|--------------|
| **GUI** | Four-page console — Overview (live temps, thermal history, fan curve plot, battery), Control (power cap, thermal profile, fans), Lighting (per-zone RGB), System (battery health, BIOS settings, display rates, diagnostics, build info). Dark single-accent design, keyboard-navigable, works from a 940 px window, scales 0.8×–2.0×. |
| **CLI** | One-shot `status`, temperature sensors, fan control, power cap, thermal profiles, RGB zones, BIOS read/write, battery health, hardware probe (`--json` supported). |
| **Fans** | Max mode, manual percentage hold, and a *verified* hand-back-to-firmware. The release path sends HP's own `EWMA_ALGO` scheme — see [Fan control](#fan-control) for why a naive "turn max off" lies to you. |
| **Power** | CPU power cap with a supply-aware ceiling read from the firmware — never hardcoded. Every write is readback-verified; only the *verified* outcome is persisted. |
| **Thermal** | Balanced / Performance / Cool, written and read back from the firmware. |
| **Keyboard** | Per-zone RGB with byte-exact read-modify-write; a failed write restores the original table. Zone count comes from the firmware's colour table, not from an enum that lies. |
| **BIOS** | Read and write BIOS settings through the WMI *instance path* (class-level calls fail with rc 5 on this hardware; this project got that right). |
| **Battery** | Health %, wear grade, charge rate, cycle count, temperature — more than OGH shows. |
| **Safety** | Saved cap and thermal profile re-applied at logon only after readback verification. Interrupted operations leave a recovery marker, not a broken state. Crash reports survive the crash that writes them. |
| **Self-updating** | Checks GitHub Releases, verifies a SHA-256 digest *before* renaming anything, stages a per-asset install plan. Pre-release tags are never offered as stable updates. |

## Install

### Installer

1. Install [PawnIO](https://pawnio.eu) — the signed kernel module needed for
   direct EC access. Without it the GUI/CLI still read the WMI channels, but
   EC-backed sensors degrade and are labelled as such.
2. Download `omenmon-setup.exe` from
   [Releases](https://github.com/michaelxdips/OmenMonX/releases/latest) and run
   it **as administrator**. It:
   - copies the four binaries to `C:\Program Files\OmenMon`,
   - adds that folder to your **user** PATH,
   - registers a logon task that re-applies your saved power cap and thermal
     profile (the firmware resets both on every boot),
   - adds an Add/Remove Programs entry with a working uninstall.

### Portable (no install)

Download `OmenMonX-portable.zip`, unzip anywhere, run `omenmon-gui.exe` or
`omenmon.exe` from the folder. Nothing touches PATH, autostart or the
registry. The zip contains its own `SHA256SUMS.txt`; the release page also
publishes the digest of the zip itself.

### SmartScreen and unsigned executables

The binaries are currently **not code-signed**, so Windows SmartScreen will
show "Windows protected your PC" on first run of the installer. That is the
expected friction, not an error. To run anyway:

1. Click **More info** on the SmartScreen dialog.
2. Click **Run anyway**.

Or, for the portable zip, unblock the files before running (PowerShell):

```
Unblock-File .\omenmon-gui.exe
Unblock-File .\omenmon.exe
```

Only download from the official
[Releases](https://github.com/michaelxdips/OmenMonX/releases/latest) page and
compare the published SHA-256 digests. Code signing is under investigation;
no purchase decision has been made.

### Upgrading

The GUI checks GitHub Releases on launch and offers the update; the digest is
verified before anything is replaced. Or just overwrite with the portable zip.

## CLI reference

```
omenmon status     one-shot dashboard (temps + fan + battery summary)
omenmon temps      temperature sensors
omenmon fans       fan count and per-fan level
omenmon curve      firmware fan curve (42 points, read-only)
omenmon power      CPU power limits (0x2A block) and the writable window
omenmon cap 45     set the CPU power cap (supply-aware, readback-checked)
omenmon thermal    show the current thermal mode
omenmon thermal performance|cool|balanced
omenmon fan max    spin the fans to maximum
omenmon fan auto   hand the fans back to the firmware
omenmon fan 45     hold a manual 45 % fan speed
omenmon kbd        show the keyboard colour table
omenmon kbd 0 --color FF8800   set one keyboard zone
omenmon bios list             list BIOS settings
omenmon bios <name>           read one setting
omenmon bios <name> --set v   write one setting
omenmon battery    health, wear, charge rate, cycles, temperature
omenmon probe      hardware capability matrix (add --json for machine-readable output)
omenmon apply      re-apply the saved cap and profile (logon task)
omenmon daemon     long-running single-instance monitor
omenmon restore-defaults   clear saved settings; firmware re-applies its own
```

Writes need an elevated terminal. `status` and `battery` work unelevated for
the parts that do not need a token — the output says which parts degraded and
why. Usage errors are reported *before* elevation is attempted, so a typo does
not come back disguised as a permissions problem.

`cap` is precise about its evidence: the values verified under load on the
230 W brick are 65, 50, 40, 30, 25 and 20 W (5–25 W on USB-C). A request in
the never-swept 51–64 W band is sent as-is, verified by readback, and marked
as unverified territory in the output — not silently treated as a documented
ceiling.

## GUI reference

- **Overview** — CPU / GPU / hotspot temps, live clock, power cap, thermal
  profile, a rolling thermal-history chart, fan status with the firmware curve
  plot, and a battery card (charge, source, wear, cycles, temperature).
- **Control** — power cap slider bound to the supply-aware ceiling, thermal
  profile buttons, fan max / release / manual percentage. Every write shows
  the firmware's verified outcome, not the request.
- **Lighting** — per-zone colour pickers driven by the actual zone count, plus
  backlight brightness.
- **System** — battery health detail, BIOS settings list with per-row write
  results, display refresh-rate controls, diagnostics dump, update check,
  build info.

The tray icon offers Show/Quit; closing the window hides to tray by default.
The GUI never elevates itself and never launches UAC prompts — if you run it
unelevated you get read-only behaviour with a clear explanation.

## How the hardware actually works here

This section is the measured firmware ground truth for board 8C9C. It exists
because most of it contradicts either OGH's UI, common GitHub folklore, or an
earlier wrong claim in this very README.

### Fan control

The write channel is EC/WMI command `0x2E`, whose first byte is HP's own
`FanSetScheme` enum (read from `PerformanceControl.dll` by reflection; values
confirmed):

| Scheme | Value | Effect (measured) |
|--------|-------|-------------------|
| `EWMA_ALGO` | 0 | The firmware's own algorithm. Level dips to 0 and ramps back — this is *handing control back*. |
| `SLIDER` | 1 | Holds the commanded percentage in bytes 1–3. Manual control. |
| `IDLE_AUTO` | 2 | Quiet, load-independent. |
| `NO_SET` | 3 | No-op scheme. |

`0x27 {1,0,0,0}` is Max mode. **Turning it off does not release the fans** —
the level stays pinned at maximum until a `0x2E` write arrives. Measured from
max, a low slider write fell 59 → 34 and then *latched* at 39, holding the
fans at a manual speed while looking like a release. That is why
`omenmon fan auto` sends `EWMA_ALGO` and nothing else.

An earlier version of this README claimed manual fan control "awaits a signed
kernel module". That was wrong — the probe behind it sent the fan table's dB
index where the firmware reads a scheme byte. The write channel was open the
whole time. Lesson recorded: check the vendor's own call site before declaring
a channel dead.

### Power limits (the honest version)

The 0x2A block carries PL1/PL2/PL4/PL-with-GPU at bytes 2/3/4/7 (byte 0 is
*not* PL1 — it fluctuates 60–62 and is not watts). The supply profile is
inferred from the stock PL1 itself: there is no clean adapter flag (`0x28`
DefaultPL4 reads 190 W on both supplies; Legacy `0x0F` AdapterStatus reads 1
on both).

| Supply | PL1 range | PL4 ceiling | Evidence |
|--------|-----------|-------------|----------|
| 230 W brick | 5–50 W + 65 W | 55 W | 65/50/40/30/25/20 accepted and held under a real 16-thread load, readback-checked |
| USB-C PD | 5–25 W | 25 W | same method |

**The 51–64 W band has never been swept.** A request there is sent as-is and
verified by readback, but no load-tested accepted/rejected matrix exists yet;
the CLI says so when you land in the band. Writing an over-limit value is also
how stock limits are restored: the firmware re-applies its own block.

Every cap write follows the same protocol: read the block first (it carries
the supply profile and the plausibility check), write, sleep for firmware
settle, read back, and only then persist — persisting the *held* value, so a
firmware clamp is never replayed at the unclamped request.

### Thermal profiles

`0x1A {0xFF, mode, 0, 0}` with the remap 0 → `0x30`, 1 → `0x31`,
2 → `0x50` (balanced / performance / cool). The current mode is read from the
0x28 status byte. Note the quirk: the status decoder reports performance as
an odd mode code, which the UI normalises.

### Keyboard RGB

A 128-byte colour table. Zones live at offset `0x1A + 3*i`, each zone block
starts with `0x03`, and the **zone count is `colorTable[0]`** — not the
`0x2B` keyboard subtype, which reports "OneZoneWithNumpad" (0x04) on a machine
with three distinct colour zones. All writes are read-modify-write with a
byte-exact restore on failure: a zone that cannot be verified is rolled back,
never left half-written. A zone index is also validated against the declared
count before it can reach the buffer — `0x1A + 3*zone` stays inside 128 bytes
up to zone 36, and the bytes past the declared count are *other table fields*;
writing one would corrupt the table in a way that still reads back as success.

### Battery

Read through the standard power APIs, plus temperature. The GUI publishes an
`on_battery` flag used by the battery-saver repaint floor (see
[Performance](#performance)). Charge-limit control is a verified dead end —
see the next section.

## What does not work

Refused by the firmware on 8C9C — verified, not assumed:

| Feature | Why |
|---------|-----|
| Fan **curve** writing (`0x32`) | Rejected. Speed control (`0x2E`) and curve writing are different commands — do not conflate them. |
| Battery charge limit | `Battery Care Function` returns rc 5 for all 27 encodings tried. No ACPI threshold exists on this board either. Dead end, twice confirmed. |
| GPU mode switch (`0x52`) | BIOS error 5. |
| BIOS undervolt / OC (`0x35`) | WMI error 31. |

Two more honesty notes:

- The EC tachometer at `0xC4` reads a frozen 1249 rpm on this board, so it is
  never displayed as live RPM — live fan level comes from `0x2D`.
- The EC mutex (`Global\Access_EC`, the LibreHardwareMonitor userland
  convention) serializes *cooperating peers only*. WMI-side firmware writes
  (OGH running simultaneously, for example) are NOT serialized by it, and the
  code is honest about that. Quantifying the exposure and choosing an
  arbitration design is the v0.4 headline work; the
  `arbitration_probe` measurement tool exists for exactly this.

## Safety model

- **Never self-elevates.** No UAC prompts from code, ever. Writes fail with an
  explanation of what to run elevated instead.
- **Verified outcomes only.** A value is persisted after the firmware confirms
  it by readback, not when the write is accepted.
- **Recovery markers.** Interrupted operations leave a marker the next run
  picks up, instead of leaving a half-applied state.
- **Crash reporting survives the crash.** The panic hook writes its artifact
  before process death, and the GUI surfaces it on next launch.
- **Atomic config.** TOML config is written atomically with a `.bak`; a corrupt
  file is preserved for recovery before defaults take over, and an
  out-of-bounds fan curve falls back with a warning rather than silently.

## Performance

The GUI is a full-CPU egui renderer, so its idle cost is managed, not ignored:

- 250 ms repaint floor while focused (clocks and status stay fresh);
- **battery saver**: unfocused *and* on battery stretches the floor to 2 s;
- the fan-curve plot's point series are cached against a generation counter
  and rebuilt only when the firmware table actually refreshes, not every frame;
- a debug HUD (`OMENMON_PERF=1` env var) prints repaint rate, frame interval
  and per-frame busy time every 5 s, so the numbers above can be re-measured
  rather than trusted.

Targets for the current pass: <1% CPU idle focused, <0.5% idle unfocused on
AC. The CLI `daemon` is a single-instance monitor (atomic lockfile, battery
alerts) meant for unattended use.

## Architecture

Five crates:

| Crate | Role |
|-------|------|
| `hp-ec` | Direct EC access via PawnIO: port I/O, mutex discipline, bus resync |
| `hp-wmi` | The `hpqBIntM` WMI transport: payload encoding, instance-path ExecMethod calls, COM lifetime, typed commands (`hpCpsPubGetSetCommand`, GUID `{928b5a30-ad7c-4c36-946a-1300a80ce7e1}`) |
| `hp-core` | Config (atomic TOML with `.bak`), fan curves with hysteresis, battery, the polling engine |
| `omenmon-cli` | The CLI, the daemon (single-instance, atomic lockfile), the installer |
| `omenmon-gui` | The egui console, sampler thread (the only owner of the BIOS handle), tray, updater, crash reporting, perf HUD |

CI enforces on every push: the full test suite, clippy with warnings denied,
and a probe-leak gate proving a plain release build contains exactly the four
shipped binaries (`omenmon.exe`, `omenmon-gui.exe`, `omenmon-setup.exe`,
`daemon.exe`). Reverse-engineering probes live behind `--features probes` and
cannot reach a release build. Releases are tag-triggered and publish the four
exes plus a portable zip with SHA-256 checksums; `CHANGELOG.md` is regenerated
from tag history, grouped by commit type.

## Building from source

```
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

You need the Windows MSVC toolchain. PawnIO is only needed at *runtime*, not
to compile. Binaries land in `target/release/`. The reverse-engineering probe
binaries additionally build with:

```
cargo build --release -p omenmon-cli --features probes
```

These are measurement tools, not features — they are excluded from release
artifacts by a CI gate.

## Hardware compatibility

**Everything in this README is verified on one board: Victus 16, board 8C9C.**
No compatibility promise is made for any other board, and none will be until
per-board data exists.

To help map the landscape, run from an elevated terminal:

```
omenmon probe --json
```

and attach the JSON object to a compatibility issue. Each row is one WMI
method with a tri-state result (works / refused / not applicable), which is
enough to tell whether a board shares this firmware interface without anyone
guessing.

## Troubleshooting

| Symptom | Explanation and fix |
|---------|---------------------|
| "the BIOS interface needs administrator rights" | WMI `hpqBIntM` calls need an elevated token. Run from an elevated terminal; `status` and `battery` still work unelevated. |
| GUI shows sensors as unavailable | Same elevation rule. The UI states which parts degraded and why instead of showing zeros. |
| SmartScreen blocks the installer | Expected for unsigned binaries — see [SmartScreen](#smartscreen-and-unsigned-executables). |
| Saved cap did not survive reboot | The firmware resets its limits every boot; re-application runs from the logon task the installer registers. Portable users can call `omenmon apply` themselves. |
| Fans stuck at high speed after Max mode | Only a `0x2E` scheme write releases them. Use `omenmon fan auto` (or the GUI release button), which sends `EWMA_ALGO`. |
| Clock shows "sampling failed" | The live-clock counter comes from typeperf; one broken counter latches the error and the last good MHz is kept. Other channels are unaffected. |
| Update offered then failed digest check | Do not retry the same asset. Re-download from the Releases page; the digest check exists to refuse tampered or truncated downloads. |

## License

GPL-3.0-or-later. Derived from
[OmenMon](https://github.com/OmenMon/OmenMon) (GPL-3) hardware knowledge;
all Rust code is original.
