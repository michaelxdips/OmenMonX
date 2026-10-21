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

fn install() -> anyhow::Result<()> {
    println!("OmenMon-rs installer");
    println!("{}", "=".repeat(50));

    // elevated?
    if !is_elevated() {
        bail!("Administrator rights required. Right-click -> Run as administrator.");
    }

    // 1. copy binaries
    std::fs::create_dir_all(INSTALL_DIR)?;
    for name in ["omenmon.exe"] {
        let src = exe_dir().join(name);
        if src.exists() {
            std::fs::copy(&src, PathBuf::from(INSTALL_DIR).join(name))?;
            println!("installed  {INSTALL_DIR}\\{name}");
        } else {
            println!("skipped    {name} (not found next to installer)");
        }
    }

    // 2. PATH for current user (append if missing)
    add_to_user_path(INSTALL_DIR)?;

    // 3. scheduled task via PowerShell ScheduledTasks module (robust quoting)
    let ps = format!(
        "$a = New-ScheduledTaskAction -Execute 'C:\\Program Files\\OmenMon\\omenmon.exe' -Argument 'status'; \
         $t = New-ScheduledTaskTrigger -AtLogOn; \
         $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries; \
         Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $a -Trigger $t -Settings $s -Force | Out-Null"
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("task registration failed: {err}");
    }
    println!("autostart  task '{TASK_NAME}' registered (logon telemetry snapshot)");

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
    run(&format!("reg delete HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\OmenMon-rs /f"))?;
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
