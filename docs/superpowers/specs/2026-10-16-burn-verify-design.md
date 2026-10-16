# Burn with verify — design

**Date:** 2026-10-16
**Status:** Approved, not yet implemented
**Branch:** `feat/burn-verify`

---

## 1. Problem

Today a real burn ejects the disc as soon as it is written. To verify it, the
operator walks to the burner, pushes the tray back in, returns to the terminal and
runs `verify --disc 1A` for the disc just burned. With `--copies 2` that happens
twice per cycle, and the disc is already committed to the catalog before anyone
has read it back.

`burn --verify` reads each disc back while it is still in the drive, commits it
only once that read-back passes, and ejects it only after that. Without the flag,
`burn` behaves as it does today.

## 2. Is an immediate read-back as safe as eject, reinsert, verify?

For the data, yes.

- xorriso writes through SCSI pass-through (SG_IO), which bypasses the kernel's
  page cache, so none of the written data is cached in RAM. The drive's buffer is
  a few MB against 25 GB. The read-back through `udisksctl mount` reads the media.
- A stale view can only cause a false **failure**, never a false pass. A BD-R is
  write-once, so a cached old block cannot happen to match new data, and tier 1
  counts every expected file missing from the mount as a mismatch.

What an eject and reinsert adds is that the drive and the kernel recognise the
disc from scratch, as a restore will. Skipping it risks convenience, not
correctness: without a media-change event, `udisksctl` may fail to mount the new
disc, or may mount the previous session's view. §4.2 detects both and turns them
into *couldn't verify*, which falls back to an in-run reinsert. Whether the
Pioneer BDR-UD04 needs that fallback is a real-media question (§7.4).

## 3. Decisions

1. **Opt-in flag.** `burn --verify`, off by default.
2. **Verify before commit.** A disc whose read-back fails is not recorded. It is
   reported as failed and is to be discarded.
3. **A failed first session can be retried** on a new blank disc in the same run.
   The plan is unchanged by a failure, because nothing was recorded.
4. **A failed append retires the disc.** When session N > 1 fails, the disc
   physically holds a session the catalog does not know. The burner's identity
   check will refuse it forever (the disc reports N, the catalog expects N − 1),
   and a plain mount now shows the bad session first. Nothing is committed; the
   advice is `disc mark-dead <disc>`, then `plan` and `burn`. `mark-dead` stays a
   separate, deliberate command because it is irreversible.
5. **Couldn't verify is not a failure.** A mount failure, an unreadable or wrong
   identity, or a stale session view ejects the disc and asks the operator to
   reinsert it. Declining commits the disc as BURNED and unverified, which is
   exactly what a plain `burn` leaves behind, and continues.
6. **A read error is a failure.** A file that cannot be read while it is hashed
   off the mounted disc is a mismatch for that file, in `burn --verify` and in
   plain `verify` alike (§5.3).
7. **The pipeline runs the loop** (approach 1 of three). Putting the read-back
   inside `XorrisoBurner` would make the burner need the catalog's expected
   contents and duplicate the logic in the simulated burner. Chaining `burn` and
   `verify` in the CLI cannot keep a failed disc out of the catalog.

## 4. Architecture

### 4.1 `Burner`

`XorrisoBurner::burn` no longer ejects. `Burner::eject()` is new and best-effort,
with the current `eject` body: it does nothing for an image file or for the
simulated backend. The pipeline calls it once a disc is finished: right after the
burn when there is no verify, after the read-back when there is. A burn failure
still does not eject.

### 4.2 `Verifier::read_back`

A second entry point next to `verify`, for the disc already in the drive:

```rust
pub enum ReadBack {
    Passed { files: usize },
    Failed { files: usize, mismatches: Vec<RelPath> },
    Unverifiable(String),
}

impl Verifier<'_> {
    pub fn read_back(&self, exp: &DiscExpectation, session: u32) -> Result<ReadBack, BurnError>;
}
```

`DiscVerifier::read_back` does not prompt. In order:

1. Inspect the media once through xorriso. If the identity is not this project,
   disc and session `session`, the result is `Unverifiable`.
2. Mount through `DiscReader`. A mount failure is `Unverifiable`.
3. Read `<mount>/.diskrement/project.json` **from the mounted tree** and require
   that it names session `session`. A missing or older file means the kernel is
   showing a stale view: `Unverifiable`. Without this check a stale mount of an
   appended disc would show the new files as missing, and a good disc would be
   sent to `mark-dead`.
4. Tier 1 and tier 2 as in `verify`. Any mismatch, missing file or read error is
   `Failed`.

`StagingVerifier::read_back` checks the retained staging tree the way
`StagingVerifier::verify` does. It returns `Passed` or `Failed`, and
`Unverifiable` when the tree is absent.

`Err` is reserved for failures outside the disc (for example a temp dir that
cannot be created). The pipeline treats it like `Unverifiable`, naming the error,
so an environment hiccup never discards a written disc.

The reinsert in §5.1 reuses `read_back` after its own prompt. It does not go
through `DiscVerifier::verify`'s insert loop, because the prompt wording and the
session check differ.

### 4.3 The expected contents

`verify_disc` builds a `DiscExpectation` from the live catalog
(`placements_for_disc`, `disc_recorded_contents`). That construction moves into
one function that takes a catalog handle. `burn --verify` calls it on the work
copy `burn-catalog.sqlite`, which `burn_disc` has already built with
`Catalog::apply_update_to` and which includes this disc's session. The read-back
therefore checks exactly what a later `verify` would check against the committed
catalog.

### 4.4 `Pipeline::burn`

```rust
pub fn burn(&mut self, name: &str, now: DateTime<Utc>, burner: &Burner<'_>,
            verifier: Option<&Verifier<'_>>) -> Result<BurnReport, PipelineError>;
```

`burn_disc` gains the per-disc loop in §5.1. Everything before `burner.burn`
(the dead-disc guard, session ordinals, stale-plan checks, the work-copy catalog)
is unchanged. A retry calls `burner.burn(&image)` again with the same image: the
same `now`, the same work-copy catalog, restaged from `work_dir/snapshot`.

The CLI builds the verifier with the existing `resolve_verifier`, so the real
burner always gets `DiscVerifier` and the simulated burner always gets
`StagingVerifier`.

## 5. Behaviour

### 5.1 One disc under `burn --verify`

```
burner.burn(image)             prompt, stage, write, post-burn identity check (as today)
   │  failure → as today (nothing committed, no eject)
   ▼
read_back(exp, session N)      no prompt; identity, mount, project.json says session N, hash
   ├─ Passed       → commit → mark VERIFIED → discard staging → eject → next disc
   ├─ Unverifiable → eject → "Couldn't verify disc 1A: <reason>. Reinsert disc 1A to
   │                  verify it, or decline to leave it unverified."
   │                   ├─ reinserted → read_back again (wrong disc or still stale → ask again)
   │                   └─ declined  → commit as BURNED → discard staging → note
   │                                  "1A recorded unverified; run verify --disc 1A" → next disc
   └─ Failed       → eject, commit nothing, print
                      "Disc 1A FAILED verification: 3 of 9042 files bad (a.jpg, b.mp4, …).
                       It is not recorded; discard it."
                       ├─ session 1 → "To retry 1A, insert a new blank disc when asked; decline to stop."
                       │              → burner.burn(image) again (its own blank-disc prompt)
                       │                 └─ declined → stop: VerifyFailed
                       └─ session N>1 → stop: VerifyFailed (appended), no retry
```

- "Commit" is the existing `apply_update`. "Mark VERIFIED" is the existing
  forward-only `mark_verified` on session N, run after the commit. The embedded
  snapshot still equals the committed state, both showing BURNED. A commit
  failure after the read-back is the existing `Unrecorded { surely_written: true }`.
- The failure message lists at most the first five bad paths, in sorted order,
  then `…`.
- The failed disc carries the same identity as its retry (same project, disc,
  session and `created_at`), so the two cannot be told apart by reading them.
  That fails safe: the failed disc fails any later `verify`. The message says to
  discard it.
- Without `--verify` the flow is `burner.burn` → commit → discard staging →
  eject, which is today's behaviour with the eject moved from before the commit
  to after it.

### 5.2 Errors and exit codes

A new variant, exit **4**, the same code as a mismatch in `verify`:

```rust
PipelineError::VerifyFailed { disc: String, session: u32, bad: usize, checked: usize, appended: bool }
```

- Declining the retry makes the burner return `BurnError::Aborted`, normally
  exit 0. After a failed read-back the pipeline turns it into `VerifyFailed`.
- `appended: false` advises "re-run burn with a new blank disc".
  `appended: true` advises "disc 3A can no longer be appended to: run
  `disc mark-dead 3A`, then `plan` and `burn`".
- When an earlier set already committed in this run, `VerifyFailed` is the cause
  of a `PartialBurn`, which already exits with its cause's code. The tail reads,
  for example, `1A committed; 1B failed verification and was not recorded
  (re-run burn)`. `partial_burn_advice` gains one arm for `appended: true`:
  `mark 1B dead, then re-run plan and burn`. No advice ever mentions `reset`.
- A disc recorded unverified is not an error: exit 0, as for a plain `burn`.

### 5.3 Plain `verify`: read errors become mismatches

`hash_tree` returns a read error on a file as `Err(BurnError::Io)` today, which
stops the whole run with exit 6. It now records that file as a mismatch and
carries on, so the run finishes with exit 4 and every bad file named. Tier 2
already treats an unreadable archived version this way. An error walking the
tree (`walkdir`) also becomes a mismatch, for the directory that could not be
read. A failure to mount is still an error, as today.

### 5.4 Report

Each disc line of the burn report adds `verified (9042 files)` or
`recorded unverified — run verify --disc 1A`. Without `--verify` it is unchanged.

### 5.5 Unchanged

- `--yes` still answers only "Burn the staged plan?". The reinsert and retry
  prompts are disc questions, and no flag skips those.
- The simulated backend runs the same loop against its retained staging tree.
  There is no tray, so it never ejects.
- The staging tree is discarded only after the commit, so a retry and an
  unrecorded disc still find it. Every attempt is staged from `work_dir/snapshot`.

## 6. Out of scope

- Offering `mark-dead` inline after a failed append.
- Changing the advice for an append whose xorriso run fails partway, which also
  leaves an unrecorded session on the disc.

## 7. Testing and docs

### 7.1 Shell unit tests

- `XorrisoBurner::burn` runs no `-eject`. `Burner::eject` ejects a real device and
  does nothing for an image file or the simulated backend. The existing
  `-eject` assertions in `burner.rs` move to `eject()`.
- `read_back` on the scripted-exec testkit: `Passed`; `Failed` from a mismatch,
  from a missing file and from a read error mid-hash; `Unverifiable` from a mount
  failure, from a wrong and from an unreadable identity, and from a mounted
  `project.json` that names session N − 1.
- `StagingVerifier::read_back`: `Passed`, `Failed`, and `Unverifiable` when the
  tree is absent.
- Plain `verify`: a read error is a mismatch for that file and the run reaches
  exit 4.

### 7.2 Pipeline tests

With `run_real_burn_with` and a scripted operator and exec:

- Pass: one session committed and VERIFIED, one eject, after the read-back.
- Session-1 failure, then a retry that passes: exactly one session committed.
- Session-1 failure, retry declined: `VerifyFailed`, no rows written, staged plan
  unchanged.
- Append failure: `VerifyFailed { appended: true }`, nothing committed, no retry
  prompt.
- Unverifiable, reinserted, passes; unverifiable, declined: committed BURNED with
  no VERIFIED session.
- Two copies, 1A passes and 1B fails: `PartialBurn` with 1A committed, advice
  `re-run burn` (or `mark 1B dead…` for an append), never containing `reset`.
- Without a verifier: behaviour unchanged, eject issued by the pipeline after the
  commit.

### 7.3 Error and CLI contract

- `VerifyFailed` exits 4, and so does a `PartialBurn` whose cause it is: pinned in
  `error.rs`'s tests.
- `args.rs`: `burn --verify` parses; the default is off.
- `tests/binary.rs`: `burn --verify` with no stdin declines before the device is
  opened (exit 0).
- One scenario in `crates/diskrement-cli/tests/scenarios/`: a simulated
  `burn --verify` at `--copies 2`, after which `status` shows both discs VERIFIED.

### 7.4 Docs

- `--help` text for `--verify`.
- CLAUDE.md, Gotchas: the pipeline owns the eject; `read_back` checks
  `project.json` from the mount to catch a stale kernel view.
- `docs/smoke-test-real-media.md`, a new item to run before relying on the flag:
  `burn --verify` on a blank disc and on an append. Record whether `udisksctl`
  mounts the new session without a reload. If it does not, confirm that the
  reinsert prompt appears and that the reinserted disc verifies.
