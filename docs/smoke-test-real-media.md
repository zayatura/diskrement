# Real-media smoke test (Plan 4a)

**Since the Rust rewrite's P8** this checklist applies to both implementations.
Run it with the Rust binary (`cargo run -p diskrement-cli -- <args>`, or the
built `target/debug/diskrement`); the Scala launcher (`sbt cli/stage`, then
`cli/target/universal/stage/bin/diskrement`) remains available until P10 for
comparing behavior on any item that surprises you. Item 5 (Tier-2 verification)
and item 2 (udisks mount) are the two the automated image-backed suite cannot
reach at all — `tests/xorriso_image.rs` covers osirrox extraction, but never a
mount.

Run on the dev machine (Pioneer BD-RW BDR-UD04, /dev/sr0) with one expendable
BD-R (or BD-RE for repeatable drills) before declaring cycle 2 done.
Prereq: `sudo usermod -aG optical $USER` + re-login (else expect the
DevicePermissionDenied message — which is itself checklist item 0).
Use plain UNFORMATTED BD-R media: a POW (Pseudo Overwrite) formatted BD-R is
"is unsuitable" to libburn and is refused (one such disc was in the drive on
2026-08-30 — swap it out first).

- [ ] 0. Without group membership: `diskrement burn ...` prints the
      optical-group hint and exits 6.
- [ ] 1. `init` a scratch project (small source dir, a few MB), `scan`, `plan`,
      `burn` to a blank BD-R. Confirm: insert prompt appears, xorriso UPDATE
      progress streams, disc ejects, catalog commits (`status` shows the disc).
- [ ] 2. `verify` — confirm the disc mounts via udisks, all files OK, session
      marked VERIFIED.
- [ ] 3. Add/modify a file, `scan`/`plan`/`burn` again to the SAME disc
      (top-off). Confirm the identity check accepted it and `-toc` (or
      `xorriso -indev /dev/sr0 -toc`) shows 2 sessions.
- [ ] 4. Insert a WRONG disc (any other disc) for a top-off: confirm refusal +
      re-prompt, then decline: "Aborted.", exit 0, catalog unchanged.
- [ ] 5. `verify` again: with a version superseded in-disc (the modified file),
      confirm Tier 2 ran (superseded version extracted and OK).
- [x] 6. `plan --finalize` + `burn`: confirm the disc closes. Record the exact
      `Media status` line of the closed disc here: `Media status : is written , is closed`
      (captured 2026-08-30; matches closedFixture verbatim — no parser change needed).
- [ ] 7. Attempt a top-off of the finalized disc: confirm DiscClosed refusal.
- [x] 8. Empty tray: run `burn` with no disc; record the exact failure text:
      NOT a failure — xorriso exits 0 with `Media status : is not present`
      (captured 2026-08-30). Fixed in `MediaParse.parse` (RawStatus.NotPresent →
      MediaStatus.NoMedia); `classifyFailure` unchanged.
- [ ] 9. `verify` with the disc in another machine's drive or after remount —
      optional bit-rot drill.

Findings feed parser fixes; re-run `sbt shell/test` after any parser change.
