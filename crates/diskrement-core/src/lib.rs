//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).
//!
//! Functions are reached through their modules — `diff::diff`, `planner::plan`,
//! `manifest::build`, `replication::shortfall` — mirroring the Scala `Diff.diff` / `Planner.plan` /
//! `ManifestBuilder.build`. Types are re-exported here for convenience.

pub mod diff;
pub mod manifest;
pub mod model;
pub mod planner;
pub mod replication;

#[cfg(test)]
// Helpers land in the task that first needs them, which is sometimes one task
// ahead of their first consumer; the allow keeps that from failing `-D warnings`
// mid-phase.
#[allow(dead_code)]
mod testutil;

pub use diff::ChangeSet;
pub use manifest::{ManifestChangeKind, ManifestEntry, SessionManifest};
pub use model::{
    set_letter, Capacity, CatalogEntry, CatalogView, ContentHash, DiscArg, DiscKey, DiscType,
    RelPath, ScannedFile, MAX_COPIES,
};
pub use planner::{BurnPlan, DiscState, DiscStatus, PlannedDisc};
pub use replication::{DiscContents, SetCoverage};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs.
    }
}
