//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

use diskrement_core::Probe;

/// Temporary P0 scaffolding: proves the `shell -> core` edge links.
/// P2 deletes it when real adapters consume real core types.
pub fn probe() -> Probe {
    Probe
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_edge_links() {
        let _: diskrement_core::Probe = super::probe();
    }
}
