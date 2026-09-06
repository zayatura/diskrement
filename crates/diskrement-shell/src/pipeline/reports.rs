//! Pipeline reports: plain data the CLI renders (`Pipeline.scala`'s result
//! types). The pipeline returns these and performs no path resolution.

use chrono::{DateTime, Utc};

use diskrement_core::DiscState;

/// `ScanResult` — `hashed` is `None` when a staged scan was reused.
pub struct ScanReport {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub hashed: Option<usize>,
    pub scanned_files: usize,
}

/// `StatusReport`.
pub struct StatusReport {
    pub discs: Vec<DiscState>,
    pub open_disc_ordinal: Option<u32>,
    pub total_current_files: usize,
    pub last_burn_at: Option<DateTime<Utc>>,
}
