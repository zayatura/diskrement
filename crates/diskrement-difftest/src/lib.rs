//! Differential harness: run one scenario through the Scala binary, the Rust
//! binary, or the two alternating, and prove the runs agree (spec §8.1).
//!
//! **This crate is disposable.** It exists only while both implementations do,
//! and P10 deletes it together with the Scala tree. Nothing shipping may depend
//! on it.
//!
//! The harness complements the golden corpus under `fixtures/` rather than
//! replacing it. The corpus is the JVM-free, default-on regression floor; this
//! is the opt-in deep check that also covers the reverse direction — Scala
//! reading what Rust wrote — which a recording structurally cannot.

use std::path::{Path, PathBuf};

pub mod content;
pub mod scenario;
// Each later task adds its own `pub mod` line here as it creates the file.
// Declaring them all up front would break the build until the last one lands.

/// Absolute path of the repository root.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels under the repository root")
}
