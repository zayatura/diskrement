//! Pure domain types shared across the functional core.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Path of a file relative to the project's source root, using '/' separators.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(pub String);

impl RelPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content hash of a file (SHA-256 hex in production; opaque to the core).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A file observed during a scan, always fully hashed by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub mtime: DateTime<Utc>,
}

/// The current (live) version of one file as recorded in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub rel_path: RelPath,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
}

/// Immutable snapshot of the catalog's current view: rel path -> live version.
///
/// A `BTreeMap` rather than a `HashMap`, so key iteration is deterministic.
/// `diff` sorts its outputs explicitly anyway, but a nondeterministic core
/// would be the hardest bug here to see, so both guards stay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogView {
    pub current: BTreeMap<RelPath, CatalogEntry>,
}

impl CatalogView {
    pub fn empty() -> Self {
        Self::default()
    }

    /// The view that results from recording exactly these files as the current
    /// versions. Used to express diff idempotence (see `DiffPropertyTest`).
    pub fn from_scanned(files: &[ScannedFile]) -> Self {
        CatalogView {
            current: files
                .iter()
                .map(|f| {
                    (
                        f.rel_path.clone(),
                        CatalogEntry {
                            rel_path: f.rel_path.clone(),
                            content_hash: f.content_hash.clone(),
                            size_bytes: f.size_bytes,
                        },
                    )
                })
                .collect(),
        }
    }
}

/// Physical optical media types and their nominal byte capacities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscType {
    BdR,
    BdRDl,
    Dvd,
    DvdDl,
}

impl DiscType {
    pub fn nominal_bytes(self) -> u64 {
        match self {
            DiscType::BdR => 25_000_000_000,
            DiscType::BdRDl => 50_000_000_000,
            DiscType::Dvd => 4_700_000_000,
            DiscType::DvdDl => 8_500_000_000,
        }
    }

    /// The catalog and JSON spelling. Frozen by the data-compatibility
    /// contract; see the spec, §3.1. Do not change these strings.
    pub fn as_str(self) -> &'static str {
        match self {
            DiscType::BdR => "BD_R",
            DiscType::BdRDl => "BD_R_DL",
            DiscType::Dvd => "DVD",
            DiscType::DvdDl => "DVD_DL",
        }
    }
}

impl std::fmt::Display for DiscType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DiscType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BD_R" => Ok(DiscType::BdR),
            "BD_R_DL" => Ok(DiscType::BdRDl),
            "DVD" => Ok(DiscType::Dvd),
            "DVD_DL" => Ok(DiscType::DvdDl),
            other => Err(format!("unknown disc type: {other}")),
        }
    }
}

/// Usable bytes on a disc after subtracting filesystem/UDF overhead margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub usable_bytes: u64,
}

impl Capacity {
    pub fn new(usable_bytes: u64) -> Self {
        Capacity { usable_bytes }
    }

    /// Nominal capacity reduced by `margin_percent` (e.g. 2.0 == 2%), floored.
    ///
    /// The margin is converted to an exact integer count of thousandths of a
    /// percent before dividing, so the result never drifts by a byte the way
    /// `nominal as f64 * (1.0 - margin / 100.0)` does: DVD-DL at a 7% margin
    /// floors to 7_904_999_999 in f64, one byte short of the exact
    /// 7_905_000_000. The Scala reached for `BigDecimal` to solve the same
    /// problem.
    ///
    /// This quantises the margin to three decimal places. `--margin` is a
    /// user-facing percentage where that is far below anything meaningful (a
    /// thousandth of a percent of a BD-R is 250 KB), and the alternative — a
    /// decimal-arithmetic dependency in the one crate that is meant to have
    /// almost none — costs more than it buys.
    ///
    /// A margin above 100% would compute negative and is clamped to zero; the
    /// value comes from `project.margin_percent` and is nonsense in that range.
    pub fn from_disc_type(disc_type: DiscType, margin_percent: f64) -> Self {
        let margin_thousandths = (margin_percent * 1000.0).round() as i128;
        let usable =
            i128::from(disc_type.nominal_bytes()) * (100_000 - margin_thousandths) / 100_000;
        Capacity {
            usable_bytes: usable.max(0) as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, epoch, view_of};

    #[test]
    fn rel_path_orders_lexicographically_by_value() {
        let mut paths = [
            RelPath("b/a".to_string()),
            RelPath("a/z".to_string()),
            RelPath("a/a".to_string()),
        ];
        paths.sort();
        let values: Vec<&str> = paths.iter().map(RelPath::as_str).collect();
        assert_eq!(values, vec!["a/a", "a/z", "b/a"]);
    }

    #[test]
    fn an_empty_catalog_view_has_no_current_entries() {
        assert_eq!(CatalogView::empty().current.len(), 0);
    }

    #[test]
    fn catalog_view_lookups_are_by_rel_path() {
        let e = entry("x/y", "ab");
        let view = view_of(std::slice::from_ref(&e));
        assert_eq!(view.current.get(&RelPath("x/y".to_string())), Some(&e));
    }

    #[test]
    fn a_scanned_file_carries_path_hash_size_and_mtime() {
        let f = ScannedFile {
            rel_path: RelPath("p".to_string()),
            content_hash: ContentHash("cd".to_string()),
            size_bytes: 42,
            mtime: epoch(),
        };
        assert_eq!(f.size_bytes, 42);
        assert_eq!(f.content_hash, ContentHash("cd".to_string()));
        assert_eq!(f.mtime, epoch());
    }

    #[test]
    fn from_scanned_records_every_scanned_file_as_the_current_version() {
        // This is how `DiffPropertyTest` expresses idempotence, so it needs a
        // test of its own rather than only being exercised through a property.
        let files = vec![
            ScannedFile {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
                mtime: epoch(),
            },
            ScannedFile {
                rel_path: RelPath("b".to_string()),
                content_hash: ContentHash("h2".to_string()),
                size_bytes: 9,
                mtime: epoch(),
            },
        ];
        let view = CatalogView::from_scanned(&files);
        assert_eq!(view.current.len(), 2);
        assert_eq!(
            view.current.get(&RelPath("a".to_string())),
            Some(&CatalogEntry {
                rel_path: RelPath("a".to_string()),
                content_hash: ContentHash("h1".to_string()),
                size_bytes: 7,
            })
        );
    }

    #[test]
    fn disc_types_expose_nominal_capacities() {
        assert_eq!(DiscType::BdR.nominal_bytes(), 25_000_000_000);
        assert_eq!(DiscType::BdRDl.nominal_bytes(), 50_000_000_000);
        assert_eq!(DiscType::Dvd.nominal_bytes(), 4_700_000_000);
        assert_eq!(DiscType::DvdDl.nominal_bytes(), 8_500_000_000);
    }

    #[test]
    fn capacity_subtracts_the_margin_percent_and_floors() {
        assert_eq!(
            Capacity::from_disc_type(DiscType::BdR, 2.0).usable_bytes,
            24_500_000_000
        );
    }

    #[test]
    fn zero_margin_yields_nominal_capacity() {
        assert_eq!(
            Capacity::from_disc_type(DiscType::Dvd, 0.0).usable_bytes,
            4_700_000_000
        );
    }

    #[test]
    fn capacity_is_floored_exactly_with_no_floating_point_drift() {
        // 8_500_000_000 * 0.93 == 7_905_000_000 exactly. Computed in f64 this
        // floors to 7_904_999_999 — one byte short. Integer math must not.
        assert_eq!(
            Capacity::from_disc_type(DiscType::DvdDl, 7.0).usable_bytes,
            7_905_000_000
        );
    }

    #[test]
    fn a_fractional_margin_percent_is_honoured_exactly() {
        // 25_000_000_000 * (1 - 2.5/100) == 24_375_000_000 exactly.
        assert_eq!(
            Capacity::from_disc_type(DiscType::BdR, 2.5).usable_bytes,
            24_375_000_000
        );
    }

    #[test]
    fn disc_type_wire_names_round_trip() {
        // Frozen by the data-compatibility contract (spec §3.1): these are the
        // strings the Scala build wrote into `project.default_disc_type` and
        // into the staged `plan.json`.
        for (dt, name) in [
            (DiscType::BdR, "BD_R"),
            (DiscType::BdRDl, "BD_R_DL"),
            (DiscType::Dvd, "DVD"),
            (DiscType::DvdDl, "DVD_DL"),
        ] {
            assert_eq!(dt.as_str(), name);
            assert_eq!(name.parse::<DiscType>().expect("parses"), dt);
        }
    }

    #[test]
    fn an_unknown_disc_type_name_is_rejected() {
        assert!("CD_R".parse::<DiscType>().is_err());
    }
}
