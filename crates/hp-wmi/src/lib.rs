//! hp-wmi: HP WMI BIOS interface (hpqBIntM) — the primary hardware control path.
//!
//! Proven working on HP Victus 16 (board 8C9C, BIOS F.17) and compatible with
//! HP Omen boards (8A13/8A14) that OmenMon targets.

mod error;
pub mod bios_settings;
pub mod payload;
mod commands;
pub mod session;

pub use bios_settings::{BiosSetting, BiosSettings, SetResult};
pub use commands::*;
pub use error::{Error, Result};
pub use session::HpWmi;

#[cfg(test)]
mod encoding_tests {
    use std::path::{Path, PathBuf};

    /// Map a character back to the single CP1252 byte that would have produced
    /// it. `None` means the character cannot be CP1252 output at all, which by
    /// itself proves it is not mojibake.
    fn cp1252_bytes(run: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(run.len());
        for ch in run.chars() {
            let b = match ch {
                '\u{20AC}' => 0x80,
                '\u{201A}' => 0x82,
                '\u{0192}' => 0x83,
                '\u{201E}' => 0x84,
                '\u{2026}' => 0x85,
                '\u{2020}' => 0x86,
                '\u{2021}' => 0x87,
                '\u{02C6}' => 0x88,
                '\u{2030}' => 0x89,
                '\u{0160}' => 0x8A,
                '\u{2039}' => 0x8B,
                '\u{0152}' => 0x8C,
                '\u{017D}' => 0x8E,
                '\u{2018}' => 0x91,
                '\u{2019}' => 0x92,
                '\u{201C}' => 0x93,
                '\u{201D}' => 0x94,
                '\u{2022}' => 0x95,
                '\u{2013}' => 0x96,
                '\u{2014}' => 0x97,
                '\u{02DC}' => 0x98,
                '\u{2122}' => 0x99,
                '\u{0161}' => 0x9A,
                '\u{203A}' => 0x9B,
                '\u{0153}' => 0x9C,
                '\u{017E}' => 0x9E,
                '\u{0178}' => 0x9F,
                c if (c as u32) < 0x80 => c as u32 as u8,
                c if (0xA0..=0xFF).contains(&(c as u32)) => c as u32 as u8,
                _ => return None,
            };
            out.push(b);
        }
        Some(out)
    }

    /// True when a run of non-ASCII characters is UTF-8 bytes that were decoded
    /// as CP1252 and then re-encoded as UTF-8 — the exact damage
    /// `Set-Content -Encoding UTF8` does.
    ///
    /// Two shapes count as damage:
    ///
    /// * the bytes are valid UTF-8 and decode to *fewer* characters, which is
    ///   every complete multi-byte glyph (U+2014 arrives as three characters);
    /// * the bytes are the **first two bytes of a 3-byte sequence** — what a
    ///   U+25xx glyph becomes when its trailing byte is lost. That is the
    ///   truncated shape this incident actually produced.
    ///
    /// Genuine glyphs survive: U+2014 is the single byte 0x97 and U+00B0 is
    /// 0xB0, both UTF-8 continuation bytes with no valid decode; U+00D7 is 0xD7,
    /// a *two*-byte lead, so the 3-byte rule cannot reach it. U+2192, U+25CF and
    /// U+00A7 have no CP1252 byte at all.
    fn looks_like_mojibake(run: &str) -> bool {
        let Some(raw) = cp1252_bytes(run) else {
            return false;
        };
        match std::str::from_utf8(&raw) {
            Ok(s) => s.chars().count() < run.chars().count(),
            Err(_) => {
                // Truncated 3-byte sequence: lead 0xE0..=0xEF, then one or two
                // continuation bytes, and nothing else in the run.
                (2..=3).contains(&raw.len())
                    && (0xE0..=0xEF).contains(&raw[0])
                    && raw[1..].iter().all(|b| (0x80..=0xBF).contains(b))
            }
        }
    }

    fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().map(|n| n == "target").unwrap_or(false) {
                    continue;
                }
                collect_rs(&p, out);
            } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }

    /// Split a line into its maximal runs of non-ASCII characters.
    ///
    /// `looks_like_mojibake` is defined per run, because mojibake never spans an
    /// ASCII character: the damage is a whole glyph, and glyphs are separated by
    /// the ASCII text around them.
    fn non_ascii_runs(line: &str) -> Vec<String> {
        let mut runs = Vec::new();
        let mut cur = String::new();
        for ch in line.chars() {
            if ch.is_ascii() {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            runs.push(cur);
        }
        runs
    }

    /// True when any run in `text` is mojibake.
    fn has_mojibake(text: &str) -> bool {
        non_ascii_runs(text).iter().any(|r| looks_like_mojibake(r))
    }

    /// Regression guard for the 2026-10-21 mojibake incident.
    ///
    /// A PowerShell `Set-Content -Encoding UTF8` pass double-encoded every
    /// non-ASCII character in the tree (157 occurrences), turning `—` into
    /// three characters, `°` into two and `●` into a truncated pair. The damage
    /// compiles cleanly and is invisible in a diff, so only a test catches it.
    #[test]
    fn no_mojibake() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        collect_rs(&root.join("crates"), &mut files);
        assert!(!files.is_empty(), "found no .rs files under crates/");

        let mut offenders = Vec::new();
        for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            for (lineno, line) in text.lines().enumerate() {
                for run in non_ascii_runs(line) {
                    if looks_like_mojibake(&run) {
                        offenders.push(format!("{}:{}: {run}", f.display(), lineno + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "mojibake found ({} runs) — repair with a cp1252->utf8 round trip:\n{}",
            offenders.len(),
            offenders.join("\n")
        );
    }

    /// The detector must not fire on the legitimate glyphs the UI uses, and must
    /// catch every shape the incident produced.
    #[test]
    fn detector_ignores_real_glyphs() {
        for good in [
            "degraded \u{2014} run elevated",
            "42 points \u{00b7} read-only",
            "25 \u{00b0}C",
            "\u{25cf} OK",
            "a \u{2192} b",
            "5\u{2013}25 W",
            "read \u{00a7}3",
            "4 \u{00d7} u32",
            "50 \u{00b1} 3",
            "wait\u{2026}",
        ] {
            assert!(!has_mojibake(good), "false positive on {good:?}");
        }
        for bad in [
            "degraded \u{00e2}\u{20ac}\u{201d} run elevated",
            "25 \u{00c2}\u{00b0}C",
            "\u{00e2}\u{2014} OK",
            "\u{00e2}\u{20ac}\u{2026}",
            "4 \u{00c3}\u{2014} u32",
            "a \u{00e2}\u{2020}\u{2019} b",
        ] {
            assert!(has_mojibake(bad), "missed mojibake in {bad:?}");
        }
    }
}
