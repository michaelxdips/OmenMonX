//! Library facade so standalone binaries (daemon) can reuse CLI internals.

pub mod daemon {
    #[path = "../daemon_bin.rs"]
    pub mod daemon_impl;
    pub use daemon_impl::run;

    #[cfg(test)]
    mod tests {
        use super::daemon_impl::{log_dir, InstanceGuard};
        use std::sync::Mutex;

        /// The tests share the single real lock path (%LOCALAPPDATA%\OmenMon),
        /// so they must not run concurrently with each other or with a running
        /// daemon on this machine.
        static LOCK: Mutex<()> = Mutex::new(());

        /// The guard must be reentrant-fail: acquiring twice in one process
        /// (or two live processes) must fail with the "already running" error,
        /// and dropping the guard must remove the lock file.
        #[test]
        fn instance_guard_blocks_second_acquire_and_cleans_up() {
            let _serial = LOCK.lock().unwrap();
            let path = log_dir().join("daemon.lock");
            // Clear any stale lock from an interrupted previous run.
            let _ = std::fs::remove_file(&path);

            let first = InstanceGuard::acquire().expect("first acquire must succeed");
            assert!(path.exists(), "lock file must exist while held");

            let second = InstanceGuard::acquire();
            assert!(
                second.is_err(),
                "second acquire while live must fail"
            );
            let msg = match second {
                Ok(_) => panic!("second acquire while live must fail"),
                Err(e) => e.to_string(),
            };
            assert!(
                msg.contains("already running"),
                "error must say 'already running', got: {msg}"
            );

            drop(first);
            assert!(
                !path.exists(),
                "dropping the guard must remove the lock file"
            );

            // And a fresh acquire must now succeed.
            let third = InstanceGuard::acquire().expect("acquire after drop must succeed");
            drop(third);
            assert!(!path.exists());
        }

        /// A stale lock (owner pid not alive) must be broken, not honoured.
        #[test]
        fn instance_guard_breaks_stale_lock() {
            let _serial = LOCK.lock().unwrap();
            let path = log_dir().join("daemon.lock");
            std::fs::create_dir_all(log_dir()).unwrap();
            // A pid that cannot be alive: Windows pids are multiples of 4.
            std::fs::write(&path, b"999998").unwrap();

            let g = InstanceGuard::acquire().expect("stale lock must be broken");
            // Verify the file now records OUR pid, not the stale one.
            let txt = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                txt.trim(),
                std::process::id().to_string(),
                "lock must be rewritten with the current pid"
            );
            drop(g);
            let _ = std::fs::remove_file(&path);
        }
    }
}
