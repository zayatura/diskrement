# diskrement — Incremental Per-Disc Burn Design

**Date:** 2026-06-27
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md`
**Predecessors:** Plan 3a (orchestration / `Pipeline`), Plan 3b (CLI adapter), post-burn catalog snapshot — all done and merged.

---

## 1. Scope & motivation

Today a single `burn` stages **all** discs of a plan and commits them atomically (3a decision 1, "commit-after-success"). That is unrealistic for an operator: a multi-disc set can take hours or days, each disc must be physically inserted, written, **finalized**, labelled, and swapped for a blank. A single long-running command cannot model "take this disc out, label it, put a blank in, repeat."

This change makes the burn **incremental and operator-paced**: each `scan → plan → burn` cycle produces exactly **one** disc and commits exactly **one** session. The operator repeats the cycle until a scan comes back clean.

**In scope:**
- `plan` stages a **single disc** (the first disc of the computed layout) instead of the full set.
- `burn` writes that one disc and commits one session (`FINALIZED` or `OPEN`).
- A `plan --finalize` flag to seal the cycle's disc even when it is not full.
- `burn` prints **operator guidance**: the disc label, its finalize status, and the next step.
- Drop `discCount` ("disc M of N") from on-disc metadata — the set size is unknowable in an incremental model.

**Out of scope (unchanged / deferred):**
- The pure `Planner` packing algorithm — **no change** (see §3).
- Interactive insert/eject/label *pauses* — still deferred to the real-hardware cycle. This iteration adds **display only**, no blocking prompts.
- Real ISO/UDF + hardware burning, real disc finalization. `FINALIZED` remains a catalog flag under simulation, but is **modelled as a physical close** (this is why "finalize the inherited open disc without writing to it" is rejected — see §4).

---

## 2. Workflow & state model

The cycle is `scan → plan → burn`, repeated, each cycle yielding one disc:

```
scan  → fresh CoW snapshot; diff vs. catalog finds all outstanding changes
plan  → packs the FIRST disc only; stages it (+ remaining estimate)
burn  → writes that disc; commits ONE session; clears staged state
        (repeat scan → plan → burn until scan is clean)
```

**Key properties:**

1. **The catalog is the cursor.** Once a disc's session is committed `BURNED`, those files are in the catalog view, so the *next* scan's diff naturally excludes them. No separate "remaining work" file is needed.
2. **Re-scan after each burn is already enforced.** `burn` clears the staged scan + plan (existing behaviour), so there is nothing to `plan` until the operator runs `scan` again. No new guard needed.
3. **Fresh snapshot every cycle.** Because each cycle re-scans, the CoW snapshot is recreated and released within the cycle (existing lifecycle). The hours/days between discs never hold a snapshot open, and source changes between discs are simply picked up by the next scan.
4. **Tombstones (deletions)** observed in a cycle's scan attach to **that cycle's** single disc session. They do not consume disc space; a metadata-only session (deletions but no added/modified files) is valid, as today.

---

## 3. The planner stays unchanged

`core/Planner.plan` already produces an ordered multi-bin layout where:

```
willFinalize = (i != lastIdx)   // disc i finalizes iff a later disc exists
```

For the **first** disc this means: `willFinalize == true` **iff the plan spilled** to a second disc. That is exactly the default finalize rule we want. So:

- **`Planner` is not modified.** Its existing tests stand.
- The Pipeline runs `Planner.plan` to get the full `BurnPlan`, then **consumes `discs.head`** and uses `discs.tail` to estimate the remaining work.

Rejected alternative: rewriting `Planner` to emit a single disc + remainder. More honest, but discards well-tested packing logic for no behavioural gain.

---

## 4. Finalization

- **Default:** the plan spills ⇒ this disc finalizes (`discs.head.willFinalize` from the planner).
- **Force (`plan --finalize`):** seal this cycle's disc even if everything fit:
  `willFinalize = head.willFinalize || finalize`.
- `FINALIZED` is modelled as a **physical disc close**. Consequence: we deliberately do **not** support "finalize a previously-open disc without writing to it, and burn a fresh disc in the same cycle" — that would force the operator to handle two physical discs in one cycle, defeating the one-disc-per-cycle simplicity. Disciplined per-cycle use of `--finalize` covers the same need (e.g. avoid topping up an almost-full open disc: finalize it on the cycle it is last written).

---

## 5. Component changes

### 5.1 `core` (`Planner.scala`)
No change.

### 5.2 `shell` — `Pipeline` / `PipelineLive`

**`plan`** — new signature:
```
def plan(name: String, discTypeOverride: Option[DiscType], finalize: Boolean)
    : IO[PipelineError, PlanReport]
```
- run `Planner.plan` (unchanged) → full `BurnPlan`,
- take `head = discs.head`; resolve `willFinalize = head.willFinalize || finalize`,
- compute a **remaining estimate** from `discs.tail`: `moreDiscs = discs.tail.size`, `moreBytes = discs.tail.flatMap(_.files).map(_.sizeBytes).sum`, `moreFiles = …`,
- stage **only that one disc** (with resolved `willFinalize`), its tombstones, oversized, discType, and the remaining estimate,
- return a `PlanReport` carrying the single disc + remaining estimate + oversized for rendering.

**`burn`** — collapse today's `ZIO.foreach(discData)` atomic-all loop to a **single disc**:
- read the staged single-disc plan,
- write one disc via `SimulatedBurner`,
- commit **one** session via one `CatalogUpdate` (`DiscWrite` with status `FINALIZED`/`OPEN` per the staged plan, carrying this cycle's tombstones),
- embed the post-burn catalog snapshot — now trivially "catalog state after this one session" (the existing snapshot-then-applyUpdateTo flow, single disc),
- clear staged state (existing),
- return a `BurnReport` carrying: ordinal, label, `finalized: Boolean`, `imageBytes`, and the remaining estimate (echoed from the staged plan) for operator guidance.

**`PlanReport` (new result type)** — replaces returning the raw `BurnPlan` from `plan`:
```
final case class RemainingEstimate(moreDiscs: Int, moreFiles: Int, moreBytes: Long)
final case class PlanReport(
    disc: PlannedDisc, willFinalize: Boolean, discType: DiscType,
    remaining: RemainingEstimate, oversized: List[RelPath])
```

### 5.3 `shell` — `StagedStore`
`writePlan` / `readPlan` change from `(BurnPlan, DiscType)` to the single-disc staged shape (one `PlannedDisc` with resolved `willFinalize`, tombstones, oversized, discType, `RemainingEstimate`). New JSON codecs; bump/rewrite `plan.json` layout (no migration needed — staged state is ephemeral work-dir data).

### 5.4 `shell` — on-disc metadata (`Burn.scala`, `Json.scala`)
- Remove `discCount` from `DiscIdentity` and from `ProjectJson` (the embedded `project.json`). The disc knows its own `discOrdinal`; the embedded catalog snapshot already lists every disc in the archive, so "of N so far" is derivable at read time without baking a false fixed total onto the disc.
- `PipelineLive` drops the `discCount = burnPlan.discs.map(_.ordinal).max…` computation.

### 5.5 `cli`
- `Args.DiskrementCommand.Plan` gains `finalize: Boolean`.
- `Commands.plan` adds a `--finalize` flag; passes it to `Pipeline.plan`.
- `Runner` threads the new `plan` signature and renders `PlanReport` / `BurnReport`.
- `Render.plan` (now over `PlanReport`): show the single disc — ordinal and whether it is a **new** disc or **appending to open disc N**, file count, bytes, % full, **will-finalize?**, and the remaining estimate ("~K more files / ~M more discs remain after this one"), plus oversized.
- `Render.burn` (operator guidance), e.g.:
  ```
  ✔ Wrote disc 3  —  label: "photos disc 3"
    Status: FINALIZED — set this disc aside, it is sealed.
    ~2 more discs of changes remain. Run: scan → plan → burn
  ```
  Variants: `Status: left OPEN — may receive more later.` and, when the estimate is empty, `No more changes — the set is up to date.`
- `burn` keeps the single `[y/N]` confirmation (skipped by `--yes`).

---

## 6. Command surface (changes only)

| Command | Change |
|---|---|
| `plan <name>` | add `--finalize` (seal this cycle's disc even if not full). Output now describes one disc + remaining estimate. |
| `burn <name>` | writes one disc, commits one session; output is operator guidance (label, finalize status, next step). |

All other commands (`init`, `scan`, `reset`, `status`, `verify`, `history`) are unchanged.

---

## 7. Testing

- **`Planner` tests:** unchanged (the algorithm is untouched).
- **`PipelineBurnSpec` / `PipelineSpillSpec`:** rework to single-disc burn — one session committed per `burn`; on spill, the cycle's disc is `FINALIZED` and the leftover changes are reported as the remaining estimate (not burned).
- **`PipelineFullLoopSpec`:** becomes a **multi-cycle** loop — `scan → plan → burn` repeated until a scan reports no changes; assert disc ordinals, per-session catalog state, and that the catalog-as-cursor excludes already-burned files each cycle.
- **`--finalize`:** a cycle whose changes fit on one disc but is sealed by `--finalize`; assert the disc is `FINALIZED` and the next cycle starts a fresh disc.
- **CLI:** `CommandsSpec` for `--finalize` parsing; `RenderSpec` for the new `plan` and `burn` output (new disc vs. open-disc append, finalize status, remaining estimate, "up to date").
- **On-disc metadata:** `JsonSpec` updated for the removed `discCount` field.

---

## 8. Risks & notes

- **Breaking on-disc format:** removing `discCount` changes `project.json`. Acceptable pre-1.0 with simulated burns; no discs in the wild to stay compatible with.
- **`Pipeline.plan` return type change** (`BurnPlan` → `PlanReport`) ripples to `Runner` and `Render`; mechanical.
- **Empty/metadata-only cycle:** deletions with no added/modified files still produce a one-session burn on the open disc — existing behaviour, preserved.
