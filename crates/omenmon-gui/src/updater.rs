//! GitHub-releases auto-updater.
//! Checks https://github.com/michaelxdips/OmenMonX/releases for a newer
//! version (semver), downloads the matching asset into
//! %LOCALAPPDATA%\OmenMon\update, and stages a self-replace that runs when
//! this process exits (a running .exe cannot overwrite itself).

use anyhow::{bail, Context};
use parking_lot::Mutex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const LATEST_API: &str = "https://api.github.com/repos/michaelxdips/OmenMonX/releases/latest";
const UA: &str = concat!("OmenMon-rs/", env!("CARGO_PKG_VERSION"));

pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------- version --

/// Dot-separated semver-ish compare. True if `latest` > `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    };
    let (a, b) = (parse(latest), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

// ------------------------------------------------------------- github api --

#[derive(Deserialize, Clone)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
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
    pub update_available: bool,
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
    let resp = ureq::AgentBuilder::new()
        .user_agent(UA)
        .build()
        .get(LATEST_API)
        .call();

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
    // prefer the full installer payload (bundles CLI + GUI + setup)
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == "omenmon-setup.exe")
        .or_else(|| rel.assets.iter().find(|a| a.name == "omenmon-gui.exe"))
        .filter(|_| newer)
        .cloned();
    Ok(UpdateCheck {
        latest,
        asset_url: asset.as_ref().map(|a| a.browser_download_url.clone()),
        asset_name: asset.as_ref().map(|a| a.name.clone()),
        asset_size: asset.as_ref().map(|a| a.size).unwrap_or(0),
        update_available: asset.is_some(),
    })
}

// -------------------------------------------------------------- download ---

pub fn staging_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OmenMon")
        .join("update")
}

pub fn download(
    url: &str,
    name: &str,
    expected: u64,
    progress: &dyn Fn(u64, u64),
) -> anyhow::Result<PathBuf> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    let tmp = dir.join(format!("{name}.part"));

    let resp = ureq::AgentBuilder::new()
        .user_agent(UA)
        .build()
        .get(url)
        .call()
        .context("download request failed")?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(expected);
    let mut reader = resp.into_reader();
    let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
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
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Stage a detached helper that waits for this process to exit, copies the
/// payload over the installed binary, and relaunches. Returns the helper path.
pub fn apply_after_exit(payload: &Path, install_dir: &str) -> anyhow::Result<PathBuf> {
    let exe_name = std::env::current_exe()?
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "omenmon-gui.exe".into());
    let running = std::env::current_exe()?;
    let installed = PathBuf::from(install_dir).join(&exe_name);
    let pid = std::process::id();
    let script = format!(
        ":wait\r\ntasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul\r\nif not errorlevel 1 (timeout /T 1 /NUL >nul & goto wait)\r\ncopy /Y \"{p}\" \"{r}\"\r\nif exist \"{i}\" copy /Y \"{p}\" \"{i}\"\r\nstart \"\" \"{r}\"\r\ndel \"{p}\"\r\n",
        p = payload.display(),
        r = running.display(),
        i = installed.display(),
    );
    let helper = staging_dir().join("omenmon-apply.cmd");
    std::fs::write(&helper, script)?;
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/C", "start", "/MIN"])
        .arg(&helper)
        .creation_flags(0x00000008) // DETACHED_PROCESS
        .spawn()?;
    Ok(helper)
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
            drop(s);
            match check() {
                Ok(c) => {
                    let mut s = h.0.lock();
                    s.latest = Some(c.latest);
                    s.available = c.update_available;
                    s.busy = false;
                    if !c.update_available {
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

    /// Spawn a background download + staged self-replace.
    pub fn update_async(&self, install_dir: &str) {
        let h = self.clone();
        let dir = install_dir.to_string();
        std::thread::spawn(move || {
            let mut s = h.0.lock();
            s.busy = true;
            s.err = None;
            s.done_msg = None;
            s.progress = None;
            drop(s);
            let result = (|| -> anyhow::Result<()> {
                let c = check()?;
                let (url, name, size) = match (c.asset_url, c.asset_name) {
                    (Some(u), Some(n)) => (u, n, c.asset_size),
                    _ => bail!("no downloadable asset in latest release"),
                };
                let payload = download(&url, &name, size, &|done, total| {
                    h.0.lock().progress = Some((done, total));
                })?;
                apply_after_exit(&payload, &dir)?;
                h.0.lock().done_msg =
                    Some("update staged — applies when this window closes, then relaunches".into());
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
}
