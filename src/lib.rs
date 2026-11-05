//! Malva Dock: a macOS-style dock written in Rust with Slint.
//!
//! Crate layout:
//!
//! * [`config`]    reads `~/.config/malva/dock.conf` (INI-style) and validates it;
//! * [`geometry`]  the resting layout and hit-testing (pure functions);
//! * [`apps`]      launching, running-app detection and focusing, hardened;
//! * [`state`]     the dock's runtime state (running dots, launch hop, activation);
//! * [`platform`]  small platform helpers (`~` expansion, X11 screen probing, placement);
//! * `main.rs`     the thin binary that wires the state to the Slint window.
//!
//! The UI (`ui/dock.slint`) draws everything, including the fisheye magnification.
//! The crate is a library plus a thin `malva-dock` binary so that every function
//! can be unit-tested without a display server.

// `deny` (not `forbid`): the Slint macro-generated vtables carry their own
// `#[allow(unsafe_code)]`, which a `forbid` would reject. Our own code is still
// fully checked — any `unsafe` block outside the macro output fails the build.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod apps;
pub mod config;
pub mod geometry;
pub mod platform;
pub mod state;

/// The types generated from `ui/dock.slint`, re-exported at the crate root.
/// The `allow`s only cover generated code — hand-written code above is fully linted.
#[allow(missing_docs, unsafe_code, non_camel_case_types, clippy::all)]
mod generated {
    slint::include_modules!();
}
pub use generated::*;

/// Re-exports of the Slint-generated types used by the binary.
pub mod ui {
    pub use crate::{DockItem, DockWindow, Theme};
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Serialises tests that mutate process-global environment variables.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Acquires [`ENV_LOCK`], saves the given variables and restores them on drop.
    /// Use this in every test that calls `std::env::set_var` / `remove_var`, so
    /// parallel tests never see each other's environment.
    pub struct EnvScope {
        _guard: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvScope {
        /// Locks the environment and takes ownership of the listed variables.
        pub fn new(vars: &[&'static str]) -> Self {
            let guard = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = vars
                .iter()
                .map(|var| (*var, std::env::var_os(var)))
                .collect();
            EnvScope {
                _guard: guard,
                saved,
            }
        }
    }

    impl Drop for EnvScope {
        fn drop(&mut self) {
            for (var, value) in self.saved.drain(..) {
                match value {
                    Some(v) => std::env::set_var(var, v),
                    None => std::env::remove_var(var),
                }
            }
        }
    }

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A minimal valid 1x1 transparent PNG, for testing image icon loading.
    pub const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// A throwaway directory that removes itself on drop.
    pub struct TempDir(std::path::PathBuf);

    impl TempDir {
        /// Creates a unique temporary directory.
        pub fn new(tag: &str) -> Self {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "malva-dock-test-{}-{}-{}",
                tag,
                std::process::id(),
                n
            ));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|err| panic!("cannot create {}: {err}", path.display()));
            TempDir(path)
        }

        /// The directory's path.
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }

        /// Writes a file inside the directory and returns its path.
        pub fn file(&self, name: &str, contents: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents)
                .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
