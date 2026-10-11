# Verify Progress Implementation Plan

**Goal:** `verify` shows live progress while it reads a disc back, not ~30 minutes
of silence per full BD-R between the insert prompt and the report (issue #15).

**Spec:** the issue itself, https://github.com/zayatura/diskrement/issues/15. No
separate design doc: the change is confined to the hasher, the two verifiers, the
recorded-contents read and the CLI wiring, and touches no invariant in CLAUDE.md.

**Branch:** `feat/verify-progress`

---

## Tasks (TDD, each test watched failing first)

- [x] **1. Hasher reports bytes as it reads** (`fs/hasher.rs`). `hash_with(path,
  &mut dyn FnMut(u64))` calls back once per chunk read; `hash` delegates to it with a
  no-op. Tests: the callbacks sum to the file size for an empty file and one larger
  than the 64 KiB buffer, and `hash` and `hash_with` agree.
- [x] **2. Superseded versions carry their size** (`catalog/read.rs`,
  `catalog/model.rs`). `SupersededVersion.size_bytes` from `file_version.size_bytes`,
  so tier 2 can size its phase. Read-only change to one query; no schema change.
- [x] **3. Verifiers take a sink** (`burn/verifier.rs`). `StagingVerifier::new` and
  `DiscVerifier::new` take a `&dyn ProgressSink`. Tier 1 (and the staging verifier)
  walks first, then opens `verifying disc <name>` sized to the files' bytes and count,
  advances by bytes during each read and by one file after it, and closes through
  `progress::phase` so an I/O error still calls `done`. Tier 2 opens `extracting
  earlier versions on disc <name>` sized from the catalog, only when the disc has
  superseded versions, advancing one file per extraction. `DiscVerifier` tells the
  operator `Mounting disc <name>…` before it reads anything. Tests use
  `testkit::RecordingSink`: phase totals, advances summing to the totals, `done` last;
  a declined disc opens no phase; tier 2 only with superseded versions.
- [x] **4. CLI wiring** (`diskrement-cli/src/main.rs`). `Command::Verify` builds a
  `ConsoleProgress` and passes it to `resolve_verifier` and the `Pipeline`; it no
  longer uses `NoopSink`. The bar goes to stderr, so the pinned stdout reports are
  unchanged.
- [x] **5. Docs.** `docs/smoke-test-real-media.md` item 11 (verify of a full disc
  shows a refreshing `verifying disc …` line, including through a multi-GB file);
  CLAUDE.md notes that verify progress advances within a file.

## Execution deviations

- **Tier 2 is sized from the catalog**, which meant adding `size_bytes` to
  `SupersededVersion` (task 2). The issue sketched a files-only line, but the pinned
  `format` shape has a GB segment and an ETA, and both need a byte total.
- **`Mounting disc <name>…` goes through `BurnOperator::progress`**, so it lands on
  stdout beside the insert prompt and the burner's `Writing done` line, as the
  operator's other messages do. It only appears on the real backend, so the pinned
  simulated stdout reports are unaffected.
- **No new CLI test for "progress on stderr only."** `ConsoleProgress` writes only to
  stderr, and a non-TTY run emits its first line after 10 s, so a test would have to
  verify a disc for that long. The existing pinned stdout reports pass unchanged. By
  hand on 2026-10-11, a simulated 400 MB verify under `script` refreshed the line in
  place with the GB figure climbing at `0/2 files`, and ended with a newline before
  the report. The same run without a TTY wrote nothing to stderr.

## Verification

`DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace` (TMPDIR on btrfs, xorriso
installed), `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo fmt --check`. The TTY path has no automated test; real-media item 11 covers it.
