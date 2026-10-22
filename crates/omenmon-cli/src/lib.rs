//! Library facade so standalone binaries (daemon) can reuse CLI internals.

pub mod daemon {
    #[path = "../daemon_bin.rs"]
    pub mod daemon_impl;
    pub use daemon_impl::run;
}

#[cfg(test)]
mod tests {
    /// Every file in `src/bin` must have a `[[bin]]` entry gated behind
    /// `--features probes`, except the handful that ship.
    ///
    /// Cargo auto-discovers `src/bin/*.rs` as binaries. So a probe file with no
    /// manifest entry is built into *every* release — including the installer that
    /// end users run. That happened for real on 2026-10-22: six fan-investigation
    /// probes were added as plain files and shipped in `cargo build --release`.
    ///
    /// The shipped set is listed explicitly rather than inferred, because the
    /// failure mode is "a new file silently becomes a product binary" and only an
    /// allowlist catches that.
    #[test]
    fn no_probe_leaks_into_release() {
        const SHIPPED: &[&str] = &["main.rs", "watch.rs", "setup.rs", "daemon.rs"];

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let bin_dir = std::path::Path::new(manifest_dir).join("src/bin");
        let manifest =
            std::fs::read_to_string(std::path::Path::new(manifest_dir).join("Cargo.toml"))
                .expect("Cargo.toml must be readable");

        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&bin_dir).expect("src/bin must exist") {
            let path = entry.expect("readable dir entry").path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            if SHIPPED.contains(&name) {
                continue;
            }
            let stem = name.trim_end_matches(".rs");
            // Find this binary's block in the manifest and require the gate.
            let gated = manifest
                .split("[[bin]]")
                .any(|block| block.contains(&format!("name = \"{stem}\"")) && block.contains("required-features"));
            if !gated {
                offenders.push(name.to_string());
            }
        }

        assert!(
            offenders.is_empty(),
            "these src/bin files have no gated [[bin]] entry and would ship in every \
             release: {offenders:#?}"
        );
    }
}
