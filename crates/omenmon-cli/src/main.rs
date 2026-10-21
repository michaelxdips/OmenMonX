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
    /// Power telemetry: live CPU package watts + static limit bytes
    Power,
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
    println!("omenmon power — live telemetry (verified 2026-10-21)");
    println!("{}", "=".repeat(50));
    // 0x2A byte0 = live CPU package watts (moves under load, 5 Hz verified);
    // bytes 3/4 = static limit candidates; EC C6/C7/C8 = static limits.
    match b.raw_send(Request::with_data(CMD_DEFAULT, 0x2A, &[]), 128) {
        Ok(r) if r.is_ok() => {
            let watts = r.data.first().copied().unwrap_or(0);
            println!("CPU package:    {watts:>3} W  (live)");
            if r.data.len() > 4 {
                println!("limit bytes:    0x{:02X} 0x{:02X} (static, unconfirmed meaning)", r.data[3], r.data[4]);
            }
        }
        Ok(r) => anyhow::bail!("BIOS returned rwret={}", r.return_code),
        Err(e) => anyhow::bail!("{e} (elevated run required)"),
    }
    if let Ok(io) = hp_ec::ecio::EcIo::connect() {
        if let Ok(page) = io.with_lock(2000, |io| hp_ec::eram::Eram::new(io).read_range(0xC6, 3)) {
            println!("EC limits:      0xC6={} 0xC7={} 0xC8={} (static)", page[0], page[1], page[2]);
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
