# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test

Scala 3.3.4 / sbt. Four modules with a strict dependency chain: `cli → shell → core`.

```
sbt test                        # all modules
sbt core/test                   # munit + scalacheck (pure core)
sbt shell/test                  # zio-test
sbt cli/test                    # zio-test
sbt cli/IntegrationTest/test    # end-to-end suite; stages the launcher first
sbt cli/stage                   # -> cli/target/universal/stage/bin/diskrement
sbt "cli/run scan photos"       # run without staging
./scripts/perf-bench.sh SRC WORK  # perf benchmark (DROP_CACHES=1 for cold cache)
```

Single test: `sbt "shell/testOnly diskrement.shell.catalog.DbSpec"` (wildcards work, e.g. `*MarkDeadSpec`).

No lint or format tooling is configured. `-Wunused:all` is on for all modules — keep imports clean; warnings are treated as failures by convention even though the compiler doesn't enforce it.

## What "verified" means

`sbt clean test` and `sbt cli/IntegrationTest/test` both pass, with no new warnings and no drop in test count. Do not claim green without running them.

Caveat: the IT suite **silently skips itself** when the work area isn't on a CoW filesystem (btrfs/XFS/bcachefs) — `Support.cowOrIgnore` applies `TestAspect.ignore` — and `XorrisoImageITSpec` skips when `xorriso` isn't installed. A green IT run may mean "skipped"; check the reported test count.

## Gotchas that will bite

- **Tests must fork.** `Test / fork := true` on shell and cli exists because `java.sql.DriverManager`'s registry is JVM-global. Removing it brings back an intermittent flake. See the comment in `build.sbt`.
- **Never write a raw `transaction(...)` without `setAutoCommit(false)` first.** xerial opens connections with autoCommit=true, which defeats zio-jdbc 0.1.2's rollback. `CatalogStoreLive.tx` prepends it; use `tx`.
- **`Db.poolLayer` builds the pool inside `ZIO.withClock(Clock.ClockLive)`.** Without it the pool's TTL fiber never advances under `TestClock` and DB suites hang. Don't replace this with per-spec `TestAspect.withLiveClock`.
- **`Db.wrapConn` proxies the connection** because zio-jdbc 0.1.2 NPEs on SQLite's null `getClientInfo()`. Don't unwrap it.
- **ZIO for-comprehensions can't tuple-destructure a bind** (no `withFilter`). Bind to one name, then destructure with `=` on the next line.
- **Don't call `java.nio.file.Files` statics inside `assertTrue(...)`** — `NoClassDefFoundError` under sbt's ManagedClassLoader. Hoist into a `ZIO.attempt` first.
- `finalize` is unusable as a field name (clashes with `AnyRef`); the codebase uses `closeDisc`.

## Environment

- `DISKREMENT_DEVICE`, `DISKREMENT_BURNER` (`real`|`simulated`), `DISKREMENT_HASH_PARALLELISM` (default 6, clamped to 64).
- `DISKREMENT_DISC_BYTES` is test-only and **may only shrink** usable capacity; it is deliberately undocumented in `--help`.
- Catalog path precedence (`PathResolver`): `--catalog` > `--data-dir` > `$STATE_DIRECTORY` > `$XDG_DATA_HOME/diskrement`.
- Linux only. Real burning requires membership in the `optical` group. Manual hardware checklist: `docs/smoke-test-real-media.md`.

## Invariants — do not "fix" these

- **The catalog is append-only.** Rows are never updated or deleted. Enforced by schema and pinned by `AppendOnlySpec`.
- **`DEAD` is derived, never stored.** `disc.status` keeps its `check (status in ('OPEN','FINALIZED'))` constraint; dead-ness is computed from `disc_event`. A test asserting only the derived value would pass against an illegal update-in-place, so also assert the row itself is unchanged.
- **Commit after success.** No `PLANNED` rows are persisted; the staged plan in the work dir is the recovery unit. A crash before commit leaves the catalog untouched.
- **One disc per cycle.** Each scan→plan→burn writes exactly one disc/session; `plan --finalize` seals a non-full disc.
- **The embedded `catalog.sqlite` reflects post-burn state** (built via `applyUpdateTo` on a work copy) so each disc is self-describing. The earlier pre-burn approach was deliberately reversed; never file-swap the SQLite file under a live pool.
- **`LOST` is per physical copy, not per content hash.** After a recovery burn, two versions sharing a hash can legitimately read `LOST` and `CURRENT`.
- **`StalePlan` advice is always "re-run plan", never "run reset"** — `reset` destroys staging that `StagingDiscVerifier` needs. `ExitMappingSpec` asserts the message does not contain "reset".
- **`Pipeline` returns data and does no I/O**; the CLI resolves paths and renders.
- **Exit codes are a stable contract** (`cli/src/main/scala/diskrement/cli/Render.scala`): 2 = state/lookup, 3 = environment/oversize, 4 = verify mismatch, 5 = unexpected, 6 = media/burner; an aborted burn exits 0. Pinned by `ExitMappingSpec`.

Known deferred debt, documented on purpose — don't casually "fix" it: `archivePointers` has no dead flag and is already baked onto burned discs; `writeDisc` doesn't refuse a dead disc; the fingerprint cache never prunes deleted paths; `copies>1` redundancy degrades silently.

## Rust rewrite gotchas (P2+)

These apply to the `crates/` tree only; the Scala gotchas above still govern
`core/`, `shell/` and `cli/` until P9 deletes them.

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

## The differential harness (P3+)

`crates/diskrement-difftest` runs a scenario through the Scala binary, the Rust
binary, or the two alternating, and diffs command output and end state. It
complements `fixtures/`, which is one-directional and frozen; the harness covers
the reverse direction and arbitrary scenarios. See its README to run it.

- **It is disposable.** P10 deletes it with the Scala tree. Nothing shipping may
  depend on it.
- **Every differential test is `#[ignore]`d.** Routine `cargo test` stays
  JVM-free. A run with no `DISKREMENT_SCALA_BIN` skips and says so — check the
  printed scenario counts, exactly as with the sbt IT suite.
- **Landing a command means appending it to `runner::RUST_IMPLEMENTED`** in the
  same commit. Otherwise the Rust and interleaved modes keep skipping that
  scenario, and a fully-skipped run still exits green.
- **A normalization rule is a hole in the oracle.** Each one carries a doc
  comment saying what varies and why erasing it is safe. Widening a rule to make
  a failing diff pass is how this harness stops working without anyone noticing.

## Repo etiquette

- Feature work goes on a branch off `main` named `feat/<topic>`, merged back with an explicit merge commit titled `Merge feat/<branch>: <summary>`, then the branch is deleted locally and on `origin`. No PRs. Never commit feature work directly to `main`.
- **While the Rust rewrite is in flight, `feat/rust-rewrite` acts as the integration branch**: each phase's work goes on its own branch (e.g. `feat/rewrite-p0`) forked from `feat/rust-rewrite`, merged back into `feat/rust-rewrite` with the same explicit `Merge feat/<branch>: <summary>` commit, then the phase branch is deleted. The same rules apply: never commit phase work directly to `feat/rust-rewrite`, no PRs. `main` only receives the rewrite when its completion is explicitly confirmed.
- Conventional commits with a scope: `feat(catalog):`, `fix(cli):`, `test(catalog):`, `docs(plan):`, `chore:`.

## In-flight work

Specs and plans under `docs/superpowers/specs/` and `docs/superpowers/plans/` are the source of truth for what is being built and what was decided. Read the relevant plan before changing an area it covers; execution deviations are appended to the plan doc rather than left in commit messages.
