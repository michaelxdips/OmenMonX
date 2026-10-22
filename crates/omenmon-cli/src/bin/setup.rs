//! omenmon-setup: self-installer for OmenMon-rs.
//! Copies binaries to Program Files\OmenMon, registers the telemetry
//! autostart scheduled task, adds an Add/Remove Programs entry, and writes an
//! uninstaller. Run elevated (or via the OmenMonRunner task).

use anyhow::bail;
use std::path::PathBuf;
use std::process::Command;

const INSTALL_DIR: &str = r"C:\Program Files\OmenMon";
const TASK_NAME: &str = "OmenMonTelemetry";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.len() > 1 {
        anyhow::bail!(
            "unexpected extra arguments ({:?}); usage: omenmon-setup [install|uninstall]",
            &args[1..]
        );
    }
    let action = args.first().map(String::as_str).unwrap_or("install");
    match action {
        "install" => install()?,
        "uninstall" => uninstall()?,
        // `--help` and `-h` are what a user types first. Answering them with an
        // error and exit code 1 reads as a broken program, so print the usage and
        // exit 0.
        "help" | "--help" | "-h" => {
            println!("omenmon-setup {}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Installs OmenMon to C:\\Program Files\\OmenMon:");
            println!("  - copies omenmon.exe, omenmon-gui.exe, omenmon-setup.exe");
            println!("  - adds the install dir to the user PATH");
            println!("  - registers an elevated logon task that runs `omenmon apply`");
            println!("  - adds an Add/Remove Programs entry");
            println!();
            println!("Usage: omenmon-setup [install|uninstall]");
            println!();
            println!("  install    (default) install or repair");
            println!("  uninstall  remove files, PATH entry and logon task");
            println!();
            println!("Both actions need administrator rights.");
            return Ok(());
        }
        other => anyhow::bail!("usage: omenmon-setup [install|uninstall] (got: {other})"),
    }
    Ok(())
}

fn exe_dir() -> anyhow::Result<PathBuf> {
    // No unwraps: running from a context without an exe path (some service
    // hosts) must produce an error, not a panic with no output.
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot determine installer location: {e}"))?;
    exe.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("installer path has no parent directory"))
}

/// The PowerShell that registers the logon task.
///
/// Split out so the two things that make it correct are testable without
/// registering anything on the machine:
///
/// * the action runs **`apply`**, not `status`. The firmware resets power limits
///   and the thermal profile on every boot, so a saved cap only survives a reboot
///   if something re-applies it. `status` only prints, which meant a cap chosen in
///   the GUI was gone after a restart with nothing to restore it.
/// * the principal runs at **Highest**. `0x29` and `0x2A` are admin-only, so an
///   unelevated logon task would call `apply`, get access-denied, and leave the
///   machine at the firmware default while looking like it had restored the cap.
fn logon_task_script() -> String {
    format!(
        "$a = New-ScheduledTaskAction -Execute 'C:\\Program Files\\OmenMon\\omenmon.exe' -Argument 'apply'; \
         $t = New-ScheduledTaskTrigger -AtLogOn; \
         $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries \
              -ExecutionTimeLimit (New-TimeSpan -Minutes 5); \
         $p = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest; \
         Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $a -Trigger $t -Settings $s \
              -Principal $p -Force | Out-Null"
    )
}

fn install() -> anyhow::Result<()> {
    println!("OmenMon-rs installer");
    println!("{}", "=".repeat(50));

    // elevated?
    if !is_elevated() {
        bail!("Administrator rights required. Right-click -> Run as administrator.");
    }

    // 1. copy binaries
    let exe_dir = exe_dir()?;
    std::fs::create_dir_all(INSTALL_DIR)?;
    // A missing omenmon.exe must stop the install: the logon task runs it, so a
    // silent skip produced a broken scheduled task that failed every logon.
    let main_exe = exe_dir.join("omenmon.exe");
    if !main_exe.exists() {
        bail!(
            "omenmon.exe not found next to the installer ({})",
            exe_dir.display()
        );
    }
    for name in ["omenmon.exe", "omenmon-gui.exe", "omenmon-setup.exe"] {
        let src = exe_dir.join(name);
        if src.exists() {
            std::fs::copy(&src, PathBuf::from(INSTALL_DIR).join(name))?;
            println!("installed  {INSTALL_DIR}\\{name}");
        } else {
            println!("skipped    {name} (not found next to installer)");
        }
    }

    // 1b. GUI autostart shortcut for the current user (Startup folder)
    let startup = std::env::var("APPDATA")
        .map(|a| PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"))
        .ok();
    if let Some(dir) = startup {
        let gui = PathBuf::from(INSTALL_DIR).join("omenmon-gui.exe");
        if gui.exists() {
            std::fs::create_dir_all(&dir)?;
            let lnk = dir.join("OmenMon.lnk");
            let ps = format!(
                "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
                 $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.Save()",
                lnk.display(),
                gui.display(),
                INSTALL_DIR
            );
            // Full path: a stripped-down environment (logon context) does not
            // always resolve `powershell` on PATH.
            let out = Command::new(powershell())
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
                .output()?;
            if out.status.success() {
                println!("autostart  GUI shortcut -> {}", lnk.display());
            } else {
                // Non-fatal: the GUI autostart is a convenience, not the cap
                // re-apply path (that is the scheduled task below).
                println!("autostart  GUI shortcut FAILED (non-fatal)");
            }
        }
    }

    // 2. PATH for current user (append if missing)
    add_to_user_path(INSTALL_DIR)?;

    // 3. scheduled task via PowerShell ScheduledTasks module (robust quoting)
    let ps = logon_task_script();
    let out = Command::new(powershell())
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("task registration failed: {err}");
    }
    println!("autostart  task '{TASK_NAME}' registered (elevated, re-applies the saved cap at logon)");

    // 4. Add/Remove Programs entry (PowerShell for robust quoting)
    let uninstaller = PathBuf::from(INSTALL_DIR).join("omenmon-setup.exe");
    let ps = format!(
        "$k = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\OmenMon-rs'; \
         New-Item -Path $k -Force | Out-Null; \
         Set-ItemProperty -Path $k -Name DisplayName -Value 'OmenMon-rs'; \
         Set-ItemProperty -Path $k -Name DisplayVersion -Value '{}'; \
         Set-ItemProperty -Path $k -Name Publisher -Value 'OmenMon-rs project (GPL-3.0-or-later)'; \
         Set-ItemProperty -Path $k -Name InstallLocation -Value '{INSTALL_DIR}'; \
         Set-ItemProperty -Path $k -Name UninstallString -Value '\"{}\" uninstall'; \
         Set-ItemProperty -Path $k -Name NoModify -Value 1 -Type DWord"
    , env!("CARGO_PKG_VERSION"), uninstaller.display());
    let out = Command::new(powershell())
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("ARP registration failed: {err}");
    }
    println!("registered in Add/Remove Programs");

    println!("\nDone. Try:  omenmon status");
    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    println!("OmenMon-rs uninstaller");
    println!("{}", "=".repeat(50));

    // Check elevation up front. Without this, the first `schtasks /Delete` failed
    // with "cmd failed: schtasks ... ERROR: Access is denied" and the user had to
    // work out for themselves that the real problem was a missing admin token.
    if !is_elevated() {
        anyhow::bail!(
            "administrator rights required.\n\
             Right-click omenmon-setup.exe -> \"Run as administrator\", or run\n\
             `omenmon-setup uninstall` from an elevated terminal."
        );
    }

    // Idempotent: a partial install must still clean up, so "task not found"
    // and "key not found" are successes. The old strict check made every
    // cleanup after a failed install fail with an error the user could do
    // nothing about.
    run_idempotent(&format!("schtasks /Delete /F /TN {TASK_NAME}"));
    let _ = Command::new("schtasks").args(["/Delete", "/F", "/TN", "OmenMonRunner"]).output();
    run_idempotent("reg delete HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\OmenMon-rs /f");

    // Remove every binary the installer put there. The list previously omitted
    // omenmon-gui.exe, so uninstall left the largest file behind and the install
    // directory could never be removed - a reinstall then inherited a stale GUI.
    // status.log is not in the list: nothing in this project creates it.
    for name in ["omenmon.exe", "omenmon-gui.exe", "omenmon-setup.exe"] {
        let p = PathBuf::from(INSTALL_DIR).join(name);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                // A running instance is the usual cause; say so instead of
                // pretending the file is gone.
                println!("could not remove {name}: {e} (close OmenMon and retry)");
            }
        }
    }

    // The Startup shortcut the installer created.
    if let Some(dir) = std::env::var("APPDATA")
        .ok()
        .map(|a| PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"))
    {
        let lnk = dir.join("OmenMon.lnk");
        if lnk.exists() {
            let _ = std::fs::remove_file(&lnk);
            println!("removed startup shortcut");
        }
    }

    match std::fs::remove_dir(INSTALL_DIR) {
        Ok(()) => println!("removed {INSTALL_DIR}"),
        // Not empty means something is still running or a foreign file was put
        // there; leaving it is correct, but say why.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => println!("kept {INSTALL_DIR}: {e}"),
    }
    println!("done. (PATH entry left; harmless)");
    Ok(())
}

fn run(cmd: &str) -> anyhow::Result<()> {
    let out = Command::new("cmd").args(["/C", cmd]).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("cmd failed: {cmd}\n{err}");
    }
    Ok(())
}

/// Like [`run`], but treats "does not exist" as success (uninstall idempotency).
fn run_idempotent(cmd: &str) {
    match run(cmd) {
        Ok(()) => {}
        Err(e) => {
            let text = e.to_string();
            let not_found = text.contains("does not exist")
                || text.contains("cannot find")
                || text.contains("not found")
                || text.contains("unable to find");
            if not_found {
                println!("absent     (already removed: {cmd})");
            } else {
                println!("warning: {cmd} -> {text}");
            }
        }
    }
}

/// The elevation probe. The write probe creates Program Files\OmenMon, which
/// uninstall must not do — so uninstall skips it entirely and relies on the
/// real command failing with an access error if the token is missing.
fn is_elevated() -> bool {
    let probe = PathBuf::from(INSTALL_DIR).join(".probe");
    match std::fs::create_dir_all(INSTALL_DIR).and_then(|_| std::fs::write(&probe, b"x")) {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Full path to Windows PowerShell. A logon-context PATH does not always
/// resolve bare `powershell`.
fn powershell() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

fn add_to_user_path(dir: &str) -> anyhow::Result<()> {
    // Use PowerShell to read/append user PATH robustly (no setx quoting traps).
    // Handles an empty existing Path: appending ";dir" to "" would start the
    // variable with a leading ';' that some tools mishandle.
    let script = format!(
        "$p=[Environment]::GetEnvironmentVariable('Path','User'); \
         $new = if([string]::IsNullOrEmpty($p)){{'{dir}'}}else{{if($p -notlike '*{dir}*'){{\"$p;{dir}\"}}else{{$p}}}}; \
         if($new -ne $p){{[Environment]::SetEnvironmentVariable('Path', $new, 'User')}}"
    );
    let out = Command::new(powershell())
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("PATH update failed: {err}");
    }
    println!("PATH       {dir} appended (new terminals)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-1's regression test. The logon task must re-apply the saved state; a
    /// task that only prints leaves the firmware at its boot default and the
    /// user's cap silently gone.
    #[test]
    fn logon_task_applies_the_saved_state() {
        let s = logon_task_script();
        assert!(s.contains("-Argument 'apply'"), "must run `apply`, got: {s}");
        assert!(
            !s.contains("-Argument 'status'"),
            "`status` only prints; the firmware would stay at its boot default"
        );
    }

    /// `0x29`/`0x2A` are admin-only, so the task must run elevated or `apply`
    /// fails with access-denied while looking like it worked.
    #[test]
    fn logon_task_runs_elevated() {
        let s = logon_task_script();
        assert!(s.contains("RunLevel Highest"), "an unelevated apply cannot write 0x29");
        assert!(s.contains("-AtLogOn"), "it is a logon trigger");
        assert!(s.contains(TASK_NAME), "it registers the expected task name");
    }

    /// A hung `omenmon apply` must not leave a task running forever.
    #[test]
    fn logon_task_has_a_time_limit() {
        assert!(logon_task_script().contains("ExecutionTimeLimit"));
    }
}
