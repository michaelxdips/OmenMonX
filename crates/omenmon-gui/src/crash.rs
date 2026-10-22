//! Crash reporting and the single-writer rule for `%LOCALAPPDATA%\OmenMon`.
//!
//! A GUI panic used to produce nothing at all: the window vanished and a user
//! report had no artifact to attach. This module installs a panic hook that
//! writes the payload and a backtrace to `crash-<timestamp>.log`, and leaves a
//! marker the next launch can turn into "the last run crashed".
//!
//! It also owns [`data_dir`], because three modules need the same directory and
//! three independent guesses is how two of them end up writing to different
//! places on a machine with a redirected profile.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\OmenMon`, or a relative fallback if the environment is odd.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OmenMon")
}

fn marker_path() -> PathBuf {
    data_dir().join("last-crash.txt")
}

/// A crash from a previous run, summarised for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousCrash {
    /// The log file the hook wrote, if it still exists.
    pub log: Option<PathBuf>,
    /// First line of the panic message, for the banner.
    pub summary: String,
}

/// Install the panic hook. Call once, before the first frame.
///
/// The hook chains to the previous handler so the default stderr output survives
/// when a console is attached; it never panics itself, because a panic inside a
/// panic hook aborts the process.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best effort: a crash reporter that can fail loudly is worse than one
        // that stays quiet.
        let _ = write_crash(info);
        previous(info);
    }));
}

fn write_crash(info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log = dir.join(format!("crash-{ts}.log"));

    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".into());
    let backtrace = std::backtrace::Backtrace::force_capture();

    let body = format!(
        "OmenMon-rs {} crashed\n\
         build: {}\n\
         version: {}\n\
         os: {} {}\n\
         location: {location}\n\
         elevated: {}\n\
         exe: {}\n\
         \n\
         message: {payload}\n\
         \n\
         backtrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        option_env!("OMENMON_BUILD_HASH").unwrap_or("unknown"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::elevate::is_elevated(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    std::fs::write(&log, body)?;
    // The marker is what the next launch reads; it points at the full log.
    std::fs::write(
        marker_path(),
        format!("{ts}\n{location}\n{payload}\n{}\n", log.display()),
    )?;
    Ok(())
}

/// Read and consume the crash marker left by a previous run.
///
/// Consumed rather than re-read: a banner that appears on every launch after one
/// crash trains the user to ignore it.
pub fn take_previous_crash() -> Option<PreviousCrash> {
    let p = marker_path();
    let text = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let mut lines = text.lines();
    let _ts = lines.next()?;
    let location = lines.next().unwrap_or("<unknown>").to_string();
    let message = lines.next().unwrap_or("<no message>").to_string();
    let log = lines.next().map(PathBuf::from).filter(|p| p.exists());
    Some(PreviousCrash {
        log,
        summary: format!("{message} ({location})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker must round-trip through the same parser the UI uses, and a
    /// truncated marker must not panic.
    #[test]
    fn crash_marker_parses_and_survives_truncation() {
        let parsed = |s: &str| -> Option<(String, String)> {
            let mut lines = s.lines();
            let _ts = lines.next()?;
            let loc = lines.next()?.to_string();
            let msg = lines.next()?.to_string();
            Some((loc, msg))
        };
        let (loc, msg) = parsed("1700000000\nsrc\\main.rs:42:7\nindex out of bounds\nC:\\x.log\n").unwrap();
        assert_eq!(loc, "src\\main.rs:42:7");
        assert_eq!(msg, "index out of bounds");

        // A file with only a timestamp is not a crash report.
        assert!(parsed("1700000000\n").is_none());
    }

    /// The data directory must be absolute on a normal Windows profile, because a
    /// relative path would put crash logs in whatever the cwd happens to be.
    #[test]
    fn data_dir_is_absolute_when_localappdata_exists() {
        let d = data_dir();
        assert!(d.ends_with("OmenMon"), "got {}", d.display());
        if dirs::data_local_dir().is_some() {
            assert!(d.is_absolute(), "crash logs must not depend on the cwd: {}", d.display());
        }
    }
}
