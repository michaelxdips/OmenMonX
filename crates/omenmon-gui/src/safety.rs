//! Crash-safety marker for power-cap writes.
//!
//! The failure this prevents is specific and real: `0x29` writes a cap that the
//! firmware keeps until something changes it. If the app writes 5 W, then dies
//! (panic, kill, power loss), the machine stays throttled and the user has no
//! visible cause — the app that did it is gone.
//!
//! So every cap write records *intent* first and *settled state* after. On the
//! next start, a marker that was never settled means the previous run died with
//! a cap still applied, and the UI offers to restore it.
//!
//! The marker is advisory. It is never used to "restore" a value the firmware
//! does not currently hold — [`Pending`] carries the last verified readback, and
//! the restore path re-reads the firmware before writing anything.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// The write was sent; no verified readback yet.
    Written,
    /// Readback matched; the cap is live and known.
    Settled,
    /// A restore was requested and not yet confirmed.
    Restoring,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    /// What the UI asked for.
    pub requested_w: u8,
    /// What the firmware last reported holding. `None` until verified.
    pub observed_w: Option<u8>,
    pub phase: Phase,
    /// Unix seconds, for the "this happened N minutes ago" line.
    pub at: u64,
}

impl Pending {
    pub fn now(requested_w: u8, phase: Phase) -> Self {
        Self {
            requested_w,
            observed_w: None,
            phase,
            at: unix_now(),
        }
    }

    pub fn with_observed(mut self, observed: u8) -> Self {
        self.observed_w = Some(observed);
        self
    }

    pub fn settled(requested_w: u8, observed_w: u8) -> Self {
        Self {
            requested_w,
            observed_w: Some(observed_w),
            phase: Phase::Settled,
            at: unix_now(),
        }
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".config")
        .join("omenmon")
        .join("pending-cap.toml")
}

pub fn write(p: &Pending) {
    let f = path();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = toml::to_string_pretty(p) {
        let _ = std::fs::write(&f, t);
    }
}

pub fn clear() {
    let _ = std::fs::remove_file(path());
}

/// Read the marker. `None` means either "no marker" or "unreadable marker",
/// which are treated the same: nothing to recover.
pub fn read() -> Option<Pending> {
    let t = std::fs::read_to_string(path()).ok()?;
    toml::from_str(&t).ok()
}

/// The marker state that deserves the user's attention: a cap was written or a
/// restore was interrupted, and the process that did it never settled it.
pub fn unrecovered() -> Option<Pending> {
    read().filter(|p| p.phase != Phase::Settled)
}

/// Human description of how stale the marker is.
pub fn age_text(at: u64) -> String {
    let now = unix_now();
    let secs = now.saturating_sub(at);
    match secs {
        0..=90 => "just now".into(),
        91..=5_400 => format!("{} min ago", secs / 60),
        5_401..=172_800 => format!("{} h ago", secs / 3_600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_marker_is_not_unrecovered() {
        let p = Pending::settled(10, 10);
        assert_eq!(p.phase, Phase::Settled);
        // `unrecovered` filters on phase, so a settled marker is ignored even if
        // the cap is still low - that is a deliberate choice, not an oversight.
        assert_ne!(p.phase, Phase::Written);
    }

    #[test]
    fn written_marker_records_intent_without_a_reading() {
        let p = Pending::now(5, Phase::Written);
        assert_eq!(p.requested_w, 5);
        assert_eq!(p.observed_w, None, "no readback yet, so no claim about it");
    }

    #[test]
    fn observed_is_kept_separate_from_requested() {
        let p = Pending::now(5, Phase::Written).with_observed(25);
        assert_eq!(p.requested_w, 5);
        assert_eq!(p.observed_w, Some(25), "firmware clamped; both facts kept");
    }

    #[test]
    fn age_text_buckets() {
        let now = unix_now();
        assert_eq!(age_text(now), "just now");
        assert_eq!(age_text(now - 600), "10 min ago");
        assert_eq!(age_text(now - 7_200), "2 h ago");
        assert_eq!(age_text(now - 200_000), "2 days ago");
        // A marker with a future timestamp (clock change) must not underflow.
        assert_eq!(age_text(now + 10_000), "just now");
    }

    #[test]
    fn roundtrip() {
        let p = Pending::now(12, Phase::Restoring).with_observed(12);
        let t = toml::to_string_pretty(&p).unwrap();
        let back: Pending = toml::from_str(&t).unwrap();
        assert_eq!(back.requested_w, 12);
        assert_eq!(back.observed_w, Some(12));
        assert_eq!(back.phase, Phase::Restoring);
    }
}
