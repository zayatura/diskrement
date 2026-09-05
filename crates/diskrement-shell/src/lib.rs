//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

use diskrement_core::RelPath;

/// Temporary scaffolding: proves the `shell -> core` edge links against a real
/// core type. P2 deletes it when the catalog store consumes core types for
/// real.
pub fn probe() -> RelPath {
    RelPath("probe".to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_edge_links() {
        assert_eq!(super::probe().as_str(), "probe");
    }
}
