# diskrement-core

The functional core of diskrement: pure data and pure functions, no I/O.

- `model` — `RelPath`, `ContentHash`, `ScannedFile`, `CatalogEntry`,
  `CatalogView`, `DiscType`, `Capacity`
- `diff` — `ChangeSet` and `diff()`: classify a scan against the catalog's
  current view
- `planner` — `plan()`: ordered-fill disc packing, top off before spilling
- `manifest` — `build()`: the per-session manifest for one disc

This crate depends only on `chrono` with default features disabled, which
excludes the `clock` feature. `Utc::now()` does not compile here. Anything that
touches the filesystem, SQLite, or a subprocess belongs in `diskrement-shell`.

Two things in here are load-bearing beyond their size:

- **Wire spellings.** `DiscType::as_str` and `ManifestChangeKind::as_str` are
  frozen by the data-compatibility contract in
  `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` §3.1. The catalog's
  `check` constraints catch a wrong disc-type string; nothing catches a wrong
  manifest-kind string, so its round-trip test is the only guard. Do not change
  these strings.
- **`Bin::fits` in the planner** is written as an addition rather than a
  subtraction against `free_bytes()`. The two disagree on an overfull disc, and
  the addition is the one that matches the Scala. See
  `a_zero_byte_file_does_not_fit_an_overfull_disc`.
