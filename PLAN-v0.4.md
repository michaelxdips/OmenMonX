# OmenMon-rs — Next Iteration Plan (v0.4)

Written 2026-10-22 after the v0.3.1 correctness release. v0.3.1 shipped ~74
audited fixes across hardware safety, the updater trust path, GUI state and
layout, and CLI hardening. This plan is what comes next, in priority order.

## Guiding rules (unchanged)

1. Every firmware claim is empirically verified before it ships.
2. Persist verified outcomes only — never a request the firmware may refuse.
3. Never suggest `Start-Process -Verb RunAs` for anything.
4. The 4-binary shape is a CI-enforced invariant.
5. Repo carries source + real docs only. No handoffs, no test evidence.

---

## Theme 1 — Cross-process EC arbitration (the real architectural gap)

**Problem.** The `Global\Access_EC` mutex serializes cooperating userland
peers. It does NOT serialize WMI-side firmware writes (OGH running
simultaneously, for instance) against our port-I/O EC transactions. Two
writers can interleave. The v0.3.1 work made the mutex honest and the bus
resync robust, but the gap is real.

**Plan.**
- [x] Measure the exposure: arbitration_probe.rs (behind `--features probes`)
      runs roles A and B concurrently over the RGB RMW path. *Awaiting a
      user-run elevated session for the results.*
- [ ] Design decision (pick one):
      a. **Single-owner daemon**: all writes funnel through daemon.exe via a
         named-pipe protocol; GUI/CLI become clients. Biggest change, cleanest
         result, aligns with the existing daemon.
      b. **Global write-lock file** with pid+timestamp heartbeat held during
         any write transaction (WMI or EC). Smaller change, cooperative,
         degrades to v0.3.1 behavior if a foreign process ignores it.
      *Blocked on the probe's anomaly data: 0 anomalies justifies (b),
      anomalies require (a).*
- [ ] Implement, test with the concurrency harness from the measurement step.
- [ ] Ship as v0.4.0 headline if (a); v0.3.2 if (b).

## Theme 2 — The PL1 acceptance gap (51–64 W)

**Problem.** The firmware's accepted single-value range is verified at
5–50 W (USB-C 5–25 W) and 65 W on the brick; the memory of the project
records 65/50/40/30/25/20 all accepted at idle, but 51–64 W was never
probed. The CLI clamps to 65. If a user asks for 57 W the outcome is
unverified.

**Plan.**
- [ ] One probe session: sweep 51–64 W on the brick, record
      accepted/held/rejected per value with the standard outcome harness
      (real 16-thread load, not a PowerShell job loop). *Awaiting user-run
      elevated session.*
- [x] Interim honesty fix shipped without the sweep: `cap --help` states the
      verified points (65/50/40/30/25/20) and labels 51–64 W as unswept; a
      runtime note marks in-band requests, the readback remains the only
      verification (commit e454ef2).
- [ ] If accepted: document the true ceiling per supply, keep 65.
- [ ] If rejected in a band: make `cap` clamp to the verified band and say
      so in `--help`, with the measured reason.

## Theme 3 — `hysteresis_c`: wire or delete

**Problem.** `Config.fan_pct_for` and the TOML schema carry `hysteresis_c`,
parsed and persisted but read by nothing. It misleads anyone editing
config.toml.

**Plan.**
- [x] Wire it: `hysteresis_c` semantics implemented in `hp-core::engine`
      (TargetState anchor + band hold), pure and unit-tested with a synthetic
      temp series (jitter holds, real climbs still move, missing readings
      hold, curve floor on first poll). Shipped in the v0.3.x cycle.

## Theme 4 — GUI performance pass

**Problem.** No measured complaint, but the sampler wakes at 500 ms and
repaints at 120 ms minimum; egui full-cpu repaints on a laptop cost real
battery.

**Plan.**
- [x] Battery-saver floor shipped (250 ms → 2 s when unfocused + on battery).
- [x] Instrument: `OMENMON_PERF=1` debug HUD prints repaint rate, frame
      interval and per-frame busy time every 5 s (commit d07d22d). *Numbers
      pending a user-run session across focused/unfocused × AC/battery ×
      idle/animating.*
- [x] Cheap win found by inspection and fixed: the fan-curve plot rebuilt its
      point series every 250 ms frame although the table refreshes every 20th
      sampler tick — series now cached against a generation counter (same
      commit).
- [ ] Run the measurement matrix and fix what the numbers show.
- [ ] Target: <1% CPU idle focused, <0.5% idle unfocused on AC.

## Theme 5 — Release hygiene

- [x] Tag v0.3.2 — done before this session; CI green, portable zip + changelog live.
- [x] CHANGELOG.md generated at tag time — shipped with v0.3.2.
- [x] Group the generated changelog by commit type (feat/fix/perf/docs/build/
      probe; unrecognized prefixes fall to Other) — pattern rehearsed locally
      against all three existing tags (commit 90a8ef1).
- [x] README documents the SmartScreen override for unsigned binaries and
      digest verification (commit 836cf04). Code signing: still investigating;
      no cert purchased.
- [x] README badges no longer hardcode stale values (test count tracks the
      current suite).

## Theme 6 — Broader hardware support (stretch, next cycle)

- [ ] The code is 8C9C-verified. Survey which other Victus/Omen boards share
      the `hpqBIntM` interface and which feature gates differ (OGH
      DeviceList.json gives the map: Orisa10/A/A11 have FanControl).
- [x] Probe matrix harness exists; made machine-readable (`omenmon probe
      --json`, one JSON object, tri-state rows) — commit bd81b49, smoke-tested
      live on the unelevated path.
- [x] README asks compatibility reporters to attach the JSON and states that
      nothing is promised until per-board data exists.
- [ ] Do NOT promise compatibility; measure per-board first.

## Explicitly not planned

- Fan curve writing — firmware refuses 0x32 on this board. Revisit only if
  a BIOS update changes that.
- Battery charge limit — rc 5 for all 27 encodings; no ACPI threshold on
  this board either. Dead end, twice confirmed.
- GPU mode switch, undervolt — both firmware-refused.
- Companion mobile app, tray-only mode, theming beyond the single dark
  palette — scope creep without user demand.

## Suggested sequencing

| Release | Contents |
|---------|----------|
| v0.3.2 | Theme 2 (PL1 sweep + clamp), Theme 3 (wire-or-delete), README badge fix |
| v0.4.0 | Theme 1 (arbitration, whichever design), Theme 4 (perf pass) |
| v0.5.0 | Theme 5 (signing, changelog) + Theme 6 starts if the survey pans out |
