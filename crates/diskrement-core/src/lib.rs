//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).
//!
//! Functions are reached through their modules — `diff::diff`, `planner::plan`,
//! `manifest::build` — mirroring the Scala `Diff.diff` / `Planner.plan` /
//! `ManifestBuilder.build`. Types are re-exported here for convenience.

pub mod model;

#[cfg(test)]
// Helpers land in the task that first needs them, which is sometimes one task
// ahead of their first consumer; the allow keeps that from failing `-D warnings`
// mid-phase.
#[allow(dead_code)]
mod testutil;

pub use model::{Capacity, CatalogEntry, CatalogView, ContentHash, DiscType, RelPath, ScannedFile};

/// Temporary P0 scaffolding: gives the `shell -> core` dependency edge
/// something to link against. Task 9 retires it in favour of a real type.
pub struct Probe;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs.
    }
}
