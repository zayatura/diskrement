//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).

/// Temporary P0 scaffolding: exists only so the `shell -> core` and
/// `cli -> shell -> core` dependency edges have something real to link.
/// P1 replaces it with the actual model types and deletes it.
pub struct Probe;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs before any real code exists.
    }
}
