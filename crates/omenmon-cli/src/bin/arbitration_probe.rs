//! Arbitration measurement: does concurrent RGB writes from two processes
//! corrupt the color table?
//!
//! This is a gated probe, not a shipped feature. Build with:
//!   cargo build --release --features probes
//!
//! Protocol: process A (this binary, writer role "A") and process B (same
//! binary, role "B") both run N rounds of: read color table, flip one zone
//! to a role-distinct color, write, read back. Interleaved transactions
//! show up as a readback that matches NEITHER role's color — the signature
//! of a torn read-modify-write caused by the other process's write landing
//! between our read and our write.
//!
//! The EC mutex (Global\Access_EC) serializes cooperating peers, so both
//! processes contend on it. A clean run (0 anomalies) means the mutex
//! discipline holds for the RGB path; anomalies quantify the exposure.

use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let role = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("A")
        .to_uppercase();
    let rounds: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);

    let bios = hp_wmi::Bios::connect().map_err(|e| anyhow::anyhow!("WMI connect: {e}"))?;

    // Role-distinct colors, maximally different bit patterns.
    let (my_color, other_color) = if role == "A" {
        (0xFF0000u32, 0x0000FFu32)
    } else {
        (0x0000FFu32, 0xFF0000u32)
    };
    let zone = 0usize;

    let mut anomalies = 0u32;
    let mut write_failures = 0u32;
    let start = Instant::now();

    for round in 0..rounds {
        // read-modify-write with immediate readback verification.
        let orig = bios.get_color_table().map_err(|e| anyhow::anyhow!("read: {e}"))?;
        let mut tbl = orig.data.clone();
        if tbl.len() < 0x1A + 3 {
            anyhow::bail!("table too short");
        }
        let o = 0x1A + zone * 3;
        tbl[o] = ((my_color >> 16) & 0xFF) as u8;
        tbl[o + 1] = ((my_color >> 8) & 0xFF) as u8;
        tbl[o + 2] = (my_color & 0xFF) as u8;
        if let Err(e) = bios.set_color_table(&tbl) {
            write_failures += 1;
            eprintln!("round {round}: write failed: {e}");
            continue;
        }
        std::thread::sleep(Duration::from_millis(2));
        let now = bios.get_color_table().map_err(|e| anyhow::anyhow!("verify: {e}"))?;
        let d = &now.data;
        let got = ((d[o] as u32) << 16) | ((d[o + 1] as u32) << 8) | d[o + 2] as u32;
        if got != my_color {
            anomalies += 1;
            println!(
                "round {round}: ANOMALY zone {zone} = {got:06X} (wanted {my_color:06X}, other {other_color:06X})"
            );
        }
        // Small delay so the roles interleave rather than strictly alternate.
        std::thread::sleep(Duration::from_millis(1 + (round % 3) as u64));
    }

    // Restore a neutral color so the keyboard is not left red/blue.
    let orig = bios.get_color_table()?.data;
    let mut tbl = orig.clone();
    let o = 0x1A + zone * 3;
    tbl[o] = 0x20;
    tbl[o + 1] = 0x40;
    tbl[o + 2] = 0x80;
    let _ = bios.set_color_table(&tbl);

    let elapsed = start.elapsed().as_secs_f32();
    println!(
        "role {role}: {rounds} rounds in {elapsed:.1}s — anomalies {anomalies}, write failures {write_failures}"
    );
    if anomalies > 0 {
        println!("EXPOSURE CONFIRMED: concurrent writers torn {} times", anomalies);
        std::process::exit(2);
    }
    println!("CLEAN: mutex discipline held for this path and duration");
    Ok(())
}
