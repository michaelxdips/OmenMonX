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
    let action = std::env::args().nth(1).unwrap_or_else(|| "install".into());
    match action.as_str() {
        "install" => install()?,
        "uninstall" => uninstall()?,
        other => bail!("usage: omenmon-setup [install|uninstall] (got: {other})"),
    }
    Ok(())
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().to_path_buf()
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
    std::fs::create_dir_all(INSTALL_DIR)?;
    for name in ["omenmon.exe", "omenmon-gui.exe", "omenmon-setup.exe"] {
        let src = exe_dir().join(name);
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
            let out = Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
                .output()?;
            if out.status.success() {
                println!("autostart  GUI shortcut -> {}", lnk.display());
            } else {
                println!("autostart  GUI shortcut FAILED (non-fatal)");
            }
        }
    }

    // 2. PATH for current user (append if missing)
    add_to_user_path(INSTALL_DIR)?;

    // 3. scheduled task via PowerShell ScheduledTasks module (robust quoting)
    let ps = logon_task_script();
    let out = Command::new("powershell")
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
    let out = Command::new("powershell")
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
    run(&format!("schtasks /Delete /F /TN {TASK_NAME}"))?;
    let _ = Command::new("schtasks").args(["/Delete", "/F", "/TN", "OmenMonRunner"]).output();
    run("reg delete HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\OmenMon-rs /f")?;
    // remove binaries (best effort)
    for name in ["omenmon.exe", "omenmon-setup.exe", "status.log"] {
        let p = PathBuf::from(INSTALL_DIR).join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir(INSTALL_DIR);
    println!("removed. (PATH entry left; harmless)");
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

fn is_elevated() -> bool {
    // cheap elevation probe: write to Program Files
    let probe = PathBuf::from(INSTALL_DIR).join(".probe");
    match std::fs::create_dir_all(INSTALL_DIR).and_then(|_| std::fs::write(&probe, b"x")) {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn add_to_user_path(dir: &str) -> anyhow::Result<()> {
    // Use PowerShell to read/append user PATH robustly (no setx quoting traps)
    let script = format!(
        "$p=[Environment]::GetEnvironmentVariable('Path','User'); if($p -notlike '*{dir}*'){{[Environment]::SetEnvironmentVariable('Path', \"$p;{dir}\", 'User')}}"
    );
    let out = Command::new("powershell")
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
