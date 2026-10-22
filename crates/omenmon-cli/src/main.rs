use clap::{Parser, Subcommand};
use hp_wmi::Bios;
use hp_core as _;

mod console;
mod daemon {
    #[path = "../daemon_bin.rs"]
    pub mod daemon_impl;
    pub use daemon_impl::run;
}

#[derive(Parser)]
#[command(name = "omenmon", version, about = "Victus/Omen hardware control — Rust edition")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Probe hardware capabilities and print the feature matrix
    Probe,
    /// Read all temperature sensors
    Temps,
    /// Show fan info (count, type, level)
    Fans,
    /// Battery health, wear, charge state
    Battery,
    /// Live status one-shot (temps + fan + battery summary)
    Status,
    /// CPU power limits (0x2A block) and the writable window
    Power,
    /// Set the CPU power cap in watts (verified 5..25 W, readback-checked)
    Cap {
        /// Watts to apply, 5..25
        watts: u8,
    },
    /// Show the firmware fan curve (42 points, read-only)
    Curve,
    /// Thermal mode: show current, or set balanced | performance | cool
    Thermal {
        /// balanced | performance | cool
        mode: Option<String>,
    },
    /// Fan speed: max, auto (hand back to firmware), or a manual percentage
    Fan {
        /// max | auto | 0-100
        setting: Option<String>,
    },
    /// Keyboard color table: show, or set one zone's RGB
    Kbd {
        /// Show the color table, or a zone number 0..3 to set
        zone: Option<u8>,
        /// Hex color like FF8800 (only with --zone)
        #[arg(long)]
        color: Option<String>,
    },
    /// Re-apply the saved power cap and thermal mode (for a login task)
    Apply,
    /// Clear saved preferences and restore firmware defaults
    RestoreDefaults,
    /// BIOS settings: list, or read/write one setting
    Bios {
        /// Setting name to read, or to write together with --set
        name: Option<String>,
        /// New value (requires a name)
        #[arg(long)]
        set: Option<String>,
    },
    /// Long-running daemon (single-instance, battery alerts, curve loop)
    Daemon,
}

fn main() -> anyhow::Result<()> {
    console::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Probe => probe()?,
        Command::Temps => temps()?,
        Command::Fans => fans()?,
        Command::Battery => battery()?,
        Command::Status => status()?,
        Command::Power => power()?,
        Command::Cap { watts } => power_cap(watts)?,
        Command::Curve => fan_curve()?,
        Command::Thermal { mode } => thermal(mode.as_deref())?,
        Command::Fan { setting } => fan(setting.as_deref())?,
        Command::Kbd { zone, color } => kbd(zone, color.as_deref())?,
        Command::Apply => apply_saved()?,
        Command::RestoreDefaults => restore_defaults()?,
        Command::Bios { name, set } => bios_settings(name.as_deref(), set.as_deref())?,
        Command::Daemon => daemon::run()?,
    }
    Ok(())
}

fn battery() -> anyhow::Result<()> {
    let info = hp_core::battery::read_battery();
    println!("omenmon battery — HP Victus");
    println!("{}", "=".repeat(50));
    println!(
        "charge:          {:>3} %",
        info.charge_pct.map(|v| v.to_string()).unwrap_or("n/a".into())
    );
    println!(
        "AC:              {}",
        if info.ac_online { "plugged in" } else { "on battery" }
    );
    println!(
        "state:           {}",
        if info.charging {
            "charging"
        } else if info.ac_online {
            "idle on AC (full)"
        } else {
            "discharging"
        }
    );
    println!(
        "health:          {:>3} %  (full {} mWh / design {} mWh)",
        info.health_pct.map(|v| v.to_string()).unwrap_or("n/a".into()),
        info.full_charge_mwh.map(|v| v.to_string()).unwrap_or("?".into()),
        info.design_mwh.map(|v| v.to_string()).unwrap_or("?".into()),
    );
    if let Some(h) = info.health_pct {
        let grade = match h {
            90..=100 => "excellent",
            80..=89 => "good",
            60..=79 => "worn",
            _ => "degraded — consider replacement",
        };
        println!("wear grade:      {grade}");
    }
    println!(
        "cycle count:     {}",
        info.cycles.map(|v| v.to_string()).unwrap_or("n/a".into())
    );
    println!(
        "voltage:         {} mV",
        info.voltage_mv.map(|v| v.to_string()).unwrap_or("n/a".into())
    );
    println!(
        "rate:            {} mW ({})",
        info.rate_mw.map(|v| v.to_string()).unwrap_or("n/a".into()),
        match info.rate_mw {
            Some(r) if r > 0 => "charging",
            Some(r) if r < 0 => "draining",
            _ => "steady",
        }
    );
    if let Some(t) = info.temp_c {
        println!("temperature:     {}", console::temp_c1(t));
    }
    Ok(())
}

fn status() -> anyhow::Result<()> {
    let cfg = hp_core::Config::load();
    let bios = bios().ok();
    let io = hp_ec::ecio::EcIo::connect().ok();
    let st = match bios.as_ref() {
        Some(b) => hp_core::engine::Engine::new(cfg).poll(b, io.as_ref()),
        None => {
            // degraded: battery-only status
            let b = hp_core::battery::read_battery();
            println!("omenmon status (degraded — elevated run needed for temps/fans)");
            println!("{}", "=".repeat(50));
            println!(
                "battery: {} %, health {}, {}",
                b.charge_pct.map(|v| v.to_string()).unwrap_or("?".into()),
                b.health_pct.map(|v| format!("{v} %")).unwrap_or("?".into()),
                if b.ac_online { "AC" } else { "battery" }
            );
            return Ok(());
        }
    };
    let s = st.sample;
    println!("omenmon status");
    println!("{}", "=".repeat(50));
    println!(
        "CPU: {}   GPU: {}   hotspot: {}",
        s.cpu.unwrap_or(0.0),
        s.gpu.unwrap_or(0.0),
        s.gpu_hotspot
            .map(console::temp_c)
            .unwrap_or("n/a".into())
    );
    // The EC tachometer at 0xC4 reads a frozen 1249 rpm on this board, so it is
    // not printed: a constant presented as a measurement is worse than no number.
    // The level byte is live and is the honest readout.
    println!(
        "fan: level {}",
        s.fan_level_pct
            .map(|v| format!("{v} %"))
            .unwrap_or("n/a".into())
    );
    println!(
        "curve target: {} % (read-only; the curve cannot be written)",
        st.target_fan_pct
    );
    let b = hp_core::battery::read_battery();
    println!(
        "battery: {} %, health {}, {}",
        b.charge_pct.map(|v| v.to_string()).unwrap_or("?".into()),
        b.health_pct.map(|v| format!("{v} %")).unwrap_or("?".into()),
        if b.ac_online { "AC" } else { "battery" }
    );
    Ok(())
}

fn power() -> anyhow::Result<()> {
    use hp_wmi::CpuPowerLimits;
    let b = bios()?;
    println!("omenmon power - CPU power limits (verified 2026-10-21)");
    println!("{}", "=".repeat(52));

    // 0x2A is a LIMITS block, not live power. Measured: under a 16-thread load
    // byte 0 went 64 -> 62, i.e. it fell. It must not be shown as watts.
    let limits = match b.get_cpu_power() {
        Ok(l) if l.is_plausible() => l,
        Ok(_) => anyhow::bail!("0x2A returned no plausible limits"),
        Err(e) => anyhow::bail!("{e} (elevated run required)"),
    };
    println!("PL1 (long):     {:>3} W", limits.pl1);
    println!("PL2 (short):    {:>3} W", limits.pl2);
    println!("PL4 (peak):     {:>3} W", limits.pl4);
    println!("With GPU:       {:>3} W", limits.pl_with_gpu);
    println!(
        "Supply:         {}",
        if limits.on_brick() {
            "230 W brick profile (PL1 ceiling 65 W)"
        } else {
            "USB-C PD profile (PL1 ceiling 25 W)"
        }
    );
    println!(
        "Writable range: {}..{} W PL1, PL4 up to {} W  (firmware clamps above)",
        CpuPowerLimits::MIN_WATTS,
        limits.pl1_ceiling(),
        limits.pl4_ceiling()
    );
    println!("Restore stock:  writes an over-limit value; the firmware re-applies its own block");

    if let Ok(io) = hp_ec::ecio::EcIo::connect() {
        if let Ok(page) = io.with_lock(2000, |io| hp_ec::eram::Eram::new(io).read_range(0xC6, 3)) {
            println!("\nEC 0xC6..C8:    {} {} {}  (not a supply flag: drifts on both)", page[0], page[1], page[2]);
        }
    }
    println!("\nNote: 0x2A is a settings block. It is NOT instantaneous package power.");
    Ok(())
}

/// Set the CPU power cap, clamped to the ceiling the current supply allows.
///
/// The ceiling is read from the firmware first, so the same command is correct on
/// USB-C PD (25 W) and on the 230 W brick (65 W). Clamping to a constant is what
/// made an earlier build throttle a brick-powered machine to 25 W.
fn power_cap(watts: u8) -> anyhow::Result<()> {
    use hp_wmi::CpuPowerLimits;
    let b = bios()?;

    // Read the block first: it carries the supply profile.
    let before = b.get_cpu_power()?;
    let ceiling = before.pl1_ceiling();
    if !before.is_plausible() {
        anyhow::bail!("0x2A returned no plausible limits; refusing to write blind");
    }

    let applied = b.set_power_limit(watts)?;
    if applied != watts {
        println!(
            "requested {watts} W -> clamped to {applied} W (this supply accepts {}..{} W)",
            CpuPowerLimits::MIN_WATTS,
            ceiling
        );
    }
    // Verify by readback: the firmware is the authority, not our request.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let after = b.get_cpu_power()?;
    println!("applied: PL1={} PL2={} PL4={} W", after.pl1, after.pl2, after.pl4);
    if after.pl1 != applied {
        anyhow::bail!(
            "firmware did not accept {applied} W (reads back as {} W)",
            after.pl1
        );
    }
    println!("verified by readback.");

    // Persist so the choice survives a reboot. The firmware resets its limits on
    // every boot, so a saved value is only meaningful if something re-applies it.
    let mut cfg = hp_core::Config::load();
    cfg.set_power_cap(Some(applied));
    match cfg.save() {
        Ok(_) => println!("saved: will re-apply {applied} W on `omenmon apply`"),
        Err(e) => println!("warning: could not save preference: {e}"),
    }
    Ok(())
}

/// Re-apply the persisted power cap and thermal mode.
///
/// The firmware resets both on every boot, so this is what a login task or the
/// daemon should call. With no saved preference it changes nothing.
fn apply_saved() -> anyhow::Result<()> {
    let cfg = hp_core::Config::load();
    let b = bios()?;
    let mut did_anything = false;

    if let Some(w) = cfg.power_cap_watts {
        // The saved value was chosen on whatever supply was connected then. On a
        // different supply the ceiling moves, so say so rather than clamping in
        // silence: a 65 W preference re-applied on USB-C would quietly become 25.
        let before = b.get_cpu_power()?;
        let ceiling = before.pl1_ceiling();
        if w > ceiling {
            println!(
                "power cap: saved {w} W exceeds this supply's {ceiling} W ceiling; \
                 the firmware will hold {ceiling} W"
            );
        }
        let applied = b.set_power_limit(w)?;
        std::thread::sleep(std::time::Duration::from_millis(600));
        let after = b.get_cpu_power()?;
        println!(
            "power cap: requested {w} W -> firmware holds {} W (supply ceiling {ceiling} W)",
            after.pl1
        );
        if after.pl1 != applied {
            println!("  warning: firmware did not hold the requested value");
        }
        did_anything = true;
    }

    if let Some(name) = cfg.thermal_mode_parsed() {
        use hp_wmi::ThermalMode;
        let m = match name {
            "performance" => ThermalMode::Performance,
            "cool" => ThermalMode::Cool,
            _ => ThermalMode::Balanced,
        };
        let observed = b.set_fan_mode_verified(m)?;
        println!(
            "thermal: {name} -> firmware status {observed:#04X} (expected {:#04X})",
            m.expected_status()
        );
        did_anything = true;
    }

    if !did_anything {
        println!("nothing saved to apply (set one with `omenmon cap <w>` or `omenmon thermal <mode>`)");
    }
    Ok(())
}

/// Clear the saved preferences and put the firmware back to its own defaults.
///
/// The power block is restored by writing an over-limit value, which makes the
/// firmware re-apply **its own** stock block. That is 65/55/55 on the 230 W brick
/// and 25/25/25 on USB-C PD, and it is the only restore that is correct on both.
/// Writing a hardcoded 25 W here was a real bug: on a brick it left the machine
/// throttled at 25 W and reported success.
fn restore_defaults() -> anyhow::Result<()> {
    let b = bios()?;
    let before = b.get_cpu_power()?;
    b.reset_power_limits()?;
    b.set_fan_mode(hp_wmi::ThermalMode::Balanced.code())?;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let after = b.get_cpu_power()?;
    println!(
        "power cap: firmware stock block restored -> PL1={} PL2={} PL4={} W",
        after.pl1, after.pl2, after.pl4
    );
    if after.pl1 != before.pl1 {
        println!(
            "  (was PL1={} W before the reset; the firmware chose {})",
            before.pl1, after.pl1
        );
    }
    println!("thermal: balanced");
    let mut cfg = hp_core::Config::load();
    cfg.set_power_cap(None);
    cfg.set_thermal_mode(None);
    match cfg.save() {
        Ok(_) => println!("saved preferences cleared"),
        Err(e) => println!("warning: could not clear preferences: {e}"),
    }
    Ok(())
}

/// Show the firmware fan curve.
fn fan_curve() -> anyhow::Result<()> {
    let b = bios()?;
    let t = b.get_fan_table_parsed()?;
    println!(
        "fan curve: {} fans, {} points (read-only; speed is set via 0x27/0x2E, not by writing the curve)",
        t.fan_count, t.level_count
    );
    println!("{}", "=".repeat(52));
    print!("{:>3} {:>6}", "pt", "level");
    for f in 0..t.fan_count {
        print!(" {:>7}", format!("fan{} %", f + 1));
    }
    println!();
    for (i, p) in t.points.iter().enumerate() {
        print!("{:>3} {:>6}", i, p.level);
        for f in 0..t.fan_count as usize {
            match p.duty(f, t.fan_count) {
                Some(d) => print!(" {d:>7}"),
                None => print!(" {:>7}", "-"),
            }
        }
        println!();
    }
    println!(
        "\npeak duty {}%   monotonic: {}",
        t.max_duty(),
        if t.is_monotonic() { "yes" } else { "NO" }
    );
    println!("Column 'level' is a firmware level index (dB), not a temperature.");
    Ok(())
}

/// Switch thermal mode and report the firmware's actual state.
fn bios_settings(name: Option<&str>, set: Option<&str>) -> anyhow::Result<()> {
    use hp_wmi::BiosSettings;

    let s = BiosSettings::connect()
        .map_err(|e| anyhow::anyhow!("{e} (elevated run required)"))?;

    match (name, set) {
        // `--set` without a name is a usage error, not a list request. Matching
        // this before the bare `(None, _)` arm is what makes the error reachable.
        (None, Some(_)) => anyhow::bail!("--set needs a setting name"),
        // List everything.
        (None, None) => {
            let all = s.list().map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("BIOS settings ({} entries)", all.len());
            println!("{}", "=".repeat(64));
            for x in &all {
                let ro = if x.read_only { "RO" } else { "  " };
                println!("[{ro}] {:<34} = {}", x.name, x.current);
            }
            println!("\nWritable settings can be changed with: omenmon bios <name> --set <value>");
        }
        // Read one.
        (Some(n), None) => {
            let all = s.list().map_err(|e| anyhow::anyhow!("{e}"))?;
            match all.iter().find(|x| x.name.eq_ignore_ascii_case(n)) {
                Some(x) => {
                    println!("{} = {}", x.name, x.current);
                    if !x.possible.is_empty() {
                        println!("possible: {}", x.possible.join(" | "));
                    }
                    if x.read_only {
                        println!("(read-only)");
                    }
                }
                None => anyhow::bail!(
                    "no setting named '{n}' is enumerated by this firmware"
                ),
            }
        }
        // Write one, then read back.
        (Some(n), Some(v)) => {
            let before = s
                .list()
                .ok()
                .and_then(|all| all.into_iter().find(|x| x.name.eq_ignore_ascii_case(n)))
                .map(|x| x.current)
                .unwrap_or_default();
            let r = s.set(n, v).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{n} <- '{v}'");
            println!("firmware returned: {r}");
            match r {
                hp_wmi::SetResult::Accepted | hp_wmi::SetResult::AcceptedNeedsReboot => {
                    let after = s
                        .list()
                        .ok()
                        .and_then(|all| all.into_iter().find(|x| x.name.eq_ignore_ascii_case(n)))
                        .map(|x| x.current)
                        .unwrap_or_default();
                    println!("readback: '{}' -> '{}'", before, after);
                    if after != v {
                        println!("note: readback does not equal the requested value");
                    }
                }
                hp_wmi::SetResult::ValueRejected => {
                    println!(
                        "This setting exists but the firmware refuses this value.\n\
                         On board 8C9C this is what 'Battery Care Function' does for every\n\
                         value: the name is present in the image, the platform flag is off."
                    );
                }
                hp_wmi::SetResult::UnknownSetting => {
                    println!("No setting with that name exists in this firmware.");
                }
                hp_wmi::SetResult::Other(c) => {
                    println!("Unexpected return code {c}; nothing was assumed.");
                }
            }
        }
    }
    Ok(())
}

/// Fan speed control.
///
/// `max` is verified working on this board (0x27). `auto` releases max mode, which
/// needs a 0x2E write as well — 0x27 off alone leaves the fans pinned. A number
/// sets a manual duty through 0x2E.
fn fan(setting: Option<&str>) -> anyhow::Result<()> {
    let b = bios()?;
    let (before, _) = b.fan_levels()?;
    match setting {
        None => {
            let (a, g) = b.fan_levels()?;
            println!("fan levels: cpu={a} gpu={g} (live 0x2D read)");
            println!("  max-fan flag: {:02X?}", b.get_max_fan()?.data.get(..1));
            println!("\nusage: omenmon fan max | auto | <0-100>");
        }
        Some("max") => {
            b.set_max_fan(true)?;
            // Spool time: reading immediately reports the old level.
            let mut peak = before;
            for _ in 0..8 {
                std::thread::sleep(std::time::Duration::from_millis(900));
                peak = peak.max(b.fan_levels()?.0);
            }
            println!("max fan: level {before} -> {peak}");
            if peak > before.saturating_add(5) {
                println!("verified.");
            } else {
                println!("WARNING: level did not rise; firmware may have ignored 0x27.");
            }
            println!("run `omenmon fan auto` to hand the fans back to the firmware");
        }
        Some("auto") | Some("release") => {
            let after = b.release_max_fan()?;
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let settled = b.fan_levels()?.0;
            println!("released max mode: level {before} -> {after} -> {settled}");
            println!(
                "handed back with 0x2E scheme 0 (EWMA_ALGO, HP's FanSetScheme); \
                 the firmware owns the fans again"
            );
        }
        Some(other) => {
            let pct: u8 = other
                .parse()
                .map_err(|_| anyhow::anyhow!("expected max | auto | 0-100, got '{other}'"))?;
            if pct > 100 {
                anyhow::bail!("duty must be 0-100, got {pct}");
            }
            b.set_fan_speed_pct(pct)?;
            std::thread::sleep(std::time::Duration::from_millis(2500));
            let after = b.fan_levels()?.0;
            println!("manual {pct}%: level {before} -> {after}");
            println!("run `omenmon fan auto` to hand the fans back to the firmware");
        }
    }
    Ok(())
}

fn thermal(mode: Option<&str>) -> anyhow::Result<()> {
    use hp_wmi::ThermalMode;
    let b = bios()?;
    match mode {
        None => {
            let s = b.get_thermal_status()?;
            println!("thermal status 0x28[8] = {s:#04X}");
            match ThermalMode::from_status(s) {
                Some(m) => println!("  -> {m}"),
                None => println!("  -> unrecognised"),
            }
            println!("\navailable: balanced (0x30), performance (0x31), cool (0x50)");
        }
        Some(name) => {
            let m = match name.to_ascii_lowercase().as_str() {
                "balanced" | "default" | "l2" => ThermalMode::Balanced,
                "performance" | "perf" | "l7" => ThermalMode::Performance,
                "cool" | "l4" => ThermalMode::Cool,
                other => anyhow::bail!(
                    "unknown mode '{other}' - use balanced | performance | cool"
                ),
            };
            let observed = b.set_fan_mode_verified(m)?;
            let expected = m.expected_status();
            println!("requested {name} (code {:#04X})", m.code());
            println!("firmware status 0x28[8] = {observed:#04X} (expected {expected:#04X})");
            if observed == expected {
                println!("verified.");
                let mut cfg = hp_core::Config::load();
                cfg.set_thermal_mode(Some(
                    match m {
                        ThermalMode::Performance => "performance",
                        ThermalMode::Cool => "cool",
                        ThermalMode::Balanced => "balanced",
                    },
                ));
                match cfg.save() {
                    Ok(_) => println!("saved: will re-apply on `omenmon apply`"),
                    Err(e) => println!("warning: could not save preference: {e}"),
                }
            } else {
                anyhow::bail!("firmware did not settle on the expected mode");
            }
        }
    }
    Ok(())
}

fn kbd(zone: Option<u8>, color: Option<&str>) -> anyhow::Result<()> {
    let b = bios()?;
    match (zone, color) {
        (None, _) => {
            let r = b.get_color_table()?;
            let d = &r.data;
            println!("color table ({} bytes), zone colors at 0x1A + 3*i:", d.len());
            for i in 0..4usize {
                let o = 0x1A + i * 3;
                if o + 2 < d.len() {
                    println!("  zone {i}: {:02X}{:02X}{:02X}", d[o], d[o + 1], d[o + 2]);
                }
            }
        }
        (Some(z), Some(hex)) => {
            if z > 3 {
                anyhow::bail!("zone must be 0..3");
            }
            let c = u32::from_str_radix(hex.trim_start_matches("0x"), 16)
                .map_err(|_| anyhow::anyhow!("bad hex color '{hex}'"))?;
            // safe-write: read -> modify -> write -> verify -> restore on failure
            let orig = b.get_color_table()?.data;
            if orig.len() < 0x2A {
                anyhow::bail!("color table too short");
            }
            let mut tbl = orig.clone();
            let o = 0x1A + z as usize * 3;
            tbl[o] = ((c >> 16) & 0xFF) as u8;
            tbl[o + 1] = ((c >> 8) & 0xFF) as u8;
            tbl[o + 2] = (c & 0xFF) as u8;
            b.set_color_table(&tbl)?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            let now = b.get_color_table()?.data;
            if now[o..o + 3] == tbl[o..o + 3] {
                println!("zone {z} set to {hex} — verified by readback");
            } else {
                println!("readback mismatch; attempting restore");
                b.set_color_table(&orig)?;
                anyhow::bail!("write not verified — original restored");
            }
        }
        (Some(_), None) => anyhow::bail!("--color <hex> required with a zone"),
    }
    Ok(())
}

fn bios() -> anyhow::Result<Bios> {
    let b = Bios::connect().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(b)
}

fn probe() -> anyhow::Result<()> {
    println!("omenmon probe — HP WMI BIOS capability matrix");
    println!("{}", "=".repeat(55));
    let b = bios()?;

    println!("hpqBIntM instance:        OK (connected)");

    match b.get_system_data() {
        Ok(s) => {
            println!("GetSystem (0x28):         OK");
            println!("  StatusFlags:            0x{:04X}  (PPAB bits: 3/4/8 = {}/{}/{})",
                s.status_flags,
                s.status_flags & 0x0008 != 0,
                s.status_flags & 0x0010 != 0,
                s.status_flags & 0x0100 != 0);
            println!("  ThermalPolicy:           {} ({})", s.thermal_policy,
                if s.thermal_policy_v1() { "V1 - mode remap applies" } else { "legacy V0" });
            println!("  SupportFlags:            0x{:02X}  SwFanCtl={} ExtremeMode={} TwoBytePL4={}",
                s.support_flags, s.sw_fan_control(), s.extreme_mode(), s.two_byte_pl4());
            println!("  DefaultPL4:              {} W", s.default_pl4);
            println!("  BiosOc:                  {} ({})", s.bios_oc,
                if s.bios_oc_supported() { "supported" } else { "NOT supported" });
            println!("  GpuModeSwitch:           0x{:02X}", s.gpu_mode_switch);
            println!("  DefaultPLwGpu:           {} W", s.default_pl_with_gpu);
        }
        Err(e) => println!("GetSystem (0x28):         FAIL - {e}"),
    }
    match b.get_fan_count() {
        Ok(n) => println!("GetFanCount (0x10):       OK — {n} fan(s)"),
        Err(e) => println!("GetFanCount (0x10):       FAIL — {e}"),
    }
    match b.get_fan_type() {
        Ok(r) => {
            let t = r.byte0().unwrap_or(0);
            println!("GetFanType (0x2C):        OK — byte0=0x{t:02X} (nibbles: 1=CPU 2=GPU 3=exhaust 4=pump 5=intake)");
        }
        Err(e) => println!("GetFanType (0x2C):        FAIL — {e}"),
    }
    match b.get_fan_level() {
        Ok(r) => {
            let hex: Vec<String> = r.data.iter().take(8).map(|b| format!("{b:02X}")).collect();
            println!("GetFanLevel (0x2D):       OK — [{}]", hex.join(" "));
        }
        Err(e) => println!("GetFanLevel (0x2D):       FAIL — {e}"),
    }
    match b.get_max_fan() {
        Ok(r) => println!("GetMaxFan (0x26):         OK — {}", r.byte0().unwrap_or(0)),
        Err(e) => println!("GetMaxFan (0x26):         FAIL — {e}"),
    }
    match b.get_gpu_power() {
        Ok(r) => println!("GetGpuPower (0x21):       OK — bytes {:02X?}", &r.data[..r.data.len().min(4)]),
        Err(e) => println!("GetGpuPower (0x21):       FAIL — {e}"),
    }
    match b.get_gpu_mode() {
        Ok(r) => println!("GetGpuMode (0x52):        OK — {}", r.byte0().unwrap_or(0)),
        Err(e) => println!("GetGpuMode (0x52):        FAIL — {e}"),
    }
    match b.get_fan_table() {
        Ok(r) => println!("GetFanTable (0x2F):       OK — {} bytes", r.data.len()),
        Err(e) => println!("GetFanTable (0x2F):       FAIL — {e}"),
    }
    match b.has_backlight() {
        Ok(v) => println!("HasBacklight (kbd 1):     OK — {v}"),
        Err(e) => println!("HasBacklight (kbd 1):     FAIL — {e}"),
    }
    match b.get_kbd_type() {
        Ok(v) => println!("GetKbdType (0x2B):        OK — {v}"),
        Err(e) => println!("GetKbdType (0x2B):        FAIL — {e}"),
    }
    match b.get_color_table() {
        Ok(r) => {
            let hex: Vec<String> = r.data.iter().take(16).map(|b| format!("{b:02X}")).collect();
            println!("GetColorTable (kbd 2):    OK — [{}...]", hex.join(" "));
        }
        Err(e) => println!("GetColorTable (kbd 2):    FAIL — {e}"),
    }
    match b.get_backlight() {
        Ok(v) => println!("GetBacklight (kbd 4):     OK — 0x{v:02X}"),
        Err(e) => println!("GetBacklight (kbd 4):     FAIL — {e}"),
    }

    // Temperature — brute force sensor selectors
    println!("\nGetTemperature (0x23) sensor probe:");
    for sensor in [0u8, 1, 2, 3, 4, 0xFF] {
        match b.get_temperature(sensor) {
            Ok(r) => {
                if r.is_ok() {
                    println!("  sensor {sensor:3}: OK — {} (raw {:02X?})", r.byte0().unwrap_or(0), &r.data[..r.data.len().min(4)]);
                } else {
                    println!("  sensor {sensor:3}: rwret={}", r.return_code);
                }
            }
            Err(e) => println!("  sensor {sensor:3}: FAIL — {e}"),
        }
    }
    Ok(())
}

fn temps() -> anyhow::Result<()> {
    let b = bios()?;
    for sensor in [0u8, 1, 2, 3] {
        if let Ok(r) = b.get_temperature(sensor) {
            if r.is_ok() {
                println!("sensor {sensor}: {}C", r.byte0().unwrap_or(0));
            }
        }
    }
    Ok(())
}

fn fans() -> anyhow::Result<()> {
    let b = bios()?;
    let count = b.get_fan_count().unwrap_or(0);
    println!("fans: {count}");
    if let Ok(r) = b.get_fan_level() {
        for (i, lvl) in r.data.iter().take(count as usize).enumerate() {
            println!("fan {i}: level {lvl}");
        }
    }
    Ok(())
}
