# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test

Rust workspace. Three crates with a strict dependency chain: `diskrement-cli` → `diskrement-shell` → `diskrement-core`.

```
cargo test --workspace                       # all tests
cargo test -p diskrement-core                # pure core, incl. proptest suites
cargo test -p diskrement-shell               # catalog, fs, burn; reads fixtures/
cargo test -p diskrement-cli                 # CLI contract + integration scenarios
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --release                        # -> target/release/diskrement
./scripts/perf-bench.sh SRC WORK             # perf benchmark (DROP_CACHES=1 for cold cache)
```

Single test: `cargo test -p diskrement-shell catalog::store`. Warnings are failures via `.cargo/config.toml` — enforced, not merely by convention.

The differential harness that used to compare against the Scala binary was deleted with the Scala tree at P10, and the byte/row comparisons against `fixtures/` were retired with multiple copies (schema V3). `fixtures/` is now a frozen corpus of V2 catalogs and staged discs, used only as input; it cannot be regenerated from `main`.

## What "verified" means

`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` all pass, with no unexplained drop in test count. Do not claim green without running them.

Caveat: the ported integration scenarios in `crates/diskrement-cli/tests/scenarios/` need a CoW `TMPDIR` and print `SKIPPED` without one; `DISKREMENT_SCENARIOS_REQUIRED=1` turns that into a failure, and any run used as evidence must set it. `crates/diskrement-shell/tests/xorriso_image.rs` skips when `xorriso` isn't installed. A green run may mean "skipped"; check the reported counts.

## Gotchas that will bite

- **Fixture databases must be opened `?immutable=1`** (with
  `OpenFlags::SQLITE_OPEN_URI`). A plain read-only open of a WAL database
  creates `-wal`/`-shm` files beside it, dirtying `fixtures/`.
- **SQLite pragmas need a real file.** An in-memory database always reports
  `journal_mode=memory`, so a WAL assertion against one passes vacuously.
- **Never open a fixture catalog with `Catalog::open`** — it runs the migration
  runner and writes. Copy the file to a `TempDir` first.
- **`ioctl(FICLONE)` does not preserve mtime.** A reflinked copy gets the
  current time; `cp -a` preserved the source's. Any Rust code that clones a
  file must copy the mtime afterwards (`File::set_modified`), or the
  scanner's size+mtime fast path never hits and the fingerprint cache is
  useless. `fs::snapshot::reflink_tree` does this — keep it that way.
- **xorriso writes everything to stderr, including its `UPDATE` progress.**
  `std::process` has no `redirectErrorStream`, so `burn::cmd::system` gives
  the child one `std::io::pipe()` for both stdout and stderr and reads it on
  the calling thread: every line reaches `on_line` live, in the child's
  order. Never split the streams again: deferring stderr is what left real
  burns silent for ~25 minutes per disc (#14). Lines are decoded lossily:
  never go back to `BufRead::lines`, which stops at the first non-UTF-8 line
  and leaves the pipe undrained, so a chatty child blocks forever.
- **Burn progress comes from `Writing:` percentages.** `parse::writing_per_mille`
  reads them and the burner scales them to the staged image size for a
  bytes-only `ProgressSink` phase. Writing ends at 100% or at libburn's
  `Closing track/session` line; the close that follows prints no percentages
  (about 11 s for a sealed BD-R), so the operator is told once.
- **Verify progress advances within a file.** At ~13 MB/s from an optical
  drive, a per-file bar stands still for minutes on one large video, so both
  verifiers hash through `hasher::hash_with` and advance by bytes as they read.
  Scan and staging still advance per file; that is fine from NVMe. Tier 2's
  extractions are opaque to us and advance one file at a time.
- **A real burn deletes each disc's staging tree right after its commit**
  (`Burner::retains_staging`, `discard_staging` in `pipeline/mod.rs`); the simulated
  backend keeps them because `StagingVerifier` reads them. This is safe only because
  every disc, twins included, is staged from `work_dir/snapshot`: never stage a disc
  from another disc's tree, and never hardlink into staging.
- **The pipeline ejects, not the burner.** `XorrisoBurner::burn` leaves the disc
  in the drive; `burn_disc` calls `Burner::eject` after the commit, or under
  `burn --verify` after the read-back (`pipeline/read_back.rs`). The read-back
  happens before the commit, against expected contents read from the post-burn
  work copy (`disc_expectation`), and a failed disc is never recorded.
- **The kernel cannot mount a disc it has just burned.** Closing a session raises
  no media-change event, so until an eject `/sys/block/sr0/size` keeps the blank
  disc's 2048 bytes and `udisksctl mount` fails ("not a mountable filesystem");
  udisks `Block.Rescan` and a STOP UNIT spin-down do not help (validated
  2026-10-16, Pioneer BDR-UD04, kernel 6.18). So `burn --verify`'s
  `DiscVerifier::read_back` falls back to extracting `/data` through xorriso
  (SCSI pass-through, which sees the new session) into `<work_dir>/readback`,
  removed before and after. An extraction failure is `Unverifiable` (reinsert),
  never `Failed`. Plain `verify` stays mount-only: it is the check through the
  kernel's ISO 9660 driver, the path a restore takes.
- **A read-back trusts the mount only if it shows the new session.** A mount
  that still shows the previous session (the same staleness, on an appended
  disc) is compared via the mounted `/.diskrement/project.json` and treated
  like a failed mount, never `Failed`. Otherwise a good appended disc would
  read as missing its new files.
- **xorriso extracts over an existing file** (validated 2026-09-07, 1.5.8.pl02),
  but every extraction in this tree still targets a fresh path inside a
  `TempDir`, so nothing depends on that.
- **`-toc -tell_media_space` prints `Media status` twice**, once per
  sub-command. The parser takes the first match; do not "fix" it into an
  exactly-once assertion.
- **`stdio:` image files are not real media.** `-close on` has no observable
  effect on them and free space is a fake huge number, so sealing and capacity
  refusals are assertable only on real media (`docs/smoke-test-real-media.md`).
- **The real backend prompts before it opens the device.** That is what lets
  `tests/binary.rs` exercise it with no drive: a child with no stdin declines,
  and the burn aborts (exit 0) before anything is touched.
- **Disc names depend on `copies`.** `DiscKey::name(copies)` is `3` in a single-copy project and `3B` otherwise; labels, staging dirs (`staging/disc-3B`), volids, prompts and reports all go through it. Never format a disc ordinal by hand.
- **Fill order is `FillOrder::key`, not `rel_path`.** A rooted project (`init --root
  NAME=PATH`) fills discs by *(path below the root, root name)*, so its libraries
  interleave by date (Immich's `YYYY/YYYY-MM-DD/` layout). Anything that orders files
  for placement goes through the project's `FillOrder::from(&sources)`, the same one
  for every copy set. Reports (`oversized`, `unplaceable_catch_up`) stay in `rel_path`
  order.

## Environment

- `DISKREMENT_DEVICE`, `DISKREMENT_BURNER` (`real`|`simulated`), `DISKREMENT_HASH_PARALLELISM` (default 6, clamped to 64).
- `DISKREMENT_DISC_BYTES` is test-only and **may only shrink** usable capacity; it is deliberately undocumented in `--help`.
- Catalog path precedence (`crates/diskrement-cli/src/paths.rs` `resolve`): `--catalog` > `--data-dir` > `$STATE_DIRECTORY` > `$XDG_DATA_HOME/diskrement`.
- Linux only. Real burning requires membership in the `optical` group. Manual hardware checklist: `docs/smoke-test-real-media.md`.
- Toolchain: `rust-toolchain.toml` pins an exact release (warnings are errors, so a floating channel breaks the build on every new lint). The pin is honoured only by rustup; a distro `cargo` (e.g. Arch's `rust` package) ignores it, so after a system Rust upgrade a new lint failure is a toolchain bump, fixed in its own `chore:` commit together with the pin.

## Invariants — do not "fix" these

- **The catalog history is append-only.** `file_version`, `placement`, `change_record`, `disc_event` and `lost_version` rows are never updated or deleted; the `file_version`-only-grows proptest in `crates/diskrement-shell/src/catalog/store.rs` pins it. The only in-place writes are two forward-only status transitions, each guarded in its `where` clause: `disc.status` OPEN→FINALIZED and `session.state` BURNED→VERIFIED. There are no triggers — the schema bounds the values (check constraints), not the mutations; don't add a third kind of update.
- **`DEAD` is derived, never stored.** `disc.status` keeps its `check (status in ('OPEN','FINALIZED'))` constraint; dead-ness is computed from `disc_event`. A test asserting only the derived value would pass against an illegal update-in-place, so also assert the row itself is unchanged.
- **Commit after success.** No `PLANNED` rows are persisted; the staged plan in the work dir is the recovery unit. A crash before commit leaves the catalog untouched.
- **One disc per copy set per cycle.** Each plan→burn writes at most one disc/session per copy set (`copies` sets, shown as letters: 1A/1B). `burn` writes the sets in order and commits each disc as soon as it is burned; the staged plan is rewritten (atomically) to what is left after every commit, so a failure after the first commit is a `PartialBurn` naming the committed, unrecorded and unwritten discs. Its advice follows the cause (`error::partial_burn_tail`): usually "re-run burn", which resumes; "re-run plan, then burn" for a stale plan or a missing snapshot; never "reset". `plan --finalize` seals every set's open disc.
- **Deletions ride the next burned disc.** A cycle with nothing to place plans no disc at all (any `copies`), and `burn` writes nothing; its tombstones are re-derived by `diff::tombstones` (against `fingerprints`, not the live view) on every plan until a disc that is burned anyway records them. Never fall back to a disc just to carry tombstones (that opened untwinned discs, #9), and never add a sessionless deletion record. `burn` refuses a planned disc with nothing to write that is new or does not seal (what a pre-#9 plan's fallback looks like) as a `StalePlan`. Meanwhile `plan` writes those deletions to `<work_dir>/pending-deletions.json` (beside `staged/`, which `burn` clears). `status`/`burn` coverage leaves them out and `mark-dead` records their dying last copy as LOST, not REBURN (#16). The file is advisory: a failed write only warns, and missing or unreadable falls back to the catalog-only view.
- **The embedded `catalog.sqlite` reflects post-burn state** (built via `apply_update_to` on a work copy) so each disc is self-describing. The earlier pre-burn approach was deliberately reversed; never file-swap the SQLite file under a live connection.
- **A rooted project is one tree from the snapshot on.** `plan` reflinks each root to `<work_dir>/snapshot/<name>`; scanning, the catalog, staging and both verifiers see ordinary `rel_path`s under the root names, and only the planner's fill order knows roots exist. Roots are written once, by `init` (`project_root`, schema V4; `project.source_root` holds `''`), and never changed: renaming one would read as deleting and re-adding every file under it. A `RootName` is permanent once burned, so it refuses control characters and components over 255 bytes (`NAME_MAX`) up front, and a non-UTF-8 `--root` path is refused at parse time (the catalog stores paths as TEXT). `OverlappingRoots` reports the paths as given, not canonicalized; a rooted clone's `NotCowCapable` names the root.
- **`LOST` is per physical copy, not per content hash.** After a recovery burn, two versions sharing a hash can legitimately read `LOST` and `CURRENT`.
- **A copy placement has no version of its own.** A disc that holds another copy of a current version records one `placement` (its `copy_index` is its disc's `copy_set`) and no `file_version` or `change_record`. Classification is done per disc at commit time against the live catalog, so 1B's files become copies of what 1A just recorded. A version with no live copy anywhere is still re-added as a new version (REBURN).
- **`StalePlan` advice is always "re-run plan", never "run reset"** — `reset` destroys staging that `StagingVerifier` needs. The `s07_reset` scenario asserts the message does not contain "reset".
- **`Pipeline` returns data and does no I/O**; the CLI resolves paths and renders.
- **Exit codes are a stable contract** (`crates/diskrement-shell/src/error.rs`, `PipelineError::exit_code`): 2 = state/lookup, 3 = environment/oversize (including source names that are not UTF-8 and overlapping roots), 4 = verify mismatch, 5 = unexpected, 6 = media/burner; an aborted burn exits 0. Pinned by that file's tests and `crates/diskrement-cli/tests/binary.rs`.

Known deferred debt, documented on purpose — don't casually "fix" it: `archive_pointers` has no dead flag and is already baked onto burned discs; `write_disc` doesn't refuse a dead disc; the fingerprint cache never prunes deleted paths.

## Repo etiquette

- Feature work goes on a branch off `main` named `feat/<topic>`, merged back with an explicit merge commit titled `Merge feat/<branch>: <summary>`, then the branch is deleted locally and on `origin`. No PRs. Never commit feature work directly to `main`.
- Conventional commits with a scope: `feat(catalog):`, `fix(cli):`, `test(catalog):`, `docs(plan):`, `chore:`.

## In-flight work

Specs and plans under `docs/superpowers/specs/` and `docs/superpowers/plans/` are the source of truth for what is being built and what was decided. Read the relevant plan before changing an area it covers; execution deviations are appended to the plan doc rather than left in commit messages.
