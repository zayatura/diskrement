# diskrement Cycle 2 — Real Disc Burning Design

**Date:** 2026-08-30
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§6, §10 "deferred: real burner integration")
**Predecessors:** Cycle 1 (Plans 1–3b), post-burn catalog snapshot, incremental per-disc burn, CLI integration test suite — all done and merged.

---

## 1. Scope

Cycle 2 replaces the simulated burn with **real optical-media burning via xorriso**, makes
`verify` **read back from the physical disc**, and adds the **coaster workflow** (a disc
ruined by a failed burn can be marked dead and its recoverable content re-planned).

**In scope:**

- A `Burner` interface with two implementations: the existing simulated stage-to-directory
  burner (kept as the test/dev backend) and a new `XorrisoBurner` that writes real media.
- A `MediaInspector` port: interrogate the inserted disc (blank/appendable/closed, free
  capacity, and the disc's embedded diskrement identity) before any write.
- Strict pre-burn identity checks: a new disc requires blank media; a top-off requires the
  matching disc. Wrong media is never written.
- Physical finalization: when the plan marks the disc finalized, the burn closes the disc.
- Interactive insert/eject prompts in the CLI for real burns and real verifies (deferred
  from the incremental per-disc burn design to this cycle).
- Read-back `verify`: mount the disc via udisks, re-hash every visible file against the
  catalog (Tier 1), and extract superseded-within-disc versions from their original
  sessions via xorriso to hash them too (Tier 2). Every byte the catalog points at on the
  disc gets checked.
- Coaster workflow: `disc mark-dead` CLI command, an append-only dead-disc event in the
  catalog, a burned-view filter so live content whose only copy was on a dead disc is
  automatically re-planned, and a lost-history report for versions that are gone for good.
- Config: `device` and `burner` keys (+ env overrides).

**Out of scope (deferred):** enforced `copies > 1`; point-in-time restore; real
`/.archive` byte-linking; UDF mastering; automatic media-type adaptation beyond capacity
detection; a changer/robot workflow.

**Target environment:** Linux with `xorriso` ≥ 1.5.x and `udisks2`. Development hardware:
Pioneer BD-RW BDR-UD04 at `/dev/sr0`, BD-R media. ISO 9660 level 3 + Rock Ridge (Linux-
first; discs remain readable elsewhere without POSIX metadata).

---

## 2. Architecture & key decisions

Functional-core / imperative-shell discipline is unchanged. All new adapter code lives in
`diskrement.shell.burn` (burning, media) and `diskrement.shell.pipeline` (verify wiring);
the coaster filter is a `CatalogStore` query change plus a small pipeline change — the pure
`Diff`/`Planner` are untouched.

### 2.1 Key decisions

1. **xorriso is the single engine.** Mastering, burning, multisession growth, TOC/media
   interrogation, session extraction, and disc close are all driven through xorriso.
   Rationale: one tool for every media operation; native multisession append (it loads the
   previous session and grows the image, so a new `/.diskrement/catalog.sqlite` naturally
   hides the prior session's copy — the replace-within-disc semantics the core design
   assumed); and `stdio:/path/file.iso` pseudo-drives let the *real* burner code path run
   against plain files in tests. Alternatives rejected: growisofs+mkisofs (two tools,
   manual `-C` sector plumbing, maintenance-mode upstream, no file-backed test story);
   cdrskin/cdrecord (all multisession arithmetic on us).
2. **Staging is kept.** The real burn still stages the session tree (re-hash-on-copy, the
   existing validated step) and then burns the staged tree. The staged tree is the exact
   content xorriso masters; nothing is mastered directly from the CoW snapshot.
3. **Commit-after-success is unchanged.** xorriso failure, identity-check refusal, or the
   post-burn sanity check failing all leave the catalog untouched and the staged plan
   intact — the existing recovery unit.
4. **One disc per burn** (per the incremental per-disc burn design). There is no swap loop
   inside `burn`; a wrong or missing disc re-prompts or aborts, it never advances to
   another disc.
5. **Strict identity, no override.** No `--force` bypass of the identity check in this
   cycle. Adopting foreign media is out of scope.
6. **Dead discs are filtered, not re-planned specially.** Marking a disc dead removes its
   archive pointers from the "already burned" view the pipeline feeds `Diff`. Live source
   files whose only copy was there then diff as new/changed and are re-planned by the
   existing planner. No planner changes.

---

## 3. The `Burner` interface & `XorrisoBurner`

The current `SimulatedBurner` trait is renamed to `Burner`; `Pipeline` depends on `Burner`.

```
trait Burner:
  def burn(image: DiscImage): IO[BurnError, BurnResult]
```

- `SimulatedBurnerLive` keeps today's behavior byte for byte (stage + re-hash + write
  `/.diskrement`); it satisfies `Burner`.
- `DiscImage` gains `finalize: Boolean` (from the staged plan's finalize flag) and
  `device: BurnDevice` where `BurnDevice` is `RealDevice(path)` or `ImageFile(path)`
  (tests). `SimulatedBurnerLive` ignores both.
- `XorrisoBurnerLive(inspector: MediaInspector, hasher: Hasher, writer: SnapshotWriter,
  prompter: BurnPrompter)` implements the flow in §4.

Process execution: `ZIO.attemptBlocking` + `ProcessBuilder` (matching existing shell
style), stderr captured into errors, xorriso progress lines streamed to the CLI. Exact
xorriso invocations are a plan-level concern; the intended shapes are:

- new disc: `-outdev <dev> -map <staged> / -close <on|off> -commit`
- append: `-dev <dev> -map <staged> / -close <on|off> -commit` (xorriso loads the last
  session from `-dev` and grows it)
- inspect: `-outdev <dev> -toc`, `-media_info` equivalents, parsed
- identity read / session extraction: `-osirrox on` + `-extract`, with `-load session N`
  for Tier-2 verify
- charset/naming: ISO 9660 level 3 + Rock Ridge via xorriso defaults for `-map` mastering

---

## 4. The real burn flow

`burn` with the real backend, for the staged single disc:

1. **Prompt** (interactive): show which disc is expected — "insert blank disc N" or
   "insert disc N (label …) to top off" — and wait for confirmation. A `BurnPrompter`
   port abstracts this so the pipeline stays prompt-free; the CLI provides the interactive
   implementation, tests a scripted one.
2. **Inspect** the media via `MediaInspector` (§5).
   - Session 1 expected → media must be `Blank` (of sufficient capacity).
   - Session > 1 expected → media must be `Appendable` and its embedded identity must
     match project name, disc ordinal, and session ordinal = expected − 1.
   - Anything else → precise refusal (wrong disc, not blank, closed, no media, foreign
     disc) and re-prompt (or abort after operator chooses to).
3. **Stage** the session exactly as today (re-hash-on-copy into the work dir).
4. **Capacity check** (belt and braces): staged bytes + ISO filesystem overhead margin
   must fit the media's reported free space. The planner should already guarantee this;
   a violation is a hard, loud error.
5. **Burn** with xorriso; close the disc iff `finalize` is set.
6. **Post-burn sanity check**: re-open the drive, re-read the TOC, and extract the new
   session's identity file to confirm the session committed and is readable. This is a
   cheap smoke test, not the full verify.
7. **Eject** the disc and print the label guidance (existing operator-guidance output).
8. Return `BurnResult`; the pipeline commits the catalog exactly as today.

Failure at any step before 8 leaves the catalog uncommitted and the staged plan intact;
the operator may retry `burn` (same disc for transient errors, a fresh blank for a ruined
new disc) or invoke the coaster workflow (§7) for a ruined top-off disc.

---

## 5. `MediaInspector`

```
trait MediaInspector:
  def inspect(device: BurnDevice): IO[MediaError, MediaStatus]

enum MediaStatus:
  case NoMedia
  case Blank(capacityBytes: Long)
  case Appendable(freeBytes: Long, identity: Option[DiscIdentity])
  case Closed(identity: Option[DiscIdentity])
```

- `identity` is read by extracting `/.diskrement` identity metadata from the disc's last
  session via xorriso; `None` means readable media that is not a diskrement disc (which
  the identity check treats as foreign and refuses).
- Live implementation parses xorriso TOC/media output; parsers are pure functions tested
  against captured fixture outputs from the real drive and from image files.
- A fake implementation (scripted statuses) serves burner/pipeline tests.
- `verify` reuses the same inspection to confirm the right disc is inserted before
  mounting.

---

## 6. Read-back verify

`verify` with the real backend, per disc (prompt-insert like `burn`):

- **Tier 1 — mount & re-hash.** Mount via `udisksctl mount -b <device>` (polkit lets a
  desktop-session user mount removable media without root). Walk the mounted `/data`
  tree; hash every file against the catalog's recorded contents *for that disc* (the
  per-disc semantics from `0376e0a`); confirm `/.diskrement` manifest and catalog
  snapshot are present and readable. Unmount and eject when done. This proves the
  archival property that matters: the kernel can mount the disc and every visible byte
  reads back correctly.
- **Tier 2 — superseded-version extraction.** For versions the catalog records on this
  disc that are *hidden* in the merged view (same path re-burned in a later session on
  the same disc — rare), load the recording session via xorriso, extract those files to a
  temp dir, and hash them. Runs only when such versions exist, so it is usually free.
  Together the tiers check every physical byte the catalog points at on the disc.
- Success calls the existing `markVerified`. Mismatches are reported per file with the
  existing verify report shapes.
- The disc-reading side (mount/walk/extract) sits behind a `DiscReader` port so the
  image-file test implementation can read from `.iso` files via osirrox extraction
  (mounting an image requires loop devices — not portable in tests).
- The simulated backend keeps the current staged-data verify; the CLI integration suite
  is untouched.

---

## 7. Coaster workflow (dead discs)

**Catalog model.** A new append-only event marks a disc **dead** (schema detail for the
plan: an event/status row, never an update-in-place). Derived status rules:

- A dead disc is never eligible for top-off; if it was the open disc, the planner opens a
  new disc (falls out of the filtered view — the open disc simply no longer appears).
- Dead discs are excluded from `verify` runs and flagged in `status` output.
- Its archive pointers stop counting as "safely burned": the catalog query feeding the
  pipeline's current-view/burned-state input to `Diff` **excludes pointers on dead
  discs**. Live source files whose only copy was there then automatically re-plan.

**CLI.** `diskrement disc mark-dead <ordinal>` with a confirmation prompt. On marking, a
**lost-history report** is printed and recorded: which file versions are now permanently
lost (their only copy was on the dead disc and the source has moved past them) versus
which live files will be re-burned on the next scan/plan. Lost versions stay in the
catalog (append-only), flagged lost.

**Typical recovery flow** for a ruined top-off disc: burn fails → retry (transient?) →
`disc mark-dead N` → `reset` staged state → `scan`/`plan` (now includes the failed
session's content *and* anything only on the dead disc) → `burn` to fresh media. A ruined
*new* disc with no committed sessions needs no marking — swap in a blank and retry.

**Known interaction:** `archivePointers`/`fileHistory` duplicate-row caveat for
`copies > 1` (deferred note from 3a) also applies to the dead-disc filter; both activate
together when `copies > 1` lands.

---

## 8. Config & environment

Extends the existing config file / env override mechanism:

| Key | Default | Env override | Meaning |
|-----|---------|--------------|---------|
| `device` | `/dev/sr0` | `DISKREMENT_DEVICE` | Burner device path |
| `burner` | `real` | `DISKREMENT_BURNER` | `real` or `simulated` |

The CLI integration suite pins `DISKREMENT_BURNER=simulated` explicitly. The existing
`DISKREMENT_DISC_BYTES` shrink-only clamp keeps working for both backends (for real media
it may only shrink below the media's reported capacity).

---

## 9. Error model

New `BurnError`/`MediaError` cases, each with a distinct, actionable CLI rendering and
exit-code mapping per the existing CLI conventions:

- `ToolMissing` / `ToolTooOld` — xorriso absent or below required version (preflight).
- `DeviceNotFound`, `DevicePermissionDenied` — the latter detects the EACCES case and
  tells the operator exactly what to do (add user to the `optical` group, re-login).
- `NoMedia`, `NotBlank`, `DiscClosed`, `WrongDisc(expected, found)`,
  `ForeignDisc` — identity/state refusals from §4 step 2.
- `InsufficientCapacity(needed, available)` — §4 step 4.
- `BurnFailed(exitCode, stderrTail)` — xorriso failure with captured stderr.
- `PostBurnCheckFailed(detail)` — §4 step 6.
- `MountFailed(detail)` — udisks mount failure during verify (includes the
  udisks-not-available case, with guidance).

Media-state refusals during an interactive burn loop back to the insert prompt rather
than aborting the run; all other errors abort with the mapped exit code.

---

## 10. Testing strategy

1. **Unit tests:** xorriso command construction; TOC/media-info parsing against captured
   fixtures (from the real Pioneer drive and from image files); identity-check decision
   table; dead-disc-filtered catalog queries; lost-history report content.
2. **Image-file integration tests:** `XorrisoBurner` against `stdio:/…/disc.iso`
   pseudo-drives — the real code path, no hardware: first-session master, multisession
   append, finalize (close), post-burn sanity check, Tier-2 session extraction, and
   `DiscReader`-based verify against the image. Tagged/separated so environments without
   xorriso skip them cleanly.
3. **CLI integration suite:** unchanged, pinned to the simulated backend.
4. **Real-media smoke test:** a documented manual checklist run on the Pioneer drive
   before the cycle is called done: burn a new BD-R → top off → finalize via
   `plan --finalize` → full read-back verify → (optional, on expendable media)
   mark-dead drill confirming re-plan picks up the content.

Prerequisite for real-media testing on the dev machine: the user must be in the
`optical` group (`sudo usermod -aG optical $USER` + re-login); the
`DevicePermissionDenied` error message documents this.

---

## 11. Delivery split

One spec (this document), two implementation plans, matching the 3a/3b precedent:

- **Plan 4a — real burning + read-back verify:** `Burner` split, `XorrisoBurner`,
  `MediaInspector`, `BurnPrompter` + CLI insert/eject prompts, config keys, error model,
  Tier-1 + Tier-2 verify, `DiscReader`, image-file integration tests, smoke-test
  checklist.
- **Plan 4b — coaster workflow:** dead-disc catalog event + filtered burned view,
  `disc mark-dead` CLI command, lost-history report, `status`/`verify` exclusions, tests.

4a is independently shippable; 4b builds on the catalog only and does not touch the
burner.
