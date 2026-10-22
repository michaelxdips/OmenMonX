//! daemon module (included by main.rs as `mod daemon`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Log/data directory for the daemon (lock file, log). PUB(crate)-visible so
/// the lib.rs unit tests can exercise the instance guard directly.
pub(crate) fn log_dir() -> std::path::PathBuf {
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

/// Simple single-instance guard: lock file with PID + stale detection.
/// PUB(crate)-visible so the lib.rs unit tests can exercise it directly.
pub(crate) struct InstanceGuard {
    path: std::path::PathBuf,
}

impl InstanceGuard {
    /// Parse `tasklist /FI "PID eq N"` output and match the PID as a whole
    /// token. `String::contains` matched 123 inside 1234 or 5123, so a stale
    /// lock for a dead PID could pass as alive and block startup.
    fn pid_alive(pid: u32) -> bool {
        let needle = format!(" {pid} "); // tasklist columns are space-separated
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .any(|tok| tok == pid.to_string())
                    || String::from_utf8_lossy(&o.stdout).contains(&needle)
            })
            .unwrap_or(false)
    }

    /// PUB(crate): the lib.rs unit tests exercise the guard directly.
    pub(crate) fn acquire() -> anyhow::Result<Self> {
        let path = log_dir().join("daemon.lock");
        // Atomic creation first: two daemons starting together must not both
        // win. Only when the lock already exists do we run the stale-pid check.
        std::fs::create_dir_all(log_dir())?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {
                std::fs::write(&path, std::process::id().to_string())?;
                return Ok(Self { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // fall through to the stale check
            }
            Err(e) => return Err(e.into()),
        }
        let mut stale = true;
        if let Ok(txt) = std::fs::read_to_string(&path) {
            if let Ok(pid) = txt.trim().parse::<u32>() {
                stale = !Self::pid_alive(pid);
                if !stale {
                    anyhow::bail!("OmenMon daemon already running (pid {pid})");
                }
            }
        }
        if stale {
            // Abandoned lock (crashed owner / WAIT_ABANDONED-style): delete and
            // recreate atomically. A concurrent starter may have recreated it
            // first; that race loses cleanly.
            let _ = std::fs::remove_file(&path);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => {
                    std::fs::write(&path, std::process::id().to_string())?;
                    Ok(Self { path })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    anyhow::bail!("OmenMon daemon already running (lock recreated)");
                }
                Err(e) => Err(e.into()),
            }
        } else {
            unreachable!("live pid bailed above")
        }
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
    let mut log_failure_reported = false;
    let mut log = move |msg: &str| {
        use std::io::Write;
        let line = format!("{} {msg}\n", ts());
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
        {
            Ok(()) => log_failure_reported = false,
            Err(e) => {
                // A permanently failing log used to be silent forever; report it
                // to stderr once so a broken install is diagnosable.
                if !log_failure_reported {
                    log_failure_reported = true;
                    eprintln!("omenmon daemon: log append failed: {e}");
                }
            }
        }
    };

    log(&format!("daemon start v{}", env!("CARGO_PKG_VERSION")));

    let cfg = hp_core::Config::load();
    let engine = hp_core::engine::Engine::new(cfg.clone());
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
    let mut last_temp_log = Instant::now();
    let temp_log_every = Duration::from_secs(3600);

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

        // Fan EC write hook is pending hp_ec::ecmp_mmio / Hpecmp signing; until
        // the signed PawnIO ECMMIO module ships there is nothing safe to write.
        // The poll above keeps temps flowing; one line an hour keeps the log
        // from being a dead file.
        if last_temp_log.elapsed() >= temp_log_every {
            last_temp_log = Instant::now();
            log(&format!(
                "temps: cpu {:?} gpu {:?} fan {}%",
                st.sample.cpu, st.sample.gpu,
                st.sample
                    .fan_level_pct
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into())
            ));
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
        // CTRL_C_EVENT=0, CTRL_CLOSE_EVENT=2, CTRL_SHUTDOWN_EVENT=5. Close and
        // shutdown must run the cleanup path too: a terminal close used to
        // bypass it and leave the lock file behind. Close/shutdown give the
        // process a limited grace window, so signal STOP and ask the system to
        // end (return 1 keeps our handler authoritative).
        if matches!(t, 0 | 2 | 5) {
            HANDLER_FIRED.store(true, Ordering::SeqCst);
            if t != 0 {
                STOP.store(true, Ordering::SeqCst);
                return 1;
            }
            return 1;
        }
        0
    }
    unsafe {
        if SetConsoleCtrlHandler(Some(handler), 1) == 0 {
            return Err("SetConsoleCtrlHandler failed".into());
        }
    }
    Ok(())
}
