//! Console output helpers.
//!
//! Windows consoles are not UTF-8 by default. When a scheduled task or a pipe
//! redirects stdout, the console codepage can be CP437/CP1252, and a `°` written
//! as UTF-8 arrives as `??`. That is cosmetic in a terminal but corrupts log
//! files that other tooling reads.
//!
//! Two defences, in order of preference:
//!
//! 1. Ask the console for UTF-8 (`SetConsoleOutputCP(65001)`) at startup. This
//!    fixes interactive use and needs no change to the strings.
//! 2. Fall back to ASCII when the output is redirected and the codepage cannot be
//!    set, so a log file is never filled with replacement characters.

use std::sync::atomic::{AtomicBool, Ordering};

static UTF8_OK: AtomicBool = AtomicBool::new(false);

/// Try to put the console into UTF-8 mode. Safe to call more than once.
pub fn init() {
    // SetConsoleOutputCP is the documented way; ignore failure and stay ASCII.
    let ok = unsafe {
        windows::Win32::System::Console::SetConsoleOutputCP(65001).is_ok()
    };
    UTF8_OK.store(ok, Ordering::Relaxed);
}

/// True when non-ASCII output is safe.
pub fn utf8_ok() -> bool {
    UTF8_OK.load(Ordering::Relaxed)
}

/// The degree symbol, or an ASCII fallback when the console cannot render it.
pub fn deg() -> &'static str {
    if utf8_ok() {
        "\u{00b0}"
    } else {
        "deg "
    }
}

/// Format a temperature with the right unit for this console.
pub fn temp_c(v: f32) -> String {
    format!("{v:.0} {}C", deg())
}

/// Format a temperature with one decimal.
pub fn temp_c1(v: f32) -> String {
    format!("{v:.1} {}C", deg())
}

#[cfg(test)]
mod tests {
    #[test]
    fn ascii_fallback_is_ascii() {
        // The fallback must contain no non-ASCII bytes, or it defeats its purpose.
        assert!("deg ".is_ascii());
        assert!(temp_c_ascii(42.0).is_ascii());
    }

    #[test]
    fn utf8_form_has_the_degree_sign() {
        assert_eq!(format!("{:.0} {}C", 42.0, "\u{00b0}"), "42 °C");
    }

    fn temp_c_ascii(v: f32) -> String {
        format!("{v:.0} deg C")
    }
}
