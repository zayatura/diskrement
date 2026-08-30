# Real-media smoke test (Plan 4a)

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
- [ ] 6. `plan --finalize` + `burn`: confirm the disc closes. Record the exact
      `Media status` line of the closed disc here: ________________ and update
      `MediaParseSpec`'s closedFixture if it differs from `is written , is closed`.
- [ ] 7. Attempt a top-off of the finalized disc: confirm DiscClosed refusal.
- [ ] 8. Empty tray: run `burn` with no disc; record the exact failure text:
      ________________ and update `MediaParse.classifyFailure` candidates if it
      is not matched.
- [ ] 9. `verify` with the disc in another machine's drive or after remount —
      optional bit-rot drill.

Findings feed parser fixes; re-run `sbt shell/test` after any parser change.
