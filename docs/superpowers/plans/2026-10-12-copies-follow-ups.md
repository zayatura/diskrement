# Multiple-copies follow-ups (#7, #8, #10, #11, #12)

**Parent:** `2026-10-11-multiple-copies.md` ("Known limitations"). #6 and #9 have their own
specs and plans; #13 (raising `copies`) is deferred.

Each issue is a small, self-contained change on its own `feat/` branch, reviewed by a
read-only subagent before merging. This file records what each one decided; execution
deviations go under the issue's heading.

## #11 — mid-write I/O is not "not written"

Branch `feat/burn-io-phase`.

- New `BurnError::WriteInterrupted(io::Error)` (exit 6): `XorrisoBurner::write` maps a
  plain `Io` from running the burn command (pipe read, `wait()`) to it. `ToolMissing`
  from a failed launch passes through unchanged.
- `post_burn_check` turns every failure of the identity read-back into
  `PostBurnCheckFailed("could not read the identity back: …")`: it runs after a
  successful write, so it can never be a pre-write `Io`.
- `pipeline::burner_failure` classes `WriteInterrupted` with `BurnFailed` and
  `PostBurnCheckFailed` ("may have been written but was not recorded").
- A spawn failure other than "not found" at the write step is also `WriteInterrupted`.
  That is conservative: preflight and media inspection have just launched xorriso, so
  this is near-impossible, and over-warning costs a check of the disc, while
  under-warning costs a spent BD-R being treated as blank.
