//! Live 1 Hz watch: does plugging in the 230 W brick change the 25 W ceiling?
//!
//! Every earlier ceiling test ran with the factory 230 W brick DISCONNECTED, so
//! "25 W is invariant" may only describe USB-C PD operation. This tool prints one
//! line per second so the brick can be attached mid-run and the change observed
//! directly.
//!
//! Columns:
//!   t      seconds since start
//!   [0]    0x2A byte 0 - the only byte in the block that drifts
//!   PL1/2/4 the limits the firmware is actually holding
//!   [7]    with-GPU limit (thermal-profile derived)
//!   status 0x28[8] thermal status byte
//!   MHz    live clock, `\Processor Information(_Total)\Actual Frequency`
//!   W      CPU package power from the Windows energy counter, if available
//!   AC     power line state
//!
//! Read-only: it never writes the power block or the thermal mode. Ctrl+C to stop.

use anyhow::Result;
use hp_wmi::payload::{Request, CMD_DEFAULT};
use hp_wmi::Bios;
use std::io::Write;
use std::time::{Duration, Instant};

fn typeperf(counter: &str) -> Option<f64> {
    let out = std::process::Command::new("typeperf")
        .args([counter, "-sc", "1"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        if line.starts_with('"') && line.matches('"').count() >= 4 {
            for p in line.split('"').rev() {
                let t = p.trim_matches(',').trim();
                if let Ok(v) = t.parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// AC line state via GetSystemPowerStatus, without pulling in extra crates:
/// powershell is slow, so cache it and refresh only every 2 s.
fn ac_state() -> String {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_Battery).BatteryStatus",
        ])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let t = s.trim();
            match t {
                "2" => "AC".to_string(),
                "1" => "BATT".to_string(),
                "" => "?".to_string(),
                other => other.to_string(),
            }
        }
        Err(_) => "?".to_string(),
    }
}

fn main() -> Result<()> {
    let bios = Bios::connect()?;
    println!("omenmon watch - live power/thermal state at 1 Hz");
    println!("Read-only. Plug in the 230 W brick and watch PL1/PL2/PL4 and [0].");
    println!("Ctrl+C to stop.\n");

    println!(
        "{:>5} {:>4} {:>4} {:>4} {:>4} {:>4} {:>7} {:>6} {:>5}",
        "t(s)", "[0]", "PL1", "PL2", "PL4", "[7]", "status", "MHz", "AC"
    );
    println!("{}", "-".repeat(58));

    let t0 = Instant::now();
    let mut last_ac = String::new();
    let mut ac_tick = 0u32;
    let freq = "\\Processor Information(_Total)\\Actual Frequency";

    loop {
        let elapsed = t0.elapsed().as_secs();

        let block = bios.raw_send(Request::new(CMD_DEFAULT, 0x2A), 128);
        let (b0, pl1, pl2, pl4, b7) = match &block {
            Ok(r) if r.data.len() >= 8 => (r.data[0], r.data[2], r.data[3], r.data[4], r.data[7]),
            _ => (0, 0, 0, 0, 0),
        };
        let status = bios.get_thermal_status().unwrap_or(0);
        let mhz = typeperf(freq).unwrap_or(0.0);

        if ac_tick == 0 {
            last_ac = ac_state();
        }
        ac_tick = (ac_tick + 1) % 2;

        println!(
            "{elapsed:>5} {b0:>4} {pl1:>4} {pl2:>4} {pl4:>4} {b7:>4} {status:>7} {mhz:>6.0} {:>5}",
            last_ac
        );
        let _ = std::io::stdout().flush();

        std::thread::sleep(Duration::from_millis(1000));
    }
}
