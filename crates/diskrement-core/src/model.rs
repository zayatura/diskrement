//! Pure domain types shared across the functional core.

use std::collections::{BTreeMap, BTreeSet};

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

/// The most copy sets a project may have: one per letter, `A` to `Z`.
pub const MAX_COPIES: u32 = 26;

/// The letter a copy set is shown as: 0 is `A`, 1 is `B`, and so on.
///
/// Panics past `Z`; `init` refuses `--copies` above [`MAX_COPIES`], so a
/// stored project can never reach it.
pub fn set_letter(copy_set: u32) -> char {
    assert!(copy_set < MAX_COPIES, "copy set {copy_set} has no letter");
    char::from(b'A' + copy_set as u8)
}

/// One physical disc: its ordinal within its copy set, and the set.
///
/// Field order is the sort order — ordinal first — so a sorted list reads
/// 1A, 1B, 2A, 2B: twins side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiscKey {
    pub ordinal: u32,
    pub copy_set: u32,
}

impl DiscKey {
    pub fn new(ordinal: u32, copy_set: u32) -> Self {
        DiscKey { ordinal, copy_set }
    }

    /// How the operator sees this disc: `3` in a single-copy project, `3B`
    /// otherwise. Labels, staging directories, prompts and reports all use
    /// this, so a project's discs are named one way everywhere.
    pub fn name(&self, copies: u32) -> String {
        if copies <= 1 {
            self.ordinal.to_string()
        } else {
            format!("{}{}", self.ordinal, set_letter(self.copy_set))
        }
    }
}

/// A disc as the operator typed it: `3`, `3B` or `3b`.
///
/// The ordinal stays signed so `verify --disc -1` reaches the domain and is
/// answered with exit 2 (CLAUDE.md, exit-code contract) rather than being
/// refused as an argument error. A set letter is only accepted after a
/// positive ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscArg {
    pub ordinal: i64,
    pub copy_set: Option<u32>,
}

impl DiscArg {
    /// The disc this names in a project with `copies` sets, or `None` when the
    /// ordinal is not positive or the letter is past the project's last set.
    /// A bare ordinal means set `A`.
    pub fn resolve(&self, copies: u32) -> Option<DiscKey> {
        let copy_set = self.copy_set.unwrap_or(0);
        let ordinal = u32::try_from(self.ordinal).ok().filter(|o| *o >= 1)?;
        (copy_set < copies).then_some(DiscKey::new(ordinal, copy_set))
    }
}

impl std::str::FromStr for DiscArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || format!("not a disc: {s:?} (expected e.g. 3 or 3B)");
        let (digits, letter) = match s.chars().last() {
            Some(c) if c.is_ascii_alphabetic() => (&s[..s.len() - 1], Some(c.to_ascii_uppercase())),
            _ => (s, None),
        };
        let ordinal: i64 = digits.parse().map_err(|_| bad())?;
        match letter {
            Some(_) if ordinal < 1 => Err(bad()),
            Some(c) => Ok(DiscArg {
                ordinal,
                copy_set: Some(u32::from(c as u8 - b'A')),
            }),
            None => Ok(DiscArg {
                ordinal,
                copy_set: None,
            }),
        }
    }
}

impl std::fmt::Display for DiscArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.copy_set {
            Some(set) => write!(f, "{}{}", self.ordinal, set_letter(set)),
            None => write!(f, "{}", self.ordinal),
        }
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
    /// The copy sets holding at least one copy of this version on a disc not
    /// marked dead. Never empty in a view the catalog produced: a version with
    /// no live copy is left out of the view entirely.
    pub live_sets: BTreeSet<u32>,
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
    /// No version is on a disc yet, so every `live_sets` is empty.
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
                            live_sets: BTreeSet::new(),
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
                live_sets: Default::default(),
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

    #[test]
    fn a_disc_name_carries_its_set_letter_only_when_copies_exceed_one() {
        assert_eq!(DiscKey::new(3, 0).name(1), "3");
        assert_eq!(DiscKey::new(3, 0).name(2), "3A");
        assert_eq!(DiscKey::new(3, 1).name(2), "3B");
        assert_eq!(DiscKey::new(12, 25).name(26), "12Z");
    }

    #[test]
    fn disc_keys_order_by_ordinal_then_set() {
        let mut keys = vec![DiscKey::new(2, 0), DiscKey::new(1, 1), DiscKey::new(1, 0)];
        keys.sort();
        assert_eq!(
            keys,
            vec![DiscKey::new(1, 0), DiscKey::new(1, 1), DiscKey::new(2, 0)]
        );
    }

    #[test]
    fn a_disc_argument_parses_with_or_without_a_set_letter() {
        assert_eq!(
            "3".parse::<DiscArg>(),
            Ok(DiscArg {
                ordinal: 3,
                copy_set: None
            })
        );
        assert_eq!(
            "3B".parse::<DiscArg>(),
            Ok(DiscArg {
                ordinal: 3,
                copy_set: Some(1)
            })
        );
        assert_eq!(
            "3b".parse::<DiscArg>(),
            Ok(DiscArg {
                ordinal: 3,
                copy_set: Some(1)
            })
        );
        // A bare negative ordinal reaches the domain (exit 2), as today.
        assert_eq!(
            "-1".parse::<DiscArg>(),
            Ok(DiscArg {
                ordinal: -1,
                copy_set: None
            })
        );
    }

    #[test]
    fn a_malformed_disc_argument_is_refused() {
        for bad in ["", "B", "B3", "3BB", "-1B", "0B", "3 B", "3é"] {
            assert!(bad.parse::<DiscArg>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_disc_argument_displays_as_typed_but_upper_case() {
        assert_eq!("3b".parse::<DiscArg>().unwrap().to_string(), "3B");
        assert_eq!("-1".parse::<DiscArg>().unwrap().to_string(), "-1");
    }

    #[test]
    fn a_disc_argument_resolves_only_inside_the_projects_sets() {
        let arg = |s: &str| s.parse::<DiscArg>().unwrap();
        assert_eq!(arg("3").resolve(1), Some(DiscKey::new(3, 0)));
        assert_eq!(arg("3A").resolve(1), Some(DiscKey::new(3, 0)));
        assert_eq!(arg("3B").resolve(1), None);
        assert_eq!(arg("3B").resolve(2), Some(DiscKey::new(3, 1)));
        assert_eq!(arg("-1").resolve(2), None);
        assert_eq!(arg("0").resolve(2), None);
    }

    #[test]
    fn set_letters_run_from_a_to_z() {
        assert_eq!(set_letter(0), 'A');
        assert_eq!(set_letter(25), 'Z');
        assert_eq!(MAX_COPIES, 26);
    }
}
