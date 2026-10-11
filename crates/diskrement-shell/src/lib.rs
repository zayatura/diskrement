//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

pub mod burn;
pub mod catalog;
pub mod error;
pub mod fs;
pub mod pipeline;
pub mod progress;
