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

The differential harness that used to compare against the Scala binary was deleted with the Scala tree at P10; `fixtures/` is now a frozen corpus that cannot be regenerated from `main`.

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
- **`std::process` has no `redirectErrorStream`.** `burn::cmd::system` reads
  stdout on the calling thread (live `UPDATE` progress) and drains stderr on a
  second thread, then replays stderr's lines through `on_line` and appends
  them after stdout's — this ports Scala's `redirectErrorStream(true)` +
  `onLine`, where merged lines all reach the callback. Stdout-then-stderr is
  enough for `tail` and `classify_failure`. Lines are decoded lossily: never
  go back to `BufRead::lines`, which stops at the first non-UTF-8 line and
  leaves the pipe undrained, so a chatty child blocks forever.
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

## Environment

- `DISKREMENT_DEVICE`, `DISKREMENT_BURNER` (`real`|`simulated`), `DISKREMENT_HASH_PARALLELISM` (default 6, clamped to 64).
- `DISKREMENT_DISC_BYTES` is test-only and **may only shrink** usable capacity; it is deliberately undocumented in `--help`.
- Catalog path precedence (`crates/diskrement-cli/src/paths.rs` `resolve`): `--catalog` > `--data-dir` > `$STATE_DIRECTORY` > `$XDG_DATA_HOME/diskrement`.
- Linux only. Real burning requires membership in the `optical` group. Manual hardware checklist: `docs/smoke-test-real-media.md`.

## Invariants — do not "fix" these

- **The catalog is append-only.** Rows are never updated or deleted. Enforced by schema and pinned by the `file_version`-only-grows proptest in `crates/diskrement-shell/src/catalog/store.rs`.
- **`DEAD` is derived, never stored.** `disc.status` keeps its `check (status in ('OPEN','FINALIZED'))` constraint; dead-ness is computed from `disc_event`. A test asserting only the derived value would pass against an illegal update-in-place, so also assert the row itself is unchanged.
- **Commit after success.** No `PLANNED` rows are persisted; the staged plan in the work dir is the recovery unit. A crash before commit leaves the catalog untouched.
- **One disc per cycle.** Each scan→plan→burn writes exactly one disc/session; `plan --finalize` seals a non-full disc.
- **The embedded `catalog.sqlite` reflects post-burn state** (built via `apply_update_to` on a work copy) so each disc is self-describing. The earlier pre-burn approach was deliberately reversed; never file-swap the SQLite file under a live pool.
- **`LOST` is per physical copy, not per content hash.** After a recovery burn, two versions sharing a hash can legitimately read `LOST` and `CURRENT`.
- **`StalePlan` advice is always "re-run plan", never "run reset"** — `reset` destroys staging that `StagingVerifier` needs. The `s07_reset` scenario asserts the message does not contain "reset".
- **`Pipeline` returns data and does no I/O**; the CLI resolves paths and renders.
- **Exit codes are a stable contract** (`crates/diskrement-shell/src/error.rs`, `PipelineError::exit_code`): 2 = state/lookup, 3 = environment/oversize, 4 = verify mismatch, 5 = unexpected, 6 = media/burner; an aborted burn exits 0. Pinned by that file's tests and `crates/diskrement-cli/tests/binary.rs`.

Known deferred debt, documented on purpose — don't casually "fix" it: `archive_pointers` has no dead flag and is already baked onto burned discs; `write_disc` doesn't refuse a dead disc; the fingerprint cache never prunes deleted paths; `copies>1` redundancy degrades silently.

## Repo etiquette

- Feature work goes on a branch off `main` named `feat/<topic>`, merged back with an explicit merge commit titled `Merge feat/<branch>: <summary>`, then the branch is deleted locally and on `origin`. No PRs. Never commit feature work directly to `main`.
- Conventional commits with a scope: `feat(catalog):`, `fix(cli):`, `test(catalog):`, `docs(plan):`, `chore:`.

## In-flight work

Specs and plans under `docs/superpowers/specs/` and `docs/superpowers/plans/` are the source of truth for what is being built and what was decided. Read the relevant plan before changing an area it covers; execution deviations are appended to the plan doc rather than left in commit messages.
