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

- **`core/`** — pure Scala 3 with no I/O. `Diff` computes a change set between the
  catalog's current view and a fresh scan. `Planner` assigns files to discs respecting
  per-disc capacity limits and the multisession top-off policy. `ManifestBuilder`
  constructs the per-disc manifest. No effect types; data in, data out.

- **`shell/`** — ZIO 2 effects wiring adapters to the core. The SQLite catalog
  (`CatalogStore`) is append-only by design, enforced at the schema level. A
  copy-on-write source snapshot (`SourceSnapshot`) is taken once per run to eliminate
  TOCTOU races. The `Pipeline` service sequences all eight operations (`init`, `scan`,
  `plan`, `burn`, `reset`, `status`, `verify`, `history`) and returns structured data
  that a CLI can render.

The write-once nature of optical media is treated as a first principle, not an
afterthought. The append-only catalog, the commit-after-success burn, the
self-describing discs — all of these follow directly from "you can never undo a burn."

**Tech stack:** Scala 3.3, ZIO 2.1, zio-jdbc 0.1, SQLite (xerial), zio-json 0.7.

---

## Status

`diskrement` is under active development. **Cycle 1 is complete:**

| Component | Status |
|-----------|--------|
| Functional core (Diff, Planner, Manifest) | Done |
| SQLite catalog with append-only schema and migrations | Done |
| File scanning, SHA-256 hashing, CoW source snapshots | Done |
| Disc staging, `/.diskrement` layout, simulated burner | Done |
| Pipeline orchestration (all 8 operations) | Done |
| CLI adapter (argument parsing, prompts, output formatting) | Done |
| Real disc burning (xorriso, BD-R multisession) | Done (burn + read-back verify) |
| Coaster workflow (mark-dead + re-plan) | Planned (Plan 4b) |

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

**Requirements:** JDK 17+, [sbt](https://www.scala-sbt.org/) 1.x, Linux (the CoW
snapshot support probes for `btrfs`/`xfs`/`bcachefs` reflinking).

```
git clone https://github.com/zayatura/diskrement
cd diskrement
sbt test
```

The test suite includes integration tests that auto-skip on filesystems without
copy-on-write support.

### Producing an executable

```bash
sbt cli/stage
```

This places a self-contained `diskrement` launch script and all its JARs under
`cli/target/universal/stage/`. You can run it directly:

```bash
cli/target/universal/stage/bin/diskrement --help
```

Or put it on your `PATH` permanently with a symlink:

```bash
ln -s "$PWD/cli/target/universal/stage/bin/diskrement" ~/.local/bin/diskrement
```

To produce a distributable zip archive instead:

```bash
sbt cli/universal:packageBin
# output: cli/target/universal/diskrement-0.1.0-SNAPSHOT.zip
```

During development you can also run without staging:

```bash
sbt "cli/run scan photos"
```

---

## License

GPL-3.0. See [LICENSE](LICENSE).
