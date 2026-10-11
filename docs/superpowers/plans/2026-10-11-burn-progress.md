# Live Burn Progress Implementation Plan

**Goal:** A real `burn` shows live progress while xorriso writes, not ~25 minutes
of silence followed by ~1,500 replayed `UPDATE` lines (issue #14).

**Spec:** the issue itself, https://github.com/zayatura/diskrement/issues/14. No
separate design doc: the change is confined to the runner, one parser, the burner's
`write` step and the console renderer, and touches no invariant in CLAUDE.md.

**Branch:** `feat/burn-progress`

---

## Cause

xorriso writes everything, including its `UPDATE … Writing:` pacifier, to stderr.
Verified 2026-10-11 with xorriso 1.5.8.pl02 by running `args::burn`'s command line
against a `stdio:` target with the streams split: stdout was empty. `cmd::system`
streamed stdout live but collected stderr on a thread and replayed it after the child
exited, on the false premise (in a code comment and in CLAUDE.md) that `UPDATE` was on
stdout. This dates from the runner's first Rust version (`f924284`); the Scala runner's
`redirectErrorStream(true)` did not have the problem.

## Tasks (TDD, each test watched failing first)

- [x] **1. Runner: one pipe for both streams** (`burn/cmd.rs`). `std::io::pipe()`;
  the write end is cloned for stdout and moved into stderr; the `Command` is dropped
  right after `spawn` so the parent's write ends close and the reader sees EOF. Read on
  the calling thread with `lossy_lines`. Tests: a stderr line reaches `on_line` while the
  child still runs (failed at 3.0 s, now immediate); `echo a; echo b >&2; echo c`
  arrives as `a, b, c` (failed as `a, c, b`).
- [x] **2. Parser** (`burn/xorriso/parse.rs`). `writing_per_mille(line) -> Option<u32>`
  reads `UPDATE : Writing: <N>s <P>%` into tenths of a percent, capped at 1000, with no
  floats. Tests use lines captured from `stdio:` and from the real drive.
- [x] **3. Bytes-only progress phase.** `ProgressSink::start_bytes` with a default that
  calls `start(label, bytes, 0)`, so `NoopSink` and test sinks need nothing.
  `ConsoleProgress` overrides it and renders `format_bytes`:
  `burning disc 1A 12.3/24.5 GB · 14 MB/s · ETA 14m02s`. The pinned `format` shape of
  every other phase is unchanged (`format` and `format_bytes` share `rate_and_eta`).
- [x] **4. Burner** (`burn/xorriso/burner.rs`). `write` opens a `start_bytes` phase sized
  to the staged image and feeds each output line to a `WriteProgress` state machine
  (`Before` → `Writing { reported }` → `Ended`): percentages advance the bar to
  `image_bytes × per_mille / 1000`; 100% or a `closing`/`fixating` line fills the bar,
  closes the phase and tells the operator once (`Writing done; closing the session and
  finalizing disc 1A. This can take a minute.`, or `… closing the session on disc 1A.`
  when not sealing). A burn that never reaches either still closes its phase, quietly.
  Raw `UPDATE` lines are no longer echoed; they stay in `CmdOutput.lines` for `tail`.
  `testkit` gains `RecordingSink` and `ScriptedExec::burn_lines`.
- [x] **5. Docs.** CLAUDE.md's runner gotcha rewritten and a progress gotcha added;
  `docs/smoke-test-real-media.md` corrects the 2026-09-11 "streamed" note and adds
  item 10 (progress on a full-size disc).

## Execution deviations

- **No `tests/xorriso_image.rs` progress test.** The issue proposed one: a burn of a
  multi-hundred-MB tree to a `stdio:` image producing a progress advance. xorriso emits a
  pacifier line about once a second and writes an image at over 1 GB/s, so a
  deterministic test would have to write more than 2 GB to a temp image (seen on
  2026-10-11: 2.5 GB gave three `Writing:` lines). That's slow and flaky. Instead, the
  parser and burner tests pin lines captured from the real drive while disc 1A of
  `immich-dsz` burned on 2026-10-11 (Pioneer BDR-UD04, BD-R, `-close on`):
  `Writing:   11974920s  100.0%   fifo   0%  buf 100%    3.4xB` and
  `Closing track/session. Working since 1292 seconds` (repeated about once a second for
  the ~11 s close). Real-media item 10 covers the end-to-end behaviour.
- **The closing match is loose** (`closing` or `fixating`, case-insensitive) because
  libburn's wording varies by medium; only the BD-R `Closing track/session` form has
  been observed.

## Verification

`DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace` (TMPDIR on btrfs; xorriso
installed, so `xorriso_image.rs` ran), `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo fmt --check`. Real-media item 10 is to be run on the next disc (1B).
