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

`diskrement` is under active development. This is the state of Cycle 1:

| Component | Status |
|-----------|--------|
| Functional core (Diff, Planner, Manifest) | Done |
| SQLite catalog with append-only schema and migrations | Done |
| File scanning, SHA-256 hashing, CoW source snapshots | Done |
| Disc staging, `/.diskrement` layout, simulated burner | Done |
| Pipeline orchestration (all 8 operations) | Done |
| **CLI adapter** (argument parsing, prompts, output formatting) | **In progress** |
| Real disc burning (`xorriso` / `growisofs`) | Planned (Cycle 2) |

The pipeline orchestration layer is fully tested with integration tests against
real temp directories and a full scan → plan → burn → verify → history loop. The
simulated burner stages disc contents to directories rather than writing to physical
media — this makes the entire workflow testable without hardware.

The CLI wiring (Plan 3b) is the remaining piece of Cycle 1. Once it lands, the tool
will be runnable end-to-end with simulated burns.

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

---

## License

GPL-3.0. See [LICENSE](LICENSE).
