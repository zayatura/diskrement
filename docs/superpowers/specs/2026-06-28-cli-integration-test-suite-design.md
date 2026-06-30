# CLI integration test suite — design

**Date:** 2026-06-28
**Status:** Approved for planning

## Goal

A black-box integration test suite that drives `diskrement` through its real CLI
binary across full scan → plan → burn → verify cycles, and asserts on **the
contents of the discs the tool produces**. The suite is hermetic: it never
touches the system-wide catalog location, cleans up all scratch data after every
test, and leaves nothing behind.

## Scope

- In scope: a Scala (ZIO-Test) suite that execs the staged CLI launcher and
  validates staged disc trees, on-disc metadata, and CLI output.
- In scope: one small, test-only production change — a disc-capacity override env
  var — to make multi-disc scenarios fast and deterministic.
- Out of scope (for now): crash/recovery simulation, real optical hardware,
  `--copies > 1`, multiple projects in one scratch.

## Background: what a "disc" is here

The simulated burner stages each disc to a directory under the project work dir:

```
<work-dir>/staging/disc-<ordinal>/
  data/<relpath>...                      # the burned file tree
  .diskrement/project.json               # project identity
  .diskrement/manifest.json              # this session's file list + hashes
  .diskrement/pointers.json              # archive pointers
  .diskrement/catalog.sqlite             # full catalog snapshot at burn time
```

That staged tree is exactly what the suite asserts on.

### One disc per cycle

`plan` computes the full multi-disc plan but only stages/reports the **head**
disc, returning the rest as a `remaining` estimate (`PlanReport.remaining`).
`burn` burns that one disc and clears staged state. Draining a backlog that
spans several discs is therefore a **loop of scan → plan → burn cycles**; the
planner tops off / opens the next disc each cycle from persisted disc states.

### Per-session staging nuance

The burner clears `staging/disc-N/` before each burn. So when an *open* disc is
topped off, `staging/disc-N/data` contains only the **latest session's** files,
not the cumulative disc contents.

**The oracle therefore runs immediately after every `burn`, before the next
cycle.** Each disc/session is validated directly at the moment it is freshly
staged — including intermediate sessions that a later top-off would overwrite.
Nothing is deferred to the end of a multi-cycle test, so no disc is ever "lost"
to a subsequent re-stage. The per-session nuance is not a limitation worked
around with history; it is simply *what the oracle asserts at each checkpoint*:
the session's staged tree and that session's manifest.

The one thing per-session staging cannot show is the *union* of multiple sessions
co-resident on a single physical disc. That cumulative current view lives in the
catalog, so the oracle additionally cross-checks it via `status` / `history`
after the relevant cycles — a corroborating check, not the primary evidence.

## Architecture

### Module layout

- A new `IntegrationTest` sbt config on the **`cli`** subproject.
- Sources under `cli/src/it/scala/diskrement/cli/it/`.
- Written with ZIO-Test (`ZIOSpecDefault`), consistent with the rest of the
  codebase.
- `IntegrationTest / test` `dependsOn (cli / stage)` so the launcher binary is
  built before the suite runs. Forked JVM.
- The IT config is **opt-in**: `sbt test` does not run it; you invoke
  `sbt cli/IntegrationTest/test` (an `itTest` alias may be added).
- Because the IT is in the same build, it decodes on-disc JSON with the
  **production codecs** (`ManifestJson`, `ProjectJson`, `PointersJson`) rather
  than reimplementing parsing.

### CLI invocation

Each command is run by exec'ing the staged launcher
`cli/target/universal/stage/bin/diskrement` via `ProcessBuilder`. This exercises
the real launcher script, JVM boot, and argument parsing — true end-to-end. JVM
startup (~1–2s/command) is acceptable for a focused suite.

### Helper components

- **`Cli`** — runs one command; returns `(exitCode, stdout, stderr)`. Injects
  the isolation env (catalog/data-dir + capacity override) into every subprocess.
- **`Scratch`** — per-test scratch lifecycle: create under CWD, tear down, assert
  no leak.
- **`Tree`** — generates files with unique random contents of given sizes and
  nesting; mutates the source (add / modify / delete).
- **`DiscOracle`** — assertions over a staged `disc-N/` directory.

## Isolation, scratch & cleanup

Per test, a unique scratch dir `./target/it-scratch/<uuid>/` (the current working
directory is assumed to be on btrfs) with:

```
source/    # the simulated source tree
work/      # --work-dir (snapshot, staging, transient catalog copies)
catalog/   # the SQLite catalog
```

- The CLI is always invoked with
  `--catalog <scratch>/catalog/<name>.sqlite`, and `--work-dir <scratch>/work` is
  passed at `init`.
- Belt-and-suspenders: the subprocess env also sets `XDG_DATA_HOME`,
  `STATE_DIRECTORY`, and `HOME` inside the scratch dir, so even a path-resolution
  bug cannot write to real system locations.
- **CoW probe** runs once: a `cp --reflink=always` attempt in scratch. If it
  fails, the **whole suite skips** with a clear message (matches the existing
  auto-skip convention for non-CoW filesystems).
- Teardown recursively deletes the scratch root in a ZIO `acquireRelease`
  finalizer (runs even on failure) and then **asserts the root no longer exists**.
- A suite-level check asserts `target/it-scratch/` is empty at the end.

## Test-only capacity override

A small production change in the shell: `PipelineLive.plan` reads an env var
`DISKREMENT_DISC_BYTES`. When set and `> 0`, it uses `Capacity(thatValue)`
directly instead of `Capacity.fromDiscType(...)`, bypassing the margin for
determinism. The override value persists naturally into new disc records'
`usableBytes`, so top-off in later cycles stays consistent.

- Documented as test-only; **not** added to `--help`.
- Lets spill/top-off run at KB scale with small **real** (random-content) files —
  fast, with negligible disk footprint.

## File generation

`Tree` writes **unique random bytes per file**, seeded by the file's relpath, so:

- Runs are reproducible (same seed → same bytes → same hashes).
- Two files of identical size still have distinct contents and hashes, so the
  oracle detects any content/identity mix-up.

Files are KB/MB-sized. Random data does not compress, but at these sizes the disk
footprint is negligible. Modifications change content (and optionally size) so the
diff classifies them as `modified`.

## The disc-content oracle

For each `disc-N/` a burn produces, `DiscOracle` checks:

1. **`data/` tree** equals the expected set of relpaths for that session/disc —
   no extras, no omissions — preserving directory structure.
2. **File integrity** — each staged file's size matches source; SHA-256 re-hash
   matches source (full re-hash; files are small).
3. **`manifest.json`** decodes, lists exactly those files with correct
   hashes/sizes, and carries the correct disc ordinal, session ordinal, label,
   and finalize flag.
4. **`project.json` / `pointers.json`** present and well-formed; project identity
   matches.
5. **`catalog.sqlite`** opens and reflects the burn (session present in
   BURNED/VERIFIED, expected file/version counts) — light row-count assertions to
   avoid over-coupling to schema internals.

Plus **CLI-surface assertions** per command: exit codes, and parsed stdout for
`scan` (added/modified/deleted/unchanged), `status` (discs/totals), `verify` (no
mismatches), `history` (version statuses).

## Scenarios

1. **Single-disc happy path** — init → scan → plan → burn → status → verify; disc
   contents == source; verify clean.
2. **Incremental — append to open disc** — disc-1 left open; a second cycle
   (add/modify/delete) writes session 2 to the **same** disc (ordinal stays 1,
   session ordinal → 2). Staged `disc-1` reflects session 2's files; the catalog
   current set is cumulative; `history` correct.
3. **Incremental — finalized → new disc** — disc-1 finalized (`--finalize` or
   full); changes in the next cycle go to a fresh **disc-2**; `history` correct.
4. **Three-disc incremental** — data drained across three discs (capacity
   override) with add/modify/delete distributed across cycles; assert each disc's
   contents and the full version history.
5. **Spill across cycles** — source larger than one disc; loop scan→plan→burn
   until `plan.remaining == 0`; the discs **partition** the file set (no overlap,
   none missing), each within capacity.
6. **Oversized file** — a single file larger than disc capacity; `plan`/`burn`
   reports it as oversized and refuses; non-zero exit; no disc produced.
7. **Reset** — `scan` then `reset` discards staged state; a following `plan`
   errors with "no staged scan."
8. **Idempotent re-scan** — scanning an unchanged source reports all-unchanged and
   plans a no-op cycle.

## Success criteria

- The suite runs green on a btrfs working directory and skips cleanly elsewhere.
- After a full run, `target/it-scratch/` is empty and no system-wide catalog
  location was written.
- Each scenario's disc contents are validated by the oracle (tree + hashes +
  metadata) and corroborated by CLI output.
