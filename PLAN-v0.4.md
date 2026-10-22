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
- [ ] Measure the exposure: run OGH + omenmon-gui concurrently on the
      reference machine, drive RGB + fan writes from both, watch for corrupt
      color tables or misdirected writes over 30 minutes.
- [ ] Design decision (pick one):
      a. **Single-owner daemon**: all writes funnel through daemon.exe via a
         named-pipe protocol; GUI/CLI become clients. Biggest change, cleanest
         result, aligns with the existing daemon.
      b. **Global write-lock file** with pid+timestamp heartbeat held during
         any write transaction (WMI or EC). Smaller change, cooperative,
         degrades to v0.3.1 behavior if a foreign process ignores it.
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
      (real 16-thread load, not a PowerShell job loop).
- [ ] If accepted: document the true ceiling per supply, keep 65.
- [ ] If rejected in a band: make `cap` clamp to the verified band and say
      so in `--help`, with the measured reason.
- [ ] Ship in v0.3.2 regardless (it is a documentation/UX fix).

## Theme 3 — `hysteresis_c`: wire or delete

**Problem.** `Config.fan_pct_for` and the TOML schema carry `hysteresis_c`,
parsed and persisted but read by nothing. It misleads anyone editing
config.toml.

**Plan.**
- [ ] Decide semantics: hold the current fan target until temperature moves
      more than N °C against the direction of change. If wanted, implement
      in `hp-core::engine` / fan_pct_for with a unit test using a synthetic
      temp series.
- [ ] If not wanted: delete the field, bump config parsing tolerance
      (unknown fields already fall back cleanly).
- [ ] Owner decision required — this is the only open design question.

## Theme 4 — GUI performance pass

**Problem.** No measured complaint, but the sampler wakes at 500 ms and
repaints at 120 ms minimum; egui full-cpu repaints on a laptop cost real
battery.

**Plan.**
- [ ] Measure: etw or a simple frame counter in the build — repaint rate,
      CPU% idle vs active vs animating write status.
- [ ] Cheap wins first: raise the idle repaint floor (request_repaint_after
      already exists; audit every call site), gate the fan-curve plot
      repaint to sampler ticks rather than the 120 ms loop.
- [ ] Consider a "battery saver" toggle: 2 s tick when on battery + window
      unfocused, restoring on focus.
- [ ] Target: <1% CPU at idle, <3% while monitoring.

## Theme 5 — Release hygiene

- [ ] Tag v0.3.2 with Themes 2+3 whatever they resolve to.
- [ ] Add a `CHANGELOG.md` generated from commit history at tag time (CI
      step, human-reviewed) — release notes are currently auto-generated
      commit lists, which is fine but flat.
- [ ] Consider code signing (self-signed cert + docs for the
      SmartScreen prompt) — unsigned exes are the #1 friction for new users.
- [ ] Update README badges to pull from the latest release tag, not hardcode.

## Theme 6 — Broader hardware support (stretch, next cycle)

- [ ] The code is 8C9C-verified. Survey which other Victus/Omen boards share
      the `hpqBIntM` interface and which feature gates differ (OGH
      DeviceList.json gives the map: Orisa10/A/A11 have FanControl).
- [ ] Probe matrix harness already exists (`omenmon probe`); make its
      output machine-readable JSON and ask users to attach it to issues.
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
