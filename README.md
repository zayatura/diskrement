# diskrement

**Incremental backups to optical discs, with a memory.**

---

## The problem

Optical media — Blu-ray in particular — is one of the most durable long-term storage
formats available to individuals. It doesn't need power, it can't be silently corrupted
by ransomware, and a pressed disc can outlast most hard drives by decades. For archiving
data you want to keep forever, it's a compelling choice.

But burning files to discs is painful in a way that hard-drive backups are not.
Hard drives are random-access: tools can compare what's on the drive to what's in
your source tree and copy only what changed. Optical media is write-once. Once a
session is burned, it cannot be modified. New sessions can be appended to a
multisession disc, but nothing already written can be touched.

Most burning tools treat this as a constraint to work around rather than a property
to embrace. They give you a "file manager" view of a disc and let you drag in files.
They have no memory of what you've already burned. If you want to back up a directory
tree incrementally across many discs over time — burning only what changed since
the last run, topping off partly-used discs before opening new ones, knowing which
discs hold which files — you're on your own.

`diskrement` (disk + incremental) is built around that workflow.

---

## What it does

Given a source directory and a catalog database, `diskrement`:

1. **Scans** the source tree and detects what has been added, modified, or deleted
   since the last burn.
2. **Plans** a burn: tops off the current open (multisession) disc first, then spills
   new data onto as many new discs as needed.
3. **Burns** each disc and commits the result to the catalog atomically. A crash
   before the commit leaves the catalog unchanged; the staged plan is the recovery unit.
4. **Verifies** burned discs by re-hashing the staged data against the catalog.
5. **Tracks history**: every version of every file is retained forever — the catalog
   is an append-only log. Because optical media can never delete what's already
   burned, the catalog reflects that reality.

Each disc is also **self-describing**: it carries a manifest and a full snapshot of
the catalog as it stood at burn time. A disc can be understood and the catalog
rebuilt from a single disc alone, without the host database.

---

## Design

`diskrement` follows a **functional core / imperative shell** architecture (sometimes
called hexagonal or ports-and-adapters):

- **`crates/diskrement-core/`** — pure Rust with no I/O. `diff` computes a change
  set between the last burned state and a fresh scan; `planner` packs it onto
  discs; `manifest` builds what gets written. One dependency, `chrono` with
  `default-features = false`, which excludes the `clock` feature — `Utc::now()`
  does not compile here; purity is enforced by the dependency graph, not by
  discipline.

- **`crates/diskrement-shell/`** — every effect: the SQLite catalog, the
  scanner and hasher, the CoW snapshot, staging, and the burners.

- **`crates/diskrement-cli/`** — the `diskrement` binary: argument parsing,
  path resolution, rendering and the exit-code contract.

The write-once nature of optical media is treated as a first principle, not an
afterthought. The append-only catalog, the commit-after-success burn, the
self-describing discs — all of these follow directly from "you can never undo a burn."

**Tech stack:** Rust (stable, edition 2021), `clap` 4, `rusqlite` (bundled
SQLite), `serde`/`serde_json`, `sha2`, `rayon`, `walkdir`, `reflink-copy`.

---

## Status

`diskrement` is under active development. **Cycle 1 is complete:**

| Component | Status |
|-----------|--------|
| Functional core (Diff, Planner, Manifest) | Done |
| SQLite catalog with append-only schema and migrations | Done |
| File scanning, SHA-256 hashing, CoW source snapshots | Done |
| Disc staging, `/.diskrement` layout, simulated burner | Done |
| Pipeline orchestration (all 9 operations) | Done |
| CLI adapter (argument parsing, prompts, output formatting) | Done |
| Real disc burning (xorriso, BD-R multisession) | Done (burn + read-back verify) |
| Coaster workflow (mark-dead + re-plan) | Done (lost-history report + re-plan) |

The pipeline orchestration layer is fully tested with integration tests against
real temp directories and a full scan → plan → burn → verify → history loop. The
simulated burner stages disc contents to directories rather than writing to physical
media — this makes the entire workflow testable without hardware.

With the CLI adapter (Plan 3b) in place, the tool is runnable end-to-end with
simulated burns:

```
diskrement init photos --source /srv/photos --work-dir /srv/work
diskrement scan photos
diskrement plan photos
diskrement burn photos          # asks for confirmation; --yes to skip
diskrement status photos
diskrement verify photos
diskrement history photos some/file.jpg
diskrement disc mark-dead photos 1   # write off a destroyed disc; --yes to skip
```

The per-project catalog defaults to `$XDG_DATA_HOME/diskrement/<name>.sqlite`
(or systemd's `$STATE_DIRECTORY`), overridable with `--catalog`.

### Real burning

The default backend burns real media via `xorriso` (ISO 9660 level 3 + Rock Ridge,
multisession). Select the device and backend with `--device /dev/sr0` /
`--burner real|simulated` (env: `DISKREMENT_DEVICE`, `DISKREMENT_BURNER`).
Your user must be able to open the drive — on most distros that means membership
in the `optical` group. `verify` mounts the disc via `udisks2` and re-hashes every
file, including versions superseded by later sessions on the same disc.

### Dead discs (coasters)

A burned disc that is later lost, destroyed, or unreadable can be written off:

```
diskrement disc mark-dead photos 1
```

This records an append-only `MARKED_DEAD` event — the disc row itself is left untouched — and
prints a report of the casualties: versions that lived only on that disc and are no
longer their path's current version are **permanently lost**; the ones still current are
listed for re-burn, and the next `scan` / `plan` / `burn` writes them to fresh media.
A dead disc is excluded from `verify`, is never topped off again, and shows as `DEAD`
in `status`.

`history` lists one row per burned copy, so after the recovery burn every row on the
dead disc reads `LOST` — including the one holding content you still have. `LOST` means
*those bytes* are gone, not that the content is unrecoverable: a file that was
superseded once on the dead disc ends up reading

```
b.txt
  v1  disc 1 / session 1   ...  LOST
  v2  disc 1 / session 2   ...  LOST
  v3  disc 2 / session 1   ...  CURRENT
```

where v2 and v3 carry the same content hash — the live copy came back on fresh media.
Between the mark-dead and that recovery burn, `status` keeps counting the re-burn list
under `Current files` — `history` still calls those files `CURRENT` — and names them on
a line of their own:

```
No live copy: 2 files (0.0 GB) — re-burn on next scan/plan/burn.
```

With more than one copy, those files also count as missing from every copy set, so no
set reads `complete` while a file has no live copy anywhere; `burn` prints the same line
and does not claim the archive is up to date.

If a plan was already staged against the disc when it was marked dead, `burn` refuses
it — re-run `plan`, which is the whole recovery: the staged scan and the source snapshot
both survive, and `plan` re-reads the disc states and targets fresh media. `reset` is
not part of the fix and throws away staged state you would only have to rebuild (with
the simulated backend, including the staging trees `verify` reads back). Likewise,
`verify --disc <n>` naming a dead or unknown disc is an error rather than a quiet
"nothing to verify".

A real burn deletes each disc's staging tree (`<work-dir>/staging/disc-<name>`) as soon
as that disc is committed. The burned disc and the catalog are the record, and a kept
tree would pin the space of source files you have since deleted, through its reflinks.
The simulated backend keeps its trees, because they are what `verify` reads back. Trees
left by older versions of diskrement stay until you remove them. On a real-backend
project with nothing staged, for example right after a burn, `reset <name>` does that
safely. `btrfs filesystem du -s <work-dir>/staging/*` shows how much space they hold
exclusively.

### Performance

`scan` hashes new or changed content with up to 6 workers in parallel; override with
`DISKREMENT_HASH_PARALLELISM` (a positive integer, clamped to 64 — an unusable
non-empty value falls back to 6 with a warning; an empty value is treated as unset). Long hashing or staging phases report progress on
**stderr** (a refreshed line on a TTY, a periodic line otherwise), so `stdout` stays
clean for data; diagnostics and warnings go to stderr too.

`scan` also keeps a fingerprint cache: a sidecar SQLite file next to the catalog,
named `<catalog-stem>.<project>.scancache.sqlite` (with the default catalog location
that reads a little oddly, e.g. `photos.photos.scancache.sqlite` — deliberate, since
one catalog file can hold several projects and the cache must not be shared between
them). The first scan of a large library is the expensive one; later scans hash only
what changed, and the scan output reports the split, e.g. `Hashed 12 of 75618 files
(75606 fingerprints reused).` (omitted when a previously staged scan is reused, since
no scan ran — `scan --force` rescans instead of reusing it). The cache is disposable — deleting it is always safe and merely costs
one full re-hash on the next scan. It is never embedded on a disc and `reset` does
not touch it. `scan --verify` re-hashes everything regardless of the cache; this is
also the escape hatch for the cache's one real caveat: it extends the same size+mtime
trust burned files already get to *scanned* files, so an in-place edit that preserves
both size and mtime goes unnoticed where an uncached scan would have re-hashed it.

`burn` stages the disc tree with copy-on-write reflink clones where the filesystem
supports them (the btrfs/XFS/bcachefs `init` already requires), falling back to a
plain copy otherwise, then re-hashes the staged tree as the integrity gate before
burning.

`scripts/perf-bench.sh SOURCE_DIR WORK_AREA` runs a full init → scan → plan → burn →
scan cycle against a throwaway project and reports per-step wall time alongside I/O
(`rchar`/`read_bytes`/`write_bytes`) from `/proc/<pid>/io`, so a rescan's savings are
visible directly. Set `DROP_CACHES=1` to drop the page cache before the first scan for
cold-cache comparability. `SOURCE_DIR` and `WORK_AREA` must be on the same filesystem,
since the benchmark's staging step relies on the same reflink support `burn` does.

---

## Building

**Requirements:** a stable Rust toolchain (pinned by `rust-toolchain.toml`),
Linux, and a CoW filesystem (btrfs, XFS with reflink, or bcachefs) for the
work area. `rusqlite`'s bundled SQLite is compiled from source, so a C
compiler is needed for the first build.

```
git clone https://github.com/zayatura/diskrement
cd diskrement
cargo build --release          # -> target/release/diskrement
cargo test --workspace         # all tests
cargo run -p diskrement-cli -- scan photos
```

The test suite includes integration tests that auto-skip on filesystems without
copy-on-write support.

### Producing an executable

```bash
cargo build --release
```

This produces `target/release/diskrement`, a single self-contained binary —
no launcher script, and no JVM on the target machine. You can run it directly:

```bash
target/release/diskrement --help
```

Or put it on your `PATH` permanently with a symlink:

```bash
ln -s "$PWD/target/release/diskrement" ~/.local/bin/diskrement
```

---

## License

GPL-3.0. See [LICENSE](LICENSE).
