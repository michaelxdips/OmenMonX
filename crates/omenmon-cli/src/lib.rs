//! Library facade so standalone binaries (daemon) can reuse CLI internals.

pub mod daemon {
    #[path = "../daemon_bin.rs"]
    pub mod daemon_impl;
    pub use daemon_impl::run;
}
