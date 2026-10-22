//! daemon module (included by main.rs as `mod daemon`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn log_dir() -> std::path::PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("OmenMon")
}

fn ts() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("[{n}]")
}

/// Simple single-instance guard using the `single_instance_guard`-style
/// approach without extra deps: lock file with PID + stale detection.
struct InstanceGuard {
    path: std::path::PathBuf,
}

impl InstanceGuard {
    fn acquire() -> anyhow::Result<Self> {
        let path = log_dir().join("daemon.lock");
        if let Ok(txt) = std::fs::read_to_string(&path) {
            if let Ok(pid) = txt.trim().parse::<u32>() {
                // stale check: is that pid alive?
                let alive = std::process::Command::new("tasklist")
                    .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
                    .unwrap_or(false);
                if alive {
                    anyhow::bail!("OmenMon daemon already running (pid {pid})");
                }
            }
        }
        std::fs::create_dir_all(log_dir())?;
        std::fs::write(&path, std::process::id().to_string())?;
        Ok(Self { path })
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

pub fn run() -> anyhow::Result<()> {
    let _guard = InstanceGuard::acquire()?;
    let dir = log_dir();
    std::fs::create_dir_all(&dir)?;
    let log_path = dir.join("daemon.log");
    let log = |msg: &str| {
        let line = format!("{} {msg}\n", ts());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    };

    log(&format!("daemon start v{}", env!("CARGO_PKG_VERSION")));

    let cfg = hp_core::Config::load();
    let engine = Arc::new(hp_core::engine::Engine::new(cfg.clone()));
    let bios = match hp_wmi::Bios::connect() {
        Ok(b) => b,
        Err(e) => {
            log(&format!("FATAL: WMI BIOS connect failed: {e}"));
            // Same translation as the CLI's `bios()`: a permission problem must
            // not read as a broken driver, because the fix is different.
            let text = e.to_string();
            let denied = text.contains("0x80041003")
                || text.contains("Access denied")
                || text.contains("Impersonation");
            if denied {
                anyhow::bail!(
                    "the daemon needs administrator rights to reach the BIOS interface; \
                     start it from an elevated terminal"
                );
            }
            anyhow::bail!("WMI BIOS unavailable: {e}");
        }
    };
    let io = hp_ec::ecio::EcIo::connect().ok();
    log(&format!(
        "sources: wmi=OK ec={}",
        if io.is_some() { "OK" } else { "unavailable" }
    ));

    {
        let stop = &STOP;
        ctrlc(move || stop.store(true, Ordering::SeqCst))
            .map_err(|e| anyhow::anyhow!("ctrl-c handler: {e}"))?;
    }

    let poll_ms = cfg.profile.poll_ms.max(250) as u64;
    let mut last_battery = Instant::now();

    log("loop start");
    while !STOP.load(Ordering::Relaxed) {
        let st = engine.poll(&bios, io.as_ref());

        if last_battery.elapsed() >= Duration::from_secs(30) {
            last_battery = Instant::now();
            let b = hp_core::battery::read_battery();
            if cfg.enable_charge_alerts && b.ac_online {
                if let Some(c) = b.charge_pct {
                    if c >= cfg.charge_alert_pct {
                        log(&format!("ALERT: charge {c}% on AC (wear guard)"));
                    }
                }
            }
            if let Some(h) = b.health_pct {
                if h < cfg.battery_health_warn_pct {
                    log(&format!("ALERT: battery health {h}% below threshold"));
                }
            }
        }

        // fan EC write hook: activates when signed PawnIO ECMMIO module ships.
        if st.fan_write_active {
            // TODO(ecmp): safe-write level once hp_ec::ecmp lands.
        }

        std::thread::sleep(Duration::from_millis(poll_ms));
    }

    log("daemon stop");
    Ok(())
}

fn ctrlc(f: impl Fn() + Send + 'static) -> Result<(), Box<dyn std::error::Error>> {
    // raw kernel32 declaration (windows crate lacks the Console feature here)
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    static HANDLER_FIRED: AtomicBool = AtomicBool::new(false);
    let fired_flag = &HANDLER_FIRED;
    std::thread::spawn(move || {
        loop {
            if fired_flag.load(Ordering::SeqCst) {
                f();
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    unsafe extern "system" fn handler(t: u32) -> i32 {
        if t == 0 {
            // CTRL_C_EVENT
            HANDLER_FIRED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }
    unsafe {
        if SetConsoleCtrlHandler(Some(handler), 1) == 0 {
            return Err("SetConsoleCtrlHandler failed".into());
        }
    }
    Ok(())
}
