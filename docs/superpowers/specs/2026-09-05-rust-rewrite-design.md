# diskrement — Rust Rewrite Design

**Date:** 2026-09-05
**Status:** Approved for planning
**Predecessors:** Cycles 1 and 2 (Plans 1–4b) — all done and merged to `main`.
**Supersedes:** nothing. The Scala implementation is preserved, not deleted, on branch `scala`.

---

## 1. Scope

Reimplement diskrement in Rust, replacing the Scala 3 / ZIO / sbt implementation
in its entirety. The rewrite is **data-compatible and CLI-compatible** with the
Scala version, but is otherwise free to be idiomatic Rust: it is not a
transliteration of the existing code structure.

**Motivation:** language preference. The Rust implementation is the one that will
be maintained going forward. Performance and deployment gains are welcome side
effects, not the goal — which matters, because it means no design decision here
should be justified by speed alone. Learning Rust is a real secondary motive,
which is why the phasing optimises for frequent demonstrable progress over
theoretical minimum work.

**In scope:**

- A Cargo workspace replacing the three sbt modules, preserving the strict
  `cli → shell → core` dependency chain.
- Full feature parity: all nine commands (`init`, `scan`, `plan`, `burn`,
  `reset`, `status`, `verify`, `history`, `disc mark-dead`), both burner
  backends, the fingerprint cache, CoW snapshots and staging, and the real
  xorriso path.
- Behavioral parity proven by porting the integration suite (10 numbered
  scenarios plus `ScenarioExitCodes`).
- Data compatibility with catalogs, staged plans, and burned discs produced by
  the Scala version, checked continuously against a golden fixture corpus
  harvested **before any Rust code is written** (§4).
- No regression on the scan/staging paths, measured against a baseline recorded
  from the Scala build at the same moment the fixtures are harvested.
- Documentation rewrite (`README.md`, `CLAUDE.md`) for the Rust build.

**Out of scope:**

- The known deferred debt documented in `CLAUDE.md` (`archivePointers` has no
  dead flag; `writeDisc` doesn't refuse a dead disc; the fingerprint cache never
  prunes deleted paths; `copies > 1` degrades silently). These are ported
  **as-is, bugs included**. Mixing behavior changes into a rewrite makes it
  impossible to tell a porting defect from an intended change. They remain open
  afterwards.
- Any redesign of the command surface, output wording aside.
- New features of any kind.

---

## 2. Alternatives considered

1. **Async port (tokio) mirroring ZIO's shape.** Conceptually closest to the
   existing effect model, but it fights the workload: hashing is CPU-bound, file
   reads and SQLite block, and xorriso is a subprocess. Every hot path would need
   `spawn_blocking` ceremony for no added throughput. **Rejected.** If learning
   async matters later, a scoped slice can adopt it without re-architecting.

2. **Strangler fig: port op-by-op behind one delegating binary.** Always
   shippable, but it requires both toolchains in-tree for the whole duration and
   produces throwaway glue. The fixture corpus plus the integration suite already
   provide safe increments at lower cost. **Rejected.**

3. **Mechanical transliteration of the Scala structure.** Would preserve every
   ZIO-shaped abstraction — eight injected services, `ZLayer` wiring, a
   `CmdRunner` seam — none of which earn their keep without an effect system.
   **Rejected**, but with a hard constraint attached: *behavior* is transliterated
   exactly even where *structure* is not (§3, §5).

---

## 3. Compatibility contract

Two things are frozen; everything else is free.

### 3.1 Data formats (hard requirement)

- **The SQLite catalog schema**, including the `schema_version` table, both
  migrations, and every `check` constraint. The DDL is transcribed verbatim
  rather than regenerated, because `status in ('OPEN','FINALIZED')` and
  `kind in ('MARKED_DEAD')` are load-bearing for the derived-`DEAD` invariant
  (§7).
- **The fingerprint cache sidecar**, including its filename convention
  `<catalog-stem>.<project>.scancache.sqlite` (`PathResolver.scala:43`).
- **The staged JSON** in the work dir: `staged/scan.json`, `staged/plan.json`
  (`StagedStore.scala:59-61`).
- **The on-disc `/.diskrement` layout**: `project.json`, `manifest.json`,
  `pointers.json`, and the embedded post-burn `catalog.sqlite`, alongside
  `data/` (`SnapshotWriter.scala:15-21`, `Stager.scala:24`).

An existing catalog and existing burned discs must keep working. A user
switching from the Scala binary to the Rust binary mid-project should not
notice.

Two format hazards deserve explicit attention, because both fail silently or
late. They are the highest-consequence risk in this project:

**Timestamps.** Every time-shaped value in the catalog and in the JSON is
`java.time.Instant.toString()`: ISO-8601, `Z`-suffixed, with *variable*
fractional precision — zero digits when the instant is whole-second, otherwise
3, 6, or 9. `chrono`'s `to_rfc3339_opts(SecondsFormat::AutoSi, true)` has
exactly that rule. **This is a claim to verify, not to assume.** A single
`shell::catalog::time` module owns both directions, and a table test pins it
against timestamp strings read out of the harvested fixture catalogs — strings
the Scala build actually wrote, not strings we believe it would write.

**Enum spellings.** `DiscType`, `ManifestChangeKind`, session `state`, disc
`status`, and `ArchivePointer.kind` all serialize as bare Scala enum names
(`ADDED`, `FINALIZED`, `BD_R`, …). serde defaults to the Rust field name, so
every such type needs an explicit rename — or, preferred here, an explicit
`as_str`/`FromStr` pair unit-tested against the frozen strings. Getting this
wrong in the catalog is caught immediately by the `check` constraints; getting
it wrong in the JSON is caught by nothing, so the fixture tests cover the JSON
paths specifically.

### 3.2 CLI surface (hard requirement)

Same nine commands, same flags, same environment variables
(`DISKREMENT_DEVICE`, `DISKREMENT_BURNER`, `DISKREMENT_HASH_PARALLELISM`, the
test-only `DISKREMENT_DISC_BYTES`), same catalog path precedence
(`--catalog` > `--data-dir` > `$STATE_DIRECTORY` > `$XDG_DATA_HOME/diskrement`),
and the same exit-code contract: 2 = state/lookup, 3 = environment/oversize,
4 = verify mismatch, 5 = unexpected, 6 = media/burner, and an aborted burn
exits 0.

Output *wording* may drift only where clap phrases usage and argument errors
differently from zio-cli. Output *structure* — what data appears in `status`,
`history`, `plan`, and the success-path wording pinned by the fixture render
oracle (§4) — does not.

---

## 4. Golden fixtures and the render oracle

The corpus is harvested in **phase P0, before a line of Rust is written**, while
the Scala build is trivially available. It is the reference for every
compatibility test from P2 onward, and it is what turns the one hard requirement
— data compatibility — into something the test suite checks on every run rather
than something asserted once by hand.

`scripts/harvest-fixtures.sh` drives a staged Scala launcher through a single
deterministic multi-cycle scenario that exercises spill, supersede, delete,
derived-`DEAD`, `LOST`-per-copy, recovery burn, and `FINALIZED` in one flow, and
records under `fixtures/`:

| path | what | byte-stable across re-harvests? |
|---|---|---|
| `catalog/NN-*.sqlite` | catalog at each recorded state | **no** — compare logically |
| `scancache/NN-*.sqlite` | fingerprint sidecar at each scan | **no** — same rule |
| `work/NN-after-planN/` | whole work dir at staged-plan moments (the recovery unit) | JSON yes, sqlite no |
| `discs/disc-N/` | staged disc trees: `data/` + `.diskrement/*` | data + JSON yes, embedded sqlite no |
| `source/` | the exact source tree, deterministic bytes | **yes** |
| `run-manifest.txt` | every command, exit code, and stdout — the **render oracle** | **yes** |
| `stderr.log` | stderr — reference only, **not** an oracle (progress lines vary) | no |
| `perf-baseline.txt` | `perf-bench.sh` output for the Scala build | no (timings) |

**Comparison rules, binding on every consumer:**

- Catalog and scancache comparisons are **logical** — dumped rows and query
  outputs, never raw bytes. SQLite file layout is not stable across library
  builds, and xerial's SQLite is not `libsqlite3-sys`'s.
- Manifest and pointer JSON compare **semantically** (parsed-value equality);
  field order is not an oracle.
- `run-manifest.txt` pins stdout and exit codes byte-for-byte for the
  success-path commands it records. It is the only cross-implementation render
  oracle that exists; `insta` snapshots (§8) pin Rust against Rust and cannot
  catch a wording drift from Scala.
- The corpus is **re-derivable**: P0 re-runs the harvest into a second directory
  and diffs the oracle, the source bytes, and the disc trees to prove the
  scenario is deterministic before anything is allowed to depend on it.

The Scala tree stays on the branch until the final phase precisely so this can
be re-run, and so the Scala source remains readable as the specification for
behavior the fixtures do not pin (planner ordinal arithmetic, the two-stage
oversize check, error kinds). **Read the Scala file before porting it.**

---

## 5. Architecture

### 5.1 What replaces ZIO

Nothing. diskrement is a batch CLI whose every I/O is blocking — filesystem,
SQLite, subprocesses — so it is written in plain synchronous Rust with a bounded
thread pool for parallel hashing. The Scala parallel-hash path already had a
fixed bound and the same first-error-fails-the-phase contract; a `rayon` pool
with an explicit `num_threads` reproduces both.

### 5.2 What replaces the ports

Most of them become nothing at all. The eight services injected into the Scala
`Pipeline` via `ZLayer` (`Pipeline.scala:61-63`, excluding `ProjectPaths`, which
is data) exist largely because ZIO's layer model encourages describing every
effect as a service. Rust has no such pressure, and idiomatic Rust does not
abstract `std::fs` behind a trait in order to fake it.

- `Hasher`, `FileScanner`, `SourceSnapshot`, `StagedStore`, `FingerprintCache`,
  and `CatalogStore` become plain structs with inherent methods, tested against a
  real `tempfile::TempDir` and a real temporary SQLite file.
- Genuine polymorphism survives only where a second implementation genuinely
  ships — the real-vs-simulated backend split. That is two closed enums,
  dispatched by `match`: `enum Burner { Xorriso(..), Simulated(..) }` and
  `enum Verifier { Disc(..), Staging(..) }`. `--burner real|simulated` selects
  both variants together, since verifying a simulated burn by mounting a physical
  disc is meaningless; backend resolution stays one decision made once in `cli`.
- `CmdRunner` disappears as a seam. The logic it existed to make testable is
  pushed into pure functions: `burn::xorriso::args` builds a `Vec<String>` and
  `burn::xorriso::parse` turns captured output lines into a media state, both
  unit-tested with zero I/O. What remains is a ~40-line `burn::cmd` that spawns a
  process and streams combined output lines to a callback.

This is the general shape of the translation: **a port becomes a pure function
plus a thin, boring edge.**

### 5.3 Layout

Three crates, under `crates/`, preserving the chain `cli → shell → core`:

```
Cargo.toml                       # workspace
rust-toolchain.toml              # pinned stable channel
.cargo/config.toml               # rustflags = ["-D", "warnings"]
crates/
  diskrement-core/               # pure; no fs, no SQLite, no process, no clock
    src/{lib,model,diff,planner,manifest,testutil}.rs
  diskrement-shell/              # every effect
    src/lib.rs
    src/error.rs
    src/catalog/{mod,schema,store,time}.rs
    src/fs/{scanner,hasher,snapshot,fingerprint_cache}.rs
    src/burn/{mod,stager,verifier,reader,media,json,snapshot_writer,cmd}.rs
    src/burn/xorriso/{args,parse,burner}.rs
    src/burn/simulated.rs
    src/pipeline/{mod,staged,reports}.rs
  diskrement-cli/                # binary `diskrement`
    src/{main,args,render,progress,prompt,paths}.rs
    tests/                       # integration scenarios
fixtures/                        # golden corpus (P0), shared by shell and cli tests
scripts/harvest-fixtures.sh      # new in P0
scripts/perf-bench.sh            # existing; repointed at the Rust binary in P9
```

Two notes on this layout.

**Why `crates/` and not the existing `core/`, `shell/`, `cli/` directories.** The
Scala modules live at those paths and must stay live until the final phase (§4).
Putting the Rust crates under `crates/` lets both trees coexist with zero
interference, which is what makes "keep the Scala build working until P9"
cheap rather than awkward.

**Why three crates and not two.** Collapsing `cli` into the shell binary would
save a manifest and a few hundred lines of ceremony, and it is tempting. It is
rejected because `CLAUDE.md` records "`Pipeline` returns data and does no I/O;
the CLI resolves paths and renders" as an invariant, and a crate boundary makes
that a compile error rather than a convention. Redesign is an explicit non-goal
(§1); the dependency chain is part of the design being ported. Core purity is
enforced the same way, one level down: `diskrement-core` takes exactly one
dependency, `chrono` with `default-features = false`, which excludes the `clock`
feature — `Utc::now()` does not compile there, and P1 proves it with a
deliberate negative-compile check.

### 5.4 Dependencies

**Runtime:** `clap` 4 (derive), `rusqlite` (`bundled`), `serde`, `serde_json`,
`sha2`, `walkdir`, `rayon`, `chrono`, `thiserror`, `reflink-copy`.

**Dev:** `proptest`, `tempfile`, `assert_cmd`, `predicates`, `insta`.

Toolchain: stable, pinned by `rust-toolchain.toml` at P0; edition 2021.
Warnings are failures via `.cargo/config.toml` rather than per-crate
`#![deny(warnings)]`, keeping the policy in one place. (Registry dependencies are
compiled with `--cap-lints allow`, so this does not fail on third-party warnings.)

`rusqlite`'s `bundled` feature compiles SQLite from source and links it
statically, so the binary carries no libsqlite3 requirement.

`reflink-copy` replaces shelling out to `cp --reflink=always`. This deletes the
`Reflink` module outright: the current code decides "this filesystem cannot
reflink" by substring-matching English `cp` stderr for `"failed to clone"` /
`"operation not supported"` (`Reflink.scala:6,10`), which is why it must force
`LC_ALL=C` into the child's environment (`SourceSnapshot.scala:33`,
`Stager.scala:96`). An `ioctl(FICLONE)` returns an errno, so the
hard-fail-in-snapshot / fall-back-in-stager decision becomes a match on an error
kind. That leaves three external commands: `xorriso`, `udisksctl`, `mount`.

### 5.5 Subsystems

**Catalog.** A single `rusqlite::Connection` — no pool. diskrement opens the
catalog, does its work, and exits; there is no concurrency to pool for. That one
decision retires four of the six gotchas in `CLAUDE.md` (§7). Migrations stay a
hand-rolled forward runner over `schema_version`, applying all pending versions
in one transaction.

**fs.** `walkdir` for the tree walk. Hashing runs on a rayon pool built with an
explicit `num_threads` from `DISKREMENT_HASH_PARALLELISM` — default 6, clamped
to 64, empty treated as unset, unusable non-empty value falling back to 6 with a
warning on stderr, all as today. The fingerprint cache is a second connection to
the sidecar file, with identical size+mtime trust semantics, total write-back,
and the `scan --verify` full-rehash escape hatch.

**pipeline.** A `Pipeline` struct with concrete fields — catalog, paths, burner,
staged store, and a small `ProgressSink` enum — exposing nine methods returning
`Result<Report, PipelineError>`. It continues to return data and perform no path
resolution.

**errors.** `thiserror` enums per module (`CatalogError`, `BurnError`,
`PipelineError`) wired with `#[from]`, plus one `exit_code()` function that is
the stable contract (§3.2), pinned by the ported `ExitMappingSpec` — including
the assertion that `StalePlan` advice says "re-run plan" and never mentions
"reset".

**cli.** clap derive. The progress reporter is hand-rolled (~50 lines on stderr,
refreshed line on a TTY via `std::io::IsTerminal`, periodic line otherwise)
rather than delegated to `indicatif`, because the CLI-compatibility requirement
makes matching existing behavior worth more than the dependency saves.

---

## 6. Concurrency model

All concurrency lives in `shell`; everything else is sequential.

- **Hash / stage-verify pool:** a rayon pool with `num_threads` = the resolved
  parallelism, results collected and sorted by `relPath` afterwards so output is
  order-independent. Same defaults, same clamp, same first-error-fails-the-phase
  contract as today.
- **Progress reporter:** one thread reading atomic counters (bytes, files),
  driving the `ProgressSink` cadence; shutdown joins workers before the reporter
  stops.
- No async runtime, no `spawn_blocking`, no async anywhere.

Expected performance is at parity or better — same thread count, SHA-NI via
`sha2`, no GC, direct `ioctl(FICLONE)`. The P9 benchmark against the P0-recorded
baseline is the evidence, not the claim.

---

## 7. Invariants and the disposition of the gotchas

All invariants in `CLAUDE.md` are properties of the domain, not of Scala, and
carry over unchanged, each re-pinned by its ported test:

- The catalog is append-only — rows never updated or deleted (`AppendOnlySpec` +
  schema).
- `DEAD` is derived from `disc_event`, never stored; `disc.status` keeps its
  check constraint. The pinning test asserts **the disc row itself is unchanged**,
  not merely the derived value — a test asserting only the derived value would
  pass against an illegal update-in-place.
- Commit after success; no `PLANNED` rows are persisted; the staged plan in the
  work dir is the recovery unit.
- One disc per cycle; `plan --finalize` seals a non-full disc.
- The embedded `catalog.sqlite` reflects post-burn state, built via
  `applyUpdateTo` on a work copy; never file-swap the SQLite file under a live
  connection.
- `LOST` is per physical copy, not per content hash.
- `StalePlan` advice is always "re-run plan", never "run reset".
- `Pipeline` returns data and does no I/O.
- Exit codes are a stable contract; an aborted burn exits 0.

**The gotchas mostly do not carry over**, and saying exactly how is part of the
design because it is most of the rewrite's payoff:

| gotcha | disposition |
|---|---|
| `Test / fork := true` for `DriverManager` | **gone** — no JVM-global driver registry |
| raw `transaction(...)` without `setAutoCommit(false)` | **structurally impossible** — `Connection::transaction()` returns a guard that rolls back on `Drop` unless committed |
| `Db.poolLayer` needing `Clock.ClockLive` | **gone** — no pool, no TTL fiber |
| `Db.wrapConn` proxy for null `getClientInfo()` | **gone** — no zio-jdbc |
| `finalize` unusable as a field name | **gone** — `closeDisc` may revert to a natural name |
| tests silently skipping on a non-CoW filesystem | **deliberately improved** — Rust has no `TestAspect.ignore`, so the skip becomes an early return that *prints its reason*, and a run where nothing executed says so instead of looking green |

Rust-era gotchas get recorded in `CLAUDE.md` as they are discovered. The list
starts empty on purpose, not because there are none.

---

## 8. Testing strategy

- **Unit tests** in `#[cfg(test)] mod tests` beside their code, against
  `tempfile` directories and real temporary SQLite files.
- **Property tests** with `proptest` in the core; `Generators.scala` becomes a
  set of strategies.
- **Snapshot tests** with `insta` for CLI rendering, replacing a large body of
  hand-written string assertions — *in addition to*, never instead of, the
  cross-implementation render oracle (§4).
- **Golden compatibility tests** consuming `fixtures/` from P2 onward under the
  comparison rules in §4.
- **Integration tests** in `crates/diskrement-cli/tests/`, driving
  `CARGO_BIN_EXE_diskrement` via `assert_cmd`. Cargo builds the binary for the
  test automatically, so the staging step the sbt suite needs disappears.

Every existing test's *assertion* gets a Rust counterpart. The test *code* is
rewritten idiomatically rather than transliterated.

### What "verified" means

Stricter than on the Scala side, which had no lint or format tooling and enforced
`-Wunused:all` by convention only:

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All clean, with no unexplained drop in test count. A green integration run that
silently skipped everything does not count — the suite prints why it skipped and
how many scenarios actually executed.

---

## 9. Phasing

Each phase gets its own plan document, lands green, and is a conventional-commit
series on `feat/rust-rewrite`.

| Phase | Scope | Ends with |
|---|---|---|
| **P0** | Fixture harvest + perf baseline + workspace scaffold + toolchain pin | Corpus committed and proven re-derivable; `cargo test` green; **Scala build untouched** |
| **P1** | Pure core: model, diff, planner, manifest | Core tests green, both property suites ported, purity proven by negative compile check |
| **P2** | Catalog: schema, migrations, store, `time` module | `AppendOnlySpec` port green; Scala-written fixture catalogs read correctly; timestamp round-trip pinned against harvested strings |
| **P3** | fs: scanner, hasher, fingerprint cache, snapshot; `init` / `scan` / `status` | **First runnable binary**; scancache semantics and `HashParallelismSpec` ports green |
| **P4** | Planner wiring, staged store, `plan` incl. `--finalize`, oversize, `StalePlan` | Staged JSON semantically identical to the fixture's |
| **P5** | Stager, manifests, pointers, embedded catalog snapshot, `burn` (simulated), commit-after-success | Staged disc tree diffs clean against the fixture disc trees |
| **P6** | `verify` (staging), `history`, `reset`, full rendering | insta snapshots for all CLI output; success-path renders match `run-manifest.txt` |
| **P7** | Coaster workflow: `disc mark-dead`, `disc_event` / `lost_version`, derived `DEAD`, lost report, re-plan | `LOST`-per-copy and unchanged-disc-row invariant tests green |
| **P8** | Real xorriso backend, media inspector, disc reader, udisks read-back verify, backend resolution | Image IT green where `xorriso` is installed; burner env semantics pinned |
| **P9** | Port the integration scenarios, perf comparison vs the P0 baseline, docs rewrite, branch swap | **Parity gate**; real-media smoke run; branch swap |

P0 deliberately front-loads the compatibility evidence for the risk in §11 that
would be most expensive to discover late. P1 is deliberately the pure core: it
has no compatibility surface at all, so it is the right place to absorb the
borrow-checker learning curve without a format question hanging over it. P3 is
the first phase that produces something a person can run, and every phase after
it is demonstrable.

The former single "catalog + fs + three commands" phase is split into P2 and P3
because it was the largest by a wide margin and its two halves have unrelated
failure modes.

---

## 10. Branch strategy

While the rewrite is in flight, `feat/rust-rewrite` is the **integration
branch**: it acts as the rewrite's `main`. The Scala tree stays untouched on it
for the duration, so it remains available both for re-harvesting fixtures and
for reading behavior the tests do not pin. No hybrid build exists: `cargo` and
`sbt` coexist without either knowing about the other.

Each phase's work goes on its own branch named `feat/rewrite-p<N>` forked from
`feat/rust-rewrite`, and merges back into `feat/rust-rewrite` with an explicit
merge commit titled `Merge feat/<branch>: <summary>`; the phase branch is
deleted after the merge. Never commit phase work directly to
`feat/rust-rewrite`, and no PRs — the same etiquette that governs `main`.

At P9, on the integration branch: delete `core/`, `shell/`, `cli/`,
`build.sbt`, and `project/`, and rewrite `README.md` and `CLAUDE.md` for the
Rust build. Then, and only when the rewrite's completion is explicitly
confirmed:

1. Push `main`'s tip to a preservation branch `scala` on `origin`.
2. Merge `feat/rust-rewrite` into `main` with an explicit merge commit
   (`Merge feat/rust-rewrite: <summary>`), per repo etiquette.
3. Delete `feat/rust-rewrite` locally and on `origin`.

Commits follow the existing convention — conventional commits with a scope
(`feat(catalog):`, `fix(cli):`, `test(core):`, `docs:`, `chore:`).

---

## 11. Risks

**Silent format drift.** The highest-consequence failure is a Rust binary that
writes a catalog or a disc the Scala version cannot read, discovered after media
has been burned. Mitigated by harvesting fixtures in P0 rather than at the end,
by the timestamp and enum-spelling tests called out in §3.1, and by the rule that
every catalog comparison is logical rather than byte-wise so the tests fail for
real reasons rather than SQLite build differences.

**Hardware-dependent phases.** P8 cannot be fully validated without a drive and
blank media, and its tests skip on machines without them — loudly, per §7. The
simulated backend keeps P1–P7 fully testable, which is why the real burner is
late rather than alongside `burn`. The real-media smoke run is part of the P9
bar, hardware permitting per `docs/smoke-test-real-media.md`.

**Learning-curve estimation.** The motivation is partly learning Rust, so phase
durations are not predictable from line counts. P1–P3 will go slower than their
Scala originals did; borrow-checker iteration is expected, not schedule failure.
The phasing is designed so that P3 produces something runnable early and every
phase after it is demonstrable, which keeps feedback frequent even if progress is
uneven.

**Scope across many sessions.** ~9k LOC plus a comparable test corpus. Mitigated
by the phase discipline: every landing is green and fixture-verified, so drift
cannot accumulate silently.
