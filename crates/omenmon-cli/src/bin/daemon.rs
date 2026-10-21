//! omenmon-daemon standalone binary — delegates to the shared daemon module.

fn main() -> anyhow::Result<()> {
    omenmon_cli::daemon::run()
}
