use clap::{Parser, Subcommand};
use hp_wmi::Bios;
use hp_core as _;

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
    /// Keyboard color table: show, or set one zone's RGB
    Kbd {
        /// Show the color table, or a zone number 0..3 to set
        zone: Option<u8>,
        /// Hex color like FF8800 (only with --zone)
        #[arg(long)]
        color: Option<String>,
    },
    /// Long-running daemon (single-instance, battery alerts, curve loop)
    Daemon,
}

fn main() -> anyhow::Result<()> {
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
        Command::Kbd { zone, color } => kbd(zone, color.as_deref())?,
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
    println!("state:           {}", if info.charging { "charging" } else { "discharging/idle" });
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
        println!("temperature:     {t:.1} °C");
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
        "CPU: {:.0} °C   GPU: {:.0} °C   hotspot: {}",
        s.cpu.unwrap_or(0.0),
        s.gpu.unwrap_or(0.0),
        s.gpu_hotspot
            .map(|v| format!("{v:.0} °C"))
            .unwrap_or("n/a".into())
    );
    println!(
        "fan: {} RPM @ level {}",
        s.fan_rpm.map(|v| v.to_string()).unwrap_or("n/a".into()),
        s.fan_level_pct
            .map(|v| format!("{v} %"))
            .unwrap_or("n/a".into())
    );
    println!(
        "curve target: {} % (fan write {})",
        st.target_fan_pct,
        if st.fan_write_active { "ACTIVE" } else { "awaiting firmware path" }
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
    use hp_wmi::payload::{Request, CMD_DEFAULT};
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
        "Writable range: {}..{} W  (firmware clamps above {})",
        hp_wmi::CpuPowerLimits::MIN_WATTS,
        hp_wmi::CpuPowerLimits::MAX_WATTS,
        hp_wmi::CpuPowerLimits::MAX_WATTS
    );

    if let Ok(io) = hp_ec::ecio::EcIo::connect() {
        if let Ok(page) = io.with_lock(2000, |io| hp_ec::eram::Eram::new(io).read_range(0xC6, 3)) {
            println!("\nEC 0xC6..C8:    {} {} {}  (static)", page[0], page[1], page[2]);
        }
    }
    println!("\nNote: 0x2A is a settings block. It is NOT instantaneous package power.");
    Ok(())
}

/// Set the CPU power cap. Clamped to the verified 5..25 W window.
fn power_cap(watts: u8) -> anyhow::Result<()> {
    let b = bios()?;
    let applied = b.set_power_limit(watts)?;
    if applied != watts {
        println!(
            "requested {watts} W -> clamped to {applied} W (board accepts {}..{} W)",
            hp_wmi::CpuPowerLimits::MIN_WATTS,
            hp_wmi::CpuPowerLimits::MAX_WATTS
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
    Ok(())
}

/// Show the firmware fan curve.
fn fan_curve() -> anyhow::Result<()> {
    let b = bios()?;
    let t = b.get_fan_table_parsed()?;
    println!(
        "fan curve: {} fans, {} points (read-only - 0x2E and 0x32 are no-ops here)",
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
            } else {
                anyhow::bail!("firmware did not settle on the expected mode");
            }
        }
    }
    Ok(())
}

fn kbd(zone: Option<u8>, color: Option<&str>) -> anyhow::Result<()> {
    use hp_wmi::payload::{Request, CMD_KEYBOARD};
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

    print!("hpqBIntM instance:        OK (connected)\n");

    match b.get_system() {
        Ok(r) => {
            println!("GetSystem (0x28):         OK");
            if r.data.len() > 1 {
                println!("  SysSupportFlags:        0x{:02X}", r.data[1]);
            }
            if r.data.len() > 2 {
                println!("  SysSupportFlags2:       0x{:02X}", r.data[2]);
            }
        }
        Err(e) => println!("GetSystem (0x28):         FAIL — {e}"),
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
                println!("sensor {sensor}: {} °C", r.byte0().unwrap_or(0));
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
