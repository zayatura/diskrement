# Continuous run — design

**Date:** 2026-10-17
**Status:** Approved, not yet implemented
**Branch:** `feat/continuous-run`

---

## 1. Problem

immich-dsz has pairs 1A/1B to 4A/4B burned and about 14 pairs left (the last one
partial). Each `plan` stages one disc per copy set, so the backlog takes ~14 rounds
of `plan` → `burn --verify`, each with "Burn the staged plan?" and two
"Insert a blank disc for … and confirm" prompts. A disc takes ~25 min to burn and
~20 min to read back, so the operator is called to the terminal every ~45 minutes
just to type `y`.

`diskrement run NAME` burns the whole backlog in one command. It plans and burns
cycle after cycle, and instead of asking for each disc it waits until the right
disc is in the drive.

## 2. Decisions

1. **A separate command**, `run`, not a `burn` flag. `burn` keeps meaning "burn
   exactly what is staged".
2. **Run until the backlog is on disc**, including the last partial pair, which
   stays open as an ordinary `plan` would leave it. Ctrl-C while waiting for a disc
   is the supported early stop.
3. **Stop on any error.** A failed read-back stops the run, so a failed disc cannot
   be mistaken for a good one by an operator who returns to an ejected tray.
4. **Detection replaces the insert question; the identity check stays.** A burn
   starts only once a disc is seated that xorriso identifies as exactly the one
   expected.
5. **The reinsert question stays a prompt.** "Couldn't verify, reinsert?" needs a
   judgement, so it remains `[y/N]`, and declining records the disc unverified, as
   under `burn --verify`.
6. **No notifications.** No bell, no hook. Deferred; the operator may revisit it.

## 3. The command and its loop

```
diskrement run NAME [--yes]        (device: --device / DISKREMENT_DEVICE, as for burn)
```

`run` takes no `--disc-type`, `--finalize` or `--rehash`: each cycle plans as a
plain `plan NAME` does. It always implies `--verify`.

```
check that the drive can be watched (§4.4)       real backend only
loop:
  plan                                          Pipeline::plan(name, None, false, false)
    └─ nothing to place → done
  first cycle only: render the plan with its estimate, then
                    "Burn the backlog (about N cycles)?" [y/N]   (--yes skips it)
    └─ declined → exit 0
  print the cycle line
  burn                                          Pipeline::burn_with(…, Some(&verify_after_burn))
    └─ error → stop with that error (§5)
  no disc of this cycle was finalized → done: the backlog fits
```

- **Plan first, every cycle.** `run` never resumes a staged plan as it stands. A
  re-plan after a run stopped halfway through a pair gives set B exactly the disc it
  was missing, because only set A's disc was committed. So `run` cannot hit
  `StalePlan`. The cost is one cached scan per cycle, while the operator swaps discs.
- **Two stopping points.** A plan with nothing to place, or a burned cycle whose
  discs were all left open (`will_finalize == false`), which means everything left
  fitted. The second rule stops `run` from chasing photos uploaded during the run:
  those would be appended to the disc just ejected. They wait for the next ordinary
  `plan`/`burn`, or the next `run`.
- **The cycle line** is printed once per cycle, before the burn: `Cycle 3: 7A
  (24.5 GB, finalizes) and 7B; about 10 more cycles after this.` A cycle is one
  disc per copy set, so the count reads the same at any `copies`. It is derived
  from `PlanReport`'s remaining estimate, in discs per set. Only the first cycle renders the full plan
  report.
- **The summary** is printed when the run ends, whether it ended normally or on an
  error: `This run: 5A, 5B, 6A verified, 6B recorded unverified; 1h 32m.` A disc
  recorded unverified is named as such, with its `verify --disc` advice.
- **Where it lives.** The loop is in the CLI (`crates/diskrement-cli/src/run.rs`),
  around the existing `Pipeline::plan` and `Pipeline::burn_with`; `Pipeline` still
  does no I/O. The stop rules are pure functions over `PlanReport` and `BurnReport`,
  tested alone. The simulated backend runs the same loop, with no detection, because
  it has no drive and never prompts for a disc.

## 4. Detecting the disc

### 4.1 Why not poll with xorriso

xorriso closes the tray whenever it acquires the drive. Its manual recommends "a
separate run of xorriso for loading the tray". Polling by running xorriso in a loop
would pull the tray in while the operator is swapping discs. Detection therefore asks
the kernel first and runs xorriso only once a disc is seated.

### 4.2 `DriveWatch`

```rust
pub enum DriveState { TrayOpen, NoDisc, NotReady, DiscOk, NoInfo }

pub trait DriveWatch {
    fn state(&self) -> Result<DriveState, BurnError>;
}
```

The real implementation opens the device with `O_RDONLY | O_NONBLOCK`, which neither
closes the tray nor reads the media and follows a `/dev/disk/by-id` symlink. It then
calls `ioctl(CDROM_DRIVE_STATUS, CDSL_CURRENT)` through `rustix`. Each poll opens and
closes the device, so no descriptor is held while xorriso runs. The mapping from the
ioctl's return values (`CDS_NO_INFO` 0 to `CDS_DISC_OK` 4) to `DriveState` is a pure
function. An open or ioctl failure is `BurnError` with the device named, exit 6.

### 4.3 The wait loop

`XorrisoBurner` gains an insert mode, set by a builder in the style of
`keeping_disc_in_drive`:

```rust
pub fn detecting_media(self, watch: &'a dyn DriveWatch) -> Self
```

In `await_correct_media`, the prompt is replaced by:

```
progress: "Waiting for <expected> in <device> (Ctrl-C to stop; nothing is written yet)."
loop:
  poll state() every 2 s until DiscOk
  check_media(id), once                    the existing strict identity check
    ├─ Ok(free) → return Ok(free)           a blank for session 1; this exact disc at N−1 for an append
    └─ Err(found) → progress, once per insertion:
                     "Found <found>; waiting for <expected>."
                     poll until state() is no longer DiscOk (the tray was opened), then loop
```

- **xorriso seeing no disc** although the kernel reports `DiscOk` (a drive still
  spinning up) is not a wrong disc: the loop polls again and re-inspects, rather
  than waiting for the tray to open.
- **Only `DiscOk` is inspected.** `TrayOpen`, `NoDisc`, `NotReady` and `NoInfo`
  all mean keep polling.
- **One inspection per insertion.** A wrong disc left in the drive is not inspected
  again. The loop waits for the tray to open first.
- **A disc already in the drive** when the wait begins is inspected at once. A blank
  there is used straight away.
- **The disc just ejected**, if the tray is closed on it again, is a wrong disc
  ("disc 5A of 'immich-dsz' at session 1"), and the loop keeps waiting.
- **An image file** (`BurnDevice::ImageFile`) is never polled. It is inspected once,
  as now, so the `stdio:` tests are unaffected.
- **No settle delay.** xorriso already waits for a drive to become ready. If the smoke
  test (§7.4) shows the first inspection after a tray closes misreading the disc, a
  5 s settle after `DiscOk` is added.
- `<expected>` is the wording the prompt already uses ("a blank disc for
  'immich-dsz' disc 5B"); `FINALIZE` is noted in the waiting line as it is in the
  prompt.

### 4.4 Up front

Before the first plan, `run` on the real backend calls `state()` once. A device that
cannot be opened, or a drive that does not answer the ioctl, fails then, with exit 6.
A run never scans for minutes only to fail at the first wait.

### 4.5 The invariant, restated

CLAUDE.md and `ConsoleOperator` say no flag may skip the insert question, so that a
burn cannot start before the operator has put the right disc in. `run` keeps the
intent and automates the check: a human must close the tray on a disc, and xorriso
must identify it as exactly the disc expected. Plain `burn` keeps its prompt. The
wording of both places is updated to say so.

## 5. Errors

- **The first error stops the run.** Earlier cycles stay committed; the current cycle
  behaves as `burn --verify` does (a `PartialBurn` if a set already committed), with
  one difference: a failed first session is **not** retried. Under `burn --verify`
  the retry goes through the burner's insert prompt; under detection it would simply
  wait for the next blank, and the run would carry on past a failure (decision 3).
  `VerifyAfterBurn` gains `retry_failed_first_session`, true for `burn --verify` and
  false for `run`, which stops with `VerifyFailed` straight away. The
  summary (§3) is printed, then the error's usual message and advice, and the run
  exits with that error's code.
- **Advice.** The existing advice is unchanged: "re-run burn" still works, because the
  staged plan is rewritten after every commit. `run` adds one line, `Or re-run
  diskrement run NAME; it re-plans first.`, except after a failed append
  (`VerifyFailed { appended: true }`), where the disc has to be marked dead first.
- **Declining** the first confirmation exits 0, with nothing burned.
- **Ctrl-C** has no handler: the process dies (exit 130). While waiting, nothing is in
  flight. During a burn, the outcome is that of interrupting `burn` today.
- **Exit codes** are those of `plan`, `burn` and the watch (6). No new variant.

## 6. Out of scope

- Notifications: a terminal bell, a `--notify CMD` hook.
- A limit such as `--pairs N`.
- Detection for plain `burn`.
- Appending files uploaded during a run.

## 7. Testing and docs

### 7.1 Shell unit tests

- The `CDROM_DRIVE_STATUS` → `DriveState` mapping.
- The detect loop, with a scripted `DriveWatch` (a sequence of states) and the
  scripted exec:
  - a blank is used with no `confirm_insert` call;
  - a wrong disc is inspected once and reported once, and the next inspection waits
    until the state has left `DiscOk`;
  - `NoDisc`, `TrayOpen`, `NotReady` and `NoInfo` keep it waiting;
  - an append finds this exact disc at session N − 1;
  - an image file never polls;
  - a watch error is a device error, exit 6.

### 7.2 CLI unit tests

- The stop rules: nothing to place → done; a burned cycle with every disc left open
  → done; a finalized disc → continue.
- The cycle line and the summary renderings, as snapshots.
- `args.rs`: `run NAME` and `run NAME --yes` parse; `run` requires a name.

### 7.3 Contract and scenario

- `tests/binary.rs`:
  - `run` on the simulated backend with no stdin declines at the confirmation and
    exits 0, with nothing burned. (The real backend cannot be used here: §4.4's
    up-front check needs a drive, and test machines may have none.)
  - `run --burner real --device <missing path>` exits 6 naming the device, before
    anything is scanned or staged (§4.4).
- Scenario `s17_run`: simulated backend, `--copies 2`, and a small
  `DISKREMENT_DISC_BYTES` so the backlog spans three pairs.
  - `run --yes` burns 1A to 3B, each read back verified, and 3A/3B stay open.
  - A second `run` reports nothing to burn and exits 0.
  - A `verify` sweep exits 0.

### 7.4 Real-media smoke item 14

On the first immich-dsz pair:

- the tray is never pulled in while discs are being swapped;
- a blank is picked up within seconds of the tray closing;
- closing the tray on the disc just burned prints "Found …" once and keeps waiting;
- Ctrl-C while waiting leaves `status` unchanged.

### 7.5 Docs

- `--help` for `run`.
- CLAUDE.md: a gotcha (xorriso closes the tray, so detection polls the kernel
  through `CDROM_DRIVE_STATUS`), and the insert-question invariant restated for
  `run` (§4.5).
- `ConsoleOperator`'s doc comment.
