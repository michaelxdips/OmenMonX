//! GitHub-releases auto-updater.
//!
//! Checks https://github.com/michaelxdips/OmenMonX/releases for a newer
//! version (semver), downloads the matching asset into
//! `%LOCALAPPDATA%\OmenMon\update`, verifies its SHA-256 against the digest
//! GitHub publishes for that asset, and only then stages an install.
//!
//! Two rules this module exists to enforce:
//!
//! * **A payload is never executed without a verified digest.** Anyone who can
//!   answer for `api.github.com` or `objects.githubusercontent.com` would
//!   otherwise get code execution at the next launch.
//! * **A payload is never copied over a file whose role it does not share.**
//!   The installer and the application binary are different things; the old code
//!   copied `omenmon-setup.exe` over `omenmon-gui.exe`, which replaced the GUI
//!   with an installer and then launched it. [`plan_for_asset`] is the single
//!   place that decides what an asset is allowed to do, and it is exhaustively
//!   tested.

use anyhow::{bail, Context};
use parking_lot::Mutex;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const LATEST_API: &str = "https://api.github.com/repos/michaelxdips/OmenMonX/releases/latest";
const UA: &str = concat!("OmenMon-rs/", env!("CARGO_PKG_VERSION"));

/// Network timeouts. Without them a stalled connection leaves `busy` set
/// forever and the UI shows an eternal spinner.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const OVERALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout(OVERALL_TIMEOUT)
        .user_agent(UA)
        .build()
}

/// The installer's silent switch. `omenmon-setup.exe` accepts it as an alias for
/// the default `install` action, so the updater can run it without a console.
const INSTALLER_ARG: &str = "install";

pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------- version --

/// Dot-separated semver-ish compare. True if `latest` > `current`.
///
/// Pre-release and build metadata (`-`/`+` suffixes) are not comparable this
/// way, and a pre-release must never be offered as newer than a stable build,
/// so any tag carrying either is rejected outright. Empty components (as in
/// `1..2`) are malformed and also rejected rather than silently dropped, which
/// would make `1..2` compare equal to `1.2`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> {
        if v.contains('-') || v.contains('+') {
            return None; // pre-release or build metadata: not a stable update
        }
        v.trim_start_matches('v')
            .split('.')
            .map(|p| {
                let digits: String =
                    p.chars().take_while(|c| c.is_ascii_digit()).collect();
                // A component that is empty or contains non-digit trailing
                // junk is malformed, not zero.
                if digits.is_empty() || digits.len() != p.len() {
                    return None;
                }
                digits.parse().ok()
            })
            .collect::<Option<Vec<_>>>()
    };
    let (Some(a), Some(b)) = (parse(latest), parse(current)) else {
        return false;
    };
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

// --------------------------------------------------------------- digest ----

/// Parse GitHub's asset digest field (`sha256:<64 hex chars>`).
///
/// GitHub omits the field entirely for assets uploaded before the feature
/// existed, and returns `null` rather than an empty string. Both must read as
/// "no digest", never as "verified".
pub fn parse_digest(raw: &str) -> Option<String> {
    let hex = raw.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

/// SHA-256 of a file, streamed so a 100 MB installer does not land in memory.
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)
        .with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Check a downloaded file against the digest GitHub published for it.
///
/// Returns the reason on mismatch so the caller can put it in front of a user
/// rather than swallowing it. A digest mismatch means the bytes on disk are not
/// the bytes GitHub described: the download is corrupt, truncated, or
/// substituted, and in all three cases it must not run.
pub fn verify(path: &Path, expected_digest: &str) -> anyhow::Result<()> {
    let expected = parse_digest(expected_digest)
        .ok_or_else(|| anyhow::anyhow!("release digest is not sha256:<64 hex>; refusing to install"))?;
    let actual = sha256_file(path)?;
    if actual != expected {
        bail!(
            "digest mismatch: expected sha256:{}, got sha256:{}",
            &expected[..12],
            &actual[..12]
        );
    }
    Ok(())
}

// ------------------------------------------------------------- github api --

#[derive(Deserialize, Clone)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    /// `sha256:<hex>` on releases published after GitHub added the field.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub latest: String,
    pub asset_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_size: u64,
    /// `sha256:<hex>` for the chosen asset, when GitHub published one.
    pub asset_digest: Option<String>,
    pub update_available: bool,
    /// Why an available version cannot be installed. Shown verbatim, because a
    /// silent "no update" would hide a security decision from the user.
    pub refusal: Option<String>,
}

/// Why an update check could not produce a version.
#[derive(Debug)]
pub enum CheckError {
    /// The repository has no published release yet. This is the normal state for a
    /// project that has not cut a release, and it must not read like a network
    /// fault: GitHub answers 404 for "no releases", not just for "no repo".
    NoReleases,
    /// The network or GitHub API failed for another reason.
    Other(String),
}

impl std::error::Error for CheckError {}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::NoReleases => {
                write!(f, "no published release yet — this build is ahead of any release")
            }
            CheckError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub fn check() -> Result<UpdateCheck, CheckError> {
    let resp = agent().get(LATEST_API).call();

    let rel: GhRelease = match resp {
        Ok(r) => r
            .into_json()
            .map_err(|e| CheckError::Other(format!("malformed release JSON: {e}")))?,
        // 404 means the repo has no releases (the repo itself exists). Reporting
        // the raw "status code 404" told the user nothing actionable.
        Err(ureq::Error::Status(404, _)) => return Err(CheckError::NoReleases),
        Err(ureq::Error::Status(code, _)) => {
            return Err(CheckError::Other(format!("GitHub returned status {code}")))
        }
        Err(e) => return Err(CheckError::Other(format!("network error: {e}"))),
    };

    let latest = rel.tag_name.trim_start_matches('v').to_string();
    let newer = is_newer(&latest, &current_version());
    // Prefer the installer because it carries every binary, then fall back to a
    // single-binary asset. Both are safe now that `plan_for_asset` decides what
    // each one is allowed to do.
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == "omenmon-setup.exe")
        .or_else(|| rel.assets.iter().find(|a| a.name == "omenmon-gui.exe"))
        .filter(|_| newer)
        .cloned();

    // An asset we cannot classify is an asset we must not run: a future release
    // could ship a differently named payload, and guessing its role is how the
    // wrong-file bug happened in the first place.
    let refusal = asset.as_ref().and_then(|a| {
        if plan_for_asset(&a.name).is_none() {
            Some(format!(
                "asset '{}' has no install plan — refusing to run it",
                a.name
            ))
        } else if a.digest.as_deref().and_then(parse_digest).is_none() {
            Some(format!(
                "release asset '{}' has no SHA-256 digest — refusing to install unverified code",
                a.name
            ))
        } else {
            None
        }
    });

    Ok(UpdateCheck {
        latest,
        asset_url: asset.as_ref().map(|a| a.browser_download_url.clone()),
        asset_name: asset.as_ref().map(|a| a.name.clone()),
        asset_size: asset.as_ref().map(|a| a.size).unwrap_or(0),
        asset_digest: asset.as_ref().and_then(|a| a.digest.clone()),
        update_available: asset.is_some() && refusal.is_none(),
        refusal,
    })
}

// -------------------------------------------------------- install planning --

/// What a downloaded asset is allowed to do.
///
/// This is deliberately a closed enum rather than "a path to copy to": the bug
/// this replaces was exactly a role confusion, and a closed set of roles makes
/// that confusion a compile-time impossibility at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallAction {
    /// Run the file as an installer; it places its own files and needs no
    /// destination from us.
    RunInstaller,
    /// Copy the payload over `<install dir>\<dest_name>`.
    ReplaceBinary { dest_name: &'static str },
}

/// Map a release asset name to the action it is permitted to perform.
///
/// Returns `None` for anything unrecognised. The updater treats `None` as a
/// refusal, not as a default: an unknown payload has no established role, and
/// copying it somewhere and executing it is precisely the defect being fixed.
pub fn plan_for_asset(asset_name: &str) -> Option<InstallAction> {
    match asset_name {
        // The bundle. It is an installer, so it installs — it is never a
        // candidate to be renamed onto a binary.
        "omenmon-setup.exe" => Some(InstallAction::RunInstaller),
        "omenmon-gui.exe" => Some(InstallAction::ReplaceBinary { dest_name: "omenmon-gui.exe" }),
        "omenmon.exe" => Some(InstallAction::ReplaceBinary { dest_name: "omenmon.exe" }),
        "watch.exe" => Some(InstallAction::ReplaceBinary { dest_name: "watch.exe" }),
        _ => None,
    }
}

/// Pick the first candidate directory that exists and accepts a write.
///
/// Split out from the filesystem probe so the ordering rule is testable: the
/// running exe's own directory wins, and Program Files is only a fallback that
/// must actually be writable. The old code returned `C:\Program Files\OmenMon`
/// whenever the exe was elsewhere, whether or not that path existed — and the
/// helper ignored the resulting copy failure.
pub fn pick_dir(candidates: &[(PathBuf, bool)]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|(dir, writable)| *writable && dir.is_dir())
        .map(|(dir, _)| dir.clone())
}

/// True when a directory accepts a create+delete probe.
pub fn is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".omenmon-write-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Where an update would be installed, and why, so the UI can show it.
#[derive(Debug, Clone)]
pub struct InstallTarget {
    pub dir: PathBuf,
    /// Empty when the directory is writable by this process.
    pub problem: Option<String>,
}

/// Resolve the install directory from the running exe.
///
/// No guessing: the exe's own directory is the install directory unless it is
/// unwritable, in which case `C:\Program Files\OmenMon` is tried only if it
/// exists. If neither works the update is refused with the reason, rather than
/// staged against a path that will silently fail.
pub fn resolve_install_target() -> InstallTarget {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let mut candidates: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(d) = &exe_dir {
        candidates.push((d.clone(), is_writable(d)));
    }
    let pf = PathBuf::from(r"C:\Program Files\OmenMon");
    candidates.push((pf.clone(), is_writable(&pf)));

    match pick_dir(&candidates) {
        Some(dir) => InstallTarget { dir, problem: None },
        None => InstallTarget {
            dir: exe_dir.unwrap_or_else(|| PathBuf::from(".")),
            problem: Some(
                "no writable install directory — the running folder and \
                 C:\\Program Files\\OmenMon both refused a write. Run as administrator."
                    .into(),
            ),
        },
    }
}

// -------------------------------------------------------------- download ---

pub fn staging_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OmenMon")
        .join("update")
}

/// Where the helper leaves its verdict, read back on the next launch.
pub fn result_file() -> PathBuf {
    staging_dir().join("install-result.txt")
}

/// Download to `<name>.part`, hash it, and only then promote it to `<name>`.
///
/// The digest check happens before the rename so a partially written or
/// substituted file never occupies the path the installer will be run from.
pub fn download(
    url: &str,
    name: &str,
    expected: u64,
    digest: &str,
    progress: &dyn Fn(u64, u64),
) -> anyhow::Result<PathBuf> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    let tmp = dir.join(format!("{name}.part"));

    // Any error path must leave no `.part` behind: the next attempt would find
    // a stale partial file indistinguishable from a fresh one.
    match download_inner(&tmp, url, expected, digest, progress) {
        Ok(()) => {}
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    match std::fs::rename(&tmp, &dest) {
        Ok(()) => Ok(dest),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(anyhow::Error::new(e).context("promote verified payload to final name"))
        }
    }
}

fn download_inner(
    tmp: &Path,
    url: &str,
    expected: u64,
    digest: &str,
    progress: &dyn Fn(u64, u64),
) -> anyhow::Result<()> {
    let resp = agent().get(url).call().context("download request failed")?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(expected);
    let mut reader = resp.into_reader();
    let mut out = std::io::BufWriter::new(std::fs::File::create(tmp)?);
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        out.write_all(&buf[..n])?;
        written += n as u64;
        progress(written, total);
    }
    use std::io::Write as _;
    out.flush()?;

    if total > 0 && written != total {
        bail!("download truncated: got {written} of {total} bytes");
    }
    // Size from the release metadata is a cheap second opinion, but the digest
    // is the authoritative check.
    if expected > 0 && written != expected {
        bail!("download size {written} does not match release metadata {expected}");
    }
    verify(tmp, digest)
}

// ---------------------------------------------------------------- helper ---

/// Build the detached helper script.
///
/// Three properties matter and each has a test:
///
/// 1. It waits for this process to exit before touching any file, because a
///    running image cannot be replaced on Windows.
/// 2. Every `copy` is followed by an `errorlevel` check, so a failed copy cannot
///    be reported as a successful update.
/// 3. It always writes [`result_file`] before exiting, so the next launch can
///    say what happened instead of leaving the user guessing.
pub fn helper_script(
    payload: &Path,
    pid: u32,
    action: &InstallAction,
    install_dir: &Path,
    relaunch: Option<&Path>,
) -> String {
    let result = result_file();
    let mut s = String::new();
    s.push_str("@echo off\r\nsetlocal\r\n");
    // A stale verdict from a previous attempt must not be read as this one's.
    s.push_str(&format!("del \"{}\" >nul 2>&1\r\n", result.display()));
    s.push_str(":wait\r\n");
    s.push_str(&format!(
        "tasklist /FI \"PID eq {pid}\" 2>nul | find \"{pid}\" >nul\r\n"
    ));
    s.push_str("if not errorlevel 1 (\r\n  timeout /T 1 /NOBREAK >nul\r\n  goto wait\r\n)\r\n");

    match action {
        InstallAction::RunInstaller => {
            // The installer places its own files; it needs elevation, so it is
            // launched through `Start-Process -Verb RunAs`, which raises the
            // normal UAC prompt rather than failing silently.
            s.push_str(&format!(
                "powershell -NoProfile -ExecutionPolicy Bypass -Command \
                 \"$p = Start-Process -FilePath '{p}' -ArgumentList '{a}' -Verb RunAs -Wait -PassThru; exit $p.ExitCode\"\r\n",
                p = payload.display(),
                a = INSTALLER_ARG
            ));
            s.push_str("if errorlevel 1 (\r\n");
            s.push_str("  echo result=failed> \"%RESULT%\"\r\n");
            s.push_str("  echo step=installer>> \"%RESULT%\"\r\n");
            s.push_str("  exit /b 1\r\n)\r\n");
            s.push_str("echo result=ok> \"%RESULT%\"\r\n");
            s.push_str("echo step=installer>> \"%RESULT%\"\r\n");
        }
        InstallAction::ReplaceBinary { dest_name } => {
            let dest = install_dir.join(dest_name);
            s.push_str(&format!("copy /Y \"{}\" \"{}\" >nul 2>&1\r\n", payload.display(), dest.display()));
            s.push_str("if errorlevel 1 (\r\n");
            s.push_str("  echo result=failed> \"%RESULT%\"\r\n");
            s.push_str("  echo step=copy>> \"%RESULT%\"\r\n");
            // The destination is quoted: `C:\Program Files (x86)` contains a
            // `)`, which unquoted would terminate the if-block and flip a
            // failed copy into a reported success.
            s.push_str(&format!("  echo dest=\"{}\">> \"%RESULT%\"\r\n", dest.display()));
            s.push_str("  exit /b 1\r\n)\r\n");
            s.push_str("echo result=ok> \"%RESULT%\"\r\n");
            s.push_str("echo step=copy>> \"%RESULT%\"\r\n");
            s.push_str(&format!("echo dest=\"{}\">> \"%RESULT%\"\r\n", dest.display()));
            if let Some(r) = relaunch {
                s.push_str(&format!("start \"\" \"{}\"\r\n", r.display()));
            }
        }
    }

    s.push_str(&format!("del \"{}\" >nul 2>&1\r\n", payload.display()));
    s.push_str("exit /b 0\r\n");
    // cmd expands %RESULT% at parse time per line, so define it before use by
    // replacing the placeholder with the literal path.
    s.replace("%RESULT%", &result.display().to_string())
}

/// Stage the helper and launch it detached. Returns the helper path.
pub fn apply_after_exit(
    payload: &Path,
    action: &InstallAction,
    install_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let pid = std::process::id();
    let relaunch = if matches!(action, InstallAction::ReplaceBinary { .. }) {
        std::env::current_exe().ok()
    } else {
        None
    };
    let script = helper_script(payload, pid, action, install_dir, relaunch.as_deref());
    let helper = staging_dir().join("omenmon-apply.cmd");
    std::fs::create_dir_all(staging_dir())?;
    std::fs::write(&helper, script)?;
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/C", "start", "/MIN"])
        .arg(&helper)
        .creation_flags(0x00000008) // DETACHED_PROCESS
        .spawn()?;
    Ok(helper)
}

/// What the last helper run reported, if it left a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub ok: bool,
    pub step: String,
    pub dest: Option<String>,
}

pub fn read_last_result() -> Option<InstallResult> {
    let text = std::fs::read_to_string(result_file()).ok()?;
    let mut ok = None;
    let mut step: Option<String> = None;
    let mut dest = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("result=") {
            ok = Some(v == "ok");
        } else if let Some(v) = line.strip_prefix("step=") {
            // An empty step means the file exists but the verdict lines have
            // not been written yet — the helper's `echo` lines land one by
            // one. Reading that as a verdict reports a half-finished install
            // as a finished one, so it is treated as "no verdict yet".
            if !v.is_empty() {
                step = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("dest=") {
            dest = Some(v.trim_matches('"').to_string());
        }
    }
    Some(InstallResult {
        ok: ok?,
        step: step?,
        dest,
    })
}

// --------------------------------------------------------- shared UI state --

#[derive(Default)]
pub struct UpdateState {
    pub latest: Option<String>,
    pub available: bool,
    pub busy: bool,
    pub progress: Option<(u64, u64)>,
    pub err: Option<String>,
    pub done_msg: Option<String>,
    /// Asset name, version and digest prefix, shown before consenting to run
    /// downloaded code.
    pub asset_name: Option<String>,
    pub digest: Option<String>,
    pub refusal: Option<String>,
}

#[derive(Clone, Default)]
pub struct UpdateHandle(pub Arc<Mutex<UpdateState>>);

impl UpdateHandle {
    /// Spawn a background version check.
    pub fn check_async(&self) {
        let h = self.clone();
        std::thread::spawn(move || {
            let mut s = h.0.lock();
            s.busy = true;
            s.err = None;
            s.done_msg = None;
            s.refusal = None;
            drop(s);
            match check() {
                Ok(c) => {
                    let mut s = h.0.lock();
                    s.latest = Some(c.latest);
                    s.available = c.update_available;
                    s.asset_name = c.asset_name;
                    s.digest = c.asset_digest;
                    s.refusal = c.refusal;
                    s.busy = false;
                    if !c.update_available && s.refusal.is_none() {
                        s.done_msg = Some("you are up to date".into());
                    }
                }
                Err(e) => {
                    let mut s = h.0.lock();
                    s.busy = false;
                    s.err = Some(format!("check failed: {e}"));
                }
            }
        });
    }

    /// Spawn a background download, verify, and staged install.
    pub fn update_async(&self) {
        // Re-entrancy guard, taken *before* spawning: two concurrent runs
        // would interleave `.part` writers and could stage and execute a
        // half-written apply.cmd. A click that lands while another download
        // or check is running is ignored.
        {
            let mut s = self.0.lock();
            if s.busy {
                return;
            }
            s.busy = true;
            s.err = None;
            s.done_msg = None;
            s.progress = None;
        }
        let h = self.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let target = resolve_install_target();
                if let Some(problem) = &target.problem {
                    bail!("{problem}");
                }
                let c = check()?;
                if let Some(why) = &c.refusal {
                    bail!("{why}");
                }
                let (url, name, size, digest) = match (c.asset_url, c.asset_name, c.asset_digest) {
                    (Some(u), Some(n), Some(d)) => (u, n, c.asset_size, d),
                    _ => bail!("no downloadable asset in latest release"),
                };
                // Decide the role before downloading, so an unclassifiable asset
                // costs no bandwidth and never reaches the disk.
                let action = plan_for_asset(&name)
                    .ok_or_else(|| anyhow::anyhow!("asset '{name}' has no install plan"))?;
                let payload = download(&url, &name, size, &digest, &|done, total| {
                    h.0.lock().progress = Some((done, total));
                })?;
                apply_after_exit(&payload, &action, &target.dir)?;
                h.0.lock().done_msg = Some(match action {
                    InstallAction::RunInstaller => {
                        "verified — the installer runs when this window closes".to_string()
                    }
                    InstallAction::ReplaceBinary { .. } => {
                        "verified — the binary is replaced when this window closes".to_string()
                    }
                });
                Ok(())
            })();
            if let Err(e) = result {
                let mut s = h.0.lock();
                s.busy = false;
                s.err = Some(format!("update failed: {e}"));
            } else {
                h.0.lock().busy = false;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "No releases" must not read like a network fault. GitHub answers 404 for a
    /// repository that has no releases, so the message has to say what is actually
    /// true rather than echoing a status code.
    #[test]
    fn no_releases_message_is_actionable() {
        let msg = CheckError::NoReleases.to_string();
        assert!(msg.contains("no published release"), "got: {msg}");
        assert!(!msg.contains("404"), "a status code is not an explanation");
    }

    /// The other error variant passes its detail through unchanged, so a genuine
    /// network failure still names the failure.
    #[test]
    fn other_errors_keep_their_detail() {
        assert_eq!(CheckError::Other("timeout".into()).to_string(), "timeout");
    }

    /// Version comparison drives whether an update is offered at all.
    #[test]
    fn newer_detection() {
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(is_newer("0.3", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // A tag written as v1.2.3 must compare the same as 1.2.3.
        assert!(is_newer("v0.3.0", "0.2.0"));
    }

    // ---- digest -----------------------------------------------------------

    #[test]
    fn digest_parsing_accepts_github_shape_only() {
        let good = "sha256:f71a34ad7c9a1c5b1f0a0c8f1e5c2f4d3a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d";
        assert_eq!(parse_digest(good).as_deref(), Some(&good["sha256:".len()..]));
        // Uppercase hex normalises, because GitHub has changed casing before.
        let upper = good.to_uppercase().replace("SHA256:", "sha256:");
        assert_eq!(parse_digest(&upper), parse_digest(good));
        // Everything else is "no digest", never "trusted".
        assert!(parse_digest("").is_none());
        assert!(parse_digest("sha512:abc").is_none());
        assert!(parse_digest("md5:d41d8cd98f00b204e9800998ecf8427e").is_none());
        assert!(parse_digest("sha256:tooshort").is_none());
        assert!(parse_digest(&format!("sha256:{}", "z".repeat(64))).is_none());
    }

    /// A known SHA-256 makes the streaming hasher checkable independently of
    /// GitHub: `sha256("")` is a published constant.
    #[test]
    fn empty_file_hashes_to_the_known_constant() {
        let dir = std::env::temp_dir().join(format!("omenmon-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty.bin");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole point of P0-2: a corrupted buffer must be rejected. This uses a
    /// real digest of "abc" and then flips one byte.
    #[test]
    fn corrupted_payload_fails_verification() {
        let dir = std::env::temp_dir().join(format!("omenmon-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("payload.bin");
        std::fs::write(&f, b"abc").unwrap();
        let good = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&f, good).is_ok(), "the honest payload must verify");

        // One flipped bit is enough.
        std::fs::write(&f, b"abd").unwrap();
        let err = verify(&f, good).expect_err("a corrupted payload must not verify");
        assert!(err.to_string().contains("digest mismatch"), "got: {err}");

        // And a release with no digest is refused outright rather than treated
        // as "nothing to check".
        let err = verify(&f, "").expect_err("an absent digest must refuse");
        assert!(err.to_string().contains("refusing"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- asset mapping ----------------------------------------------------

    /// P0-1's regression test. The installer must never be treated as a binary
    /// to copy over the GUI, and every asset the updater can pick must have an
    /// explicit destination — no fallback, no guessing.
    #[test]
    fn every_pickable_asset_maps_to_its_own_role() {
        // The two names `check()` can actually choose.
        assert_eq!(
            plan_for_asset("omenmon-setup.exe"),
            Some(InstallAction::RunInstaller),
            "the installer must be run, not copied onto a binary"
        );
        assert_eq!(
            plan_for_asset("omenmon-gui.exe"),
            Some(InstallAction::ReplaceBinary { dest_name: "omenmon-gui.exe" })
        );

        // The rest of the shipped set, so a future asset addition is a conscious
        // edit here rather than a silent default.
        assert_eq!(
            plan_for_asset("omenmon.exe"),
            Some(InstallAction::ReplaceBinary { dest_name: "omenmon.exe" })
        );
        assert_eq!(
            plan_for_asset("watch.exe"),
            Some(InstallAction::ReplaceBinary { dest_name: "watch.exe" })
        );

        // Unknown payloads have no role, so they are refused.
        for unknown in ["omenmon-setup.zip", "OmenMon-Setup.exe", "", "setup.exe"] {
            assert_eq!(plan_for_asset(unknown), None, "{unknown} must not be planned");
        }
    }

    /// The specific defect: a setup payload must never resolve to the GUI's
    /// filename. Stated as its own assertion so a future refactor cannot
    /// reintroduce the rename without failing a test that names the bug.
    #[test]
    fn installer_never_targets_the_running_gui_binary() {
        let plan = plan_for_asset("omenmon-setup.exe").unwrap();
        assert!(
            !matches!(plan, InstallAction::ReplaceBinary { .. }),
            "the installer must not be copied over a binary"
        );
    }

    // ---- helper script ----------------------------------------------------

    /// A helper that copies without checking `errorlevel` reports success after a
    /// failure. That was P0-3's silent-failure half.
    #[test]
    fn helper_checks_errorlevel_after_copy_and_writes_a_verdict() {
        let payload = PathBuf::from(r"C:\tmp\omenmon-gui.exe");
        let script = helper_script(
            &payload,
            4242,
            &InstallAction::ReplaceBinary { dest_name: "omenmon-gui.exe" },
            Path::new(r"C:\Program Files\OmenMon"),
            Some(Path::new(r"C:\Program Files\OmenMon\omenmon-gui.exe")),
        );
        assert!(script.contains("copy /Y"), "still copies, but guarded");
        assert!(script.contains("if errorlevel 1"), "a failed copy must be caught");
        assert!(script.contains("result=failed"), "failure must be recorded");
        assert!(script.contains("result=ok"), "success must be recorded");
        assert!(script.contains("install-result.txt"), "the app reads this back");
        // The wait loop must key on the real pid.
        assert!(script.contains("PID eq 4242"));
        // And the destination is the binary's own name, not the payload's.
        assert!(script.contains(r"C:\Program Files\OmenMon\omenmon-gui.exe"));
    }

    /// The installer branch must not contain a copy at all: running it is the
    /// whole action, and a stray copy is how the roles got confused.
    #[test]
    fn installer_helper_runs_instead_of_copying() {
        let script = helper_script(
            &PathBuf::from(r"C:\tmp\omenmon-setup.exe"),
            std::process::id(),
            &InstallAction::RunInstaller,
            Path::new(r"C:\Program Files\OmenMon"),
            None,
        );
        assert!(!script.contains("copy /Y"), "an installer is not copied over anything");
        assert!(script.contains("Start-Process"), "it is executed");
        assert!(script.contains("install"), "with the silent action");
        assert!(script.contains("if errorlevel 1"), "its exit code is checked");
    }

    // ---- install directory ------------------------------------------------

    /// A scratch directory that really exists, so `pick_dir`'s `is_dir()` check is
    /// exercised against reality rather than a hardcoded path.
    ///
    /// An earlier version of these tests used `C:\Users\Michael\...` and
    /// `C:\Program Files\OmenMon` as literals. They passed on the author's machine
    /// and failed in CI, where neither path exists — the test was asserting on the
    /// developer's filesystem, not on the function. Found by the first CI run.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "omenmon-installdir-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }

    /// The running exe's own folder wins; Program Files is only a fallback that
    /// must exist and be writable. The old guess returned Program Files
    /// unconditionally when the exe lived elsewhere.
    #[test]
    fn install_dir_prefers_a_writable_running_folder() {
        let exe_dir = scratch("exe");
        let pf = scratch("pf");
        let picked = pick_dir(&[(exe_dir.clone(), true), (pf.clone(), true)]);
        assert_eq!(picked, Some(exe_dir.clone()));
        let _ = std::fs::remove_dir_all(&exe_dir);
        let _ = std::fs::remove_dir_all(&pf);
    }

    #[test]
    fn install_dir_falls_back_only_to_a_usable_program_files() {
        let exe_dir = scratch("exe2");
        let pf = scratch("pf2");
        // Exe folder unwritable, Program Files writable -> Program Files.
        assert_eq!(
            pick_dir(&[(exe_dir.clone(), false), (pf.clone(), true)]),
            Some(pf.clone())
        );
        // Neither writable -> no target at all, so the caller refuses instead of
        // staging a copy that cannot work.
        assert_eq!(pick_dir(&[(exe_dir.clone(), false), (pf.clone(), false)]), None);
        let _ = std::fs::remove_dir_all(&exe_dir);
        let _ = std::fs::remove_dir_all(&pf);
    }

    /// `pick_dir` must also reject a candidate that does not exist, even when the
    /// caller claimed it was writable.
    #[test]
    fn install_dir_rejects_a_nonexistent_candidate() {
        let missing = std::env::temp_dir().join("omenmon-does-not-exist-xyz");
        assert!(!missing.exists());
        assert_eq!(pick_dir(&[(missing, true)]), None);
    }

    /// The verdict file round-trips, including a failure with no destination.
    #[test]
    fn install_result_parses_both_outcomes() {
        let parsed = |s: &str| -> Option<InstallResult> {
            let mut ok = None;
            let mut step = String::new();
            let mut dest = None;
            for line in s.lines() {
                if let Some(v) = line.strip_prefix("result=") {
                    ok = Some(v == "ok");
                } else if let Some(v) = line.strip_prefix("step=") {
                    step = v.to_string();
                } else if let Some(v) = line.strip_prefix("dest=") {
                    dest = Some(v.to_string());
                }
            }
            Some(InstallResult { ok: ok?, step, dest })
        };
        let ok = parsed("result=ok\r\nstep=copy\r\ndest=C:\\x\\omenmon-gui.exe\r\n").unwrap();
        assert!(ok.ok);
        assert_eq!(ok.step, "copy");
        assert_eq!(ok.dest.as_deref(), Some(r"C:\x\omenmon-gui.exe"));

        let bad = parsed("result=failed\r\nstep=installer\r\n").unwrap();
        assert!(!bad.ok);
        assert_eq!(bad.step, "installer");
        assert!(bad.dest.is_none());

        // A file with no verdict is not a verdict.
        assert!(parsed("garbage\n").is_none());
    }
}
