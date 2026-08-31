# diskrement Scan Performance Design

**Date:** 2026-08-31
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§5.3 hashing, §6 pipeline)
**Predecessors:** Cycles 1–2 through Plan 4a (real burning) — done and merged. Plan 4b (coaster workflow) is written but not executed; this cycle is independent of it (no catalog schema change).

---

## 1. Motivation: measured baseline

Reproduced 2026-08-31 against a real 413.6 GB / 75,618-file photo library (avg 5.5 MB/file)
on NVMe, btrfs `zstd:3`, i5-12500 (12 threads, SHA-NI), simulated burner:

| step | wall time | behavior |
|---|---|---|
| `scan` #1 (fresh project) | **450 s** | hashed all 413.6 GB sequentially, ~920 MB/s, 1 core |
| `plan` | 4.9 s | disc 1 = 24.5 GB / 14,227 files (6% of what scan hashed) |
| `burn` (simulated) | 38 s | byte-copied 24.5 GB to staging + re-read it all to re-hash |
| `scan` #2 (after burn) | **410 s** | re-read 388.6 GB — everything except the burned disc |

Causes: (a) the size+mtime→hash fast path (`CatalogStore.fingerprints`) is derived from
burn-committed rows only, so unburned files are re-hashed every cycle — filling this
library across ~18 discs would hash ~3.7 TB; (b) hashing is single-threaded
(`ZIO.foreach` in `FileScannerLive`); (c) `Stager.copyOne` byte-copies on a CoW
filesystem where `init` already guarantees reflink support; (d) scan and burn emit no
output until they finish. Measured non-issues: reflink snapshot clone (~1 s), walk/stat,
staged-scan JSON, `Diff`, `Planner`, SQLite ops.

## 2. Scope

**In scope (two phases, one plan):**

- **P1 — parallel hashing:** bounded-parallel SHA-256 in the scanner and in staging
  re-hash. Target: scan ~3–4× faster.
- **P1 — progress reporting:** live progress (bytes, files, rate, ETA) on stderr during
  the scan hash phase and burn staging, via a `ProgressSink` port.
- **P1 — reflink staging:** `Stager` clones instead of copies (with plain-copy
  fallback); the integrity re-hash stays.
- **P2 — observed-fingerprint cache:** a sidecar SQLite cache of size+mtime→hash for
  *scanned* (not just burned) files, so every scan after the first hashes only new or
  changed files.
- **Validation:** a committed benchmark script plus a final run against the real library
  compared to the baseline above.

**Out of scope (deferred):** lazy hashing (stat-only scan, hash-at-burn — "P3"; decide
after this cycle if still needed); progress for `verify`; any change to `Diff`,
`Planner`, or the append-only catalog schema; parallelizing the burn itself.

## 3. Architecture & key decisions

All changes live in `diskrement.shell` (fs, burn, pipeline) and `diskrement.cli`. The
functional core is untouched.

1. **The catalog stays pure history.** The fingerprint cache is a **sidecar database**
   (`<catalog-stem>.<project>.scancache.sqlite` next to the catalog — see §4.4), never
   embedded on discs, exempt from
   append-only rules because it is disposable: deleting it costs one re-hash, nothing
   else. Rejected alternatives: an upsert table inside the catalog (mutable state in an
   archival, disc-embedded DB) and an append-only observation log (unbounded growth,
   window-query reads).
2. **Fingerprint semantics are unchanged; only their source widens.** The scanner's
   fast path (reuse hash when size+mtime match, unless `--verify`) already exists and is
   proven. `freshScan`'s `prior` map becomes `catalog.fingerprints ++ cache.readAll`
   — the cache wins on conflict because it is fresher (e.g. it re-fingerprints a
   touched-but-identical file after one re-hash, so later scans fast-path it).
3. **Cache write-back is total, not incremental.** After every successful scan —
   including `--verify`, which still re-hashes everything — all scanned files'
   fingerprints are upserted in one transaction (~75k rows, sub-second). No bookkeeping
   about which files were re-hashed.
4. **Parallelism is bounded and deterministic.** `ZIO.foreachPar` + `withParallelism(N)`;
   N defaults to 6, overridable via `DISKREMENT_HASH_PARALLELISM` (read through
   `zio.System`; documented in README, unlike the test-only `DISKREMENT_DISC_BYTES`).
   Output is sorted by `relPath` after hashing (as today), so results are
   order-independent. Error contract unchanged: first `FileFailed` fails the scan.
5. **Pipeline stays I/O-free toward the operator.** Progress flows through a
   `ProgressSink` service (a port); the CLI provides the stderr renderer, tests and
   library use a no-op. `Pipeline` still *returns* data; it just reports progress
   through the sink while working.
6. **Staging clones batched, verifies as before.** `cp --reflink=always --parents -p`
   from the snapshot root in batches (~500 paths), falling back to the existing
   per-file `Files.copy` path for the remainder of the run if cloning is unsupported
   (ITs on tmpfs). The staged tree is still re-hashed against the scan hashes — that
   check is the point of staging — now in parallel and with progress. The real burner
   is otherwise unaffected: xorriso masters the same staged tree.

## 4. Components

### 4.1 `FileScannerLive` (changed)

Two explicit phases:

1. **walk + stat** every file (as today) — this also yields `totalBytes`/`totalFiles`
   for progress.
2. **hash phase:** partition into fast-path hits (fingerprint match) and misses; hash
   the misses with `ZIO.foreachPar` (bounded), reporting progress per completed file.

Signature gains nothing; `FileScanner.live` layer now also requires `ProgressSink`.

### 4.2 `ProgressSink` (new port, `diskrement.shell`)

```scala
trait ProgressSink:
  def start(label: String, totalBytes: Long, totalFiles: Int): UIO[Unit]
  def advance(deltaBytes: Long, deltaFiles: Int): UIO[Unit]  // called per completed file
  def done: UIO[Unit]
  // Consumers wrap each phase in this bracket: start before the work, done on
  // ANY exit (success, failure, interrupt). [Added during Task 2 review.]
  final def phase[R, E, A](label: String, totalBytes: Long, totalFiles: Int)(
      work: ZIO[R, E, A]): ZIO[R, E, A]
object ProgressSink:
  val noop: ZLayer[Any, Nothing, ProgressSink]
```

CLI implementation (`diskrement.cli`): TTY → one carriage-return-refreshed stderr line,
throttled to ~1/s (`hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m12s`);
non-TTY → a full line every ~10 s. Internal state via `Ref`; `advance` is cheap and
thread-safe (called from parallel fibers).

### 4.3 `Stager` (changed)

- Copy phase: batched reflink clone (decision 6), single fallback switch to
  `Files.copy` on first "clone unsupported" failure. Per-file existence pre-check kept
  (missing source still maps to `BurnError.SourceFileMissing`).
- Hash phase: parallel re-hash of staged files with progress (`label = "staging"`),
  same mismatch → `BurnError.HashMismatch` semantics.
- `-p` preserves mtimes so mastered disc metadata is unchanged from today's `cp -a`
  snapshot heritage.

### 4.4 `FingerprintCache` (new port + SQLite impl, `diskrement.shell.fs`)

```scala
trait FingerprintCache:
  def readAll: IO[CacheError, Map[RelPath, FileFingerprint]]
  def putAll(entries: Iterable[(RelPath, FileFingerprint)]): IO[CacheError, Unit]
```

- File: `<catalog-stem>.<project>.scancache.sqlite` in the catalog's directory, path
  derived in `PathResolver` and passed to the layer. The table is keyed by `rel_path`
  alone, so the *filename* carries the project scoping: a single catalog file can hold
  several projects, and sharing one cache between them would let one project's
  fingerprints shadow another's. [Amended during Task 5 review — the original text
  assumed one project per catalog file and omitted the project from the name, which a
  reviewer reproduced as cross-project cache poisoning: the resulting burn fails the
  staging re-hash (safe — nothing is archived or committed) but `scan --force` cannot
  clear it, only `scan --verify` can.]
- Schema: `fingerprint(rel_path TEXT PRIMARY KEY, size_bytes INTEGER, mtime TEXT,
  content_hash TEXT, updated_at TEXT)`; `PRAGMA user_version = 1`. Created on demand;
  a future incompatible version or a corrupt/unreadable file is **warned about,
  deleted, and recreated empty** — never fatal to a scan.
- Implementation is raw JDBC with a per-call connection (WAL, busy_timeout,
  transactional batched upsert) rather than the catalog's `Db` pool: the
  wipe-and-recreate recovery must delete and reopen the file, which a live
  pooled connection would fight, and two connections per scan are negligible.
  Only genuine corruption (NOTADB/CORRUPT/MALFORMED, or a schema-version
  mismatch) triggers the wipe; transient failures (locked, disk full) surface
  as warnings without destroying a healthy cache. [Amended during Task 1
  review — originally said "reuses the `Db` pool helpers".]
- Lifecycle: `reset` does **not** touch it (it is not staged state); it survives
  arbitrarily and may reference deleted paths (harmless — `prior` lookups are by
  scanned path only).

### 4.5 `PipelineLive.freshScan` (changed)

```
prior   = catalog.fingerprints ++ cache.readAll        // cache wins
scanned = scanner.scan(handle.root, prior, verify)
_       = cache.putAll(all scanned fingerprints)        // total write-back
```

Cache read/write failures degrade, never fail: an unreadable cache yields an empty
prior map, a failed write-back is skipped; the adapter logs the warning (with the
path) and the scan itself succeeds either way. Those logs go to **stderr** — ZIO's
default logger writes to stdout, which is this CLI's data channel, so `Main`
installs a stderr logger via `bootstrap`.

`ScanResult` gains `hashed: Option[Int]` (files actually hashed, i.e. fast-path
misses) and `scannedFiles: Int` (the denominator); `Render.scan` reports them on a
second line (`Hashed 61391 of 75618 files (14227 fingerprints reused).`) — the
user-visible evidence of cache effectiveness that the ITs assert on. `hashed` is
`None` when a staged scan is reused, because no scan ran and nothing was hashed *or*
reused; the second line is then omitted rather than claiming a misleading zero.
[`Option`/`scannedFiles` added during Task 5 review — the original `hashed: Int`
reported "0 hashed, N reused" on the staged-reuse path, which is untrue and would
have let a warm-rescan IT pass for the wrong reason.]

### 4.6 Benchmark script (new, `scripts/perf-bench.sh`)

Bash; takes `SOURCE_DIR` and `WORK_AREA`; runs init → scan → plan → burn `--yes`
(simulated) → scan on a throwaway project/catalog under `WORK_AREA`; reports per-step
wall time and JVM cumulative `read_bytes` (from `/proc/<pid>/io`); cleans up with
`reset` and deletes its throwaway catalog+cache. Read-only toward `SOURCE_DIR`.

## 5. Error model

- `ScanError` unchanged (fail-fast on first file failure, now possibly from a parallel
  fiber — `foreachPar` interrupts siblings).
- New `CacheError` stays internal to the shell; the pipeline converts cache failures to
  `ZIO.logWarning`, never a failed exit.
- `Stager` errors unchanged (`SourceFileMissing`, `HashMismatch`, `BurnIoError`); a
  reflink batch failure that is not "unsupported" maps to `BurnIoError`.

## 6. Testing strategy

- **Unit (shell):** parallel scanner — same results as sequential on a fixture tree,
  fast-path honored, `--verify` bypasses it, failure interrupts and surfaces
  `FileFailed`; counting fake `Hasher` asserts *which* files were hashed (warm cache ⇒
  only changed/new). `FingerprintCacheLive` spec on a temp file: create-on-demand,
  round-trip, corrupt-file recovery, version stamp. `Stager`: reflink fallback on a
  non-CoW dir (tmpfs), mismatch and missing-source semantics preserved. `ProgressSink`
  noop in all existing specs (layer addition only).
- **CLI/IT:** existing 13 IT scenarios stay green unchanged (simulated backend pinned);
  one new scenario: scan → burn → touch nothing → rescan, asserting `hashed = 0` (and
  after touching one file, `hashed = 1`) via the new scan output field; progress output
  suppressed when stderr is not a TTY except the ~10 s heartbeat (assert it does not
  corrupt stdout parsing).
- **No wall-time assertions in CI.**

## 7. Validation (acceptance)

Run `scripts/perf-bench.sh` against the real photo library (read-only), work area on the
same filesystem, and compare with the §1 baseline. Targets on the reference machine:

| metric | baseline | target |
|---|---|---|
| scan #1 (fresh project) | 450 s | **≤ 180 s** |
| scan after burning disc 1 | 410 s | **≤ 30 s** |
| burn staging (24.5 GB, simulated) | 38 s | **≤ 25 s** |
| rescan bytes read | 388.6 GB | ~0 (stat-only) |

Correctness cross-check during the run: scan #2 file counts identical to baseline
(61,391 added / 14,227 unchanged at the same point), and a spot-check of ~10 staged file
hashes against `sha256sum`. Results are recorded in the plan's final summary.

## 8. Delivery

One plan, branch `feat/scan-performance` off `main`, subagent-driven (fresh implementer
per task, spec-then-quality review), merged into `main` when green. Suggested task
order: FingerprintCache port+impl → scanner parallelism + cache wiring in pipeline →
ProgressSink + CLI renderer → Stager reflink+parallel re-hash → bench script + IT
scenario → README → real-library validation → merge. Independent of plan 4b; whichever
merges second rebases trivially (no shared schema).
