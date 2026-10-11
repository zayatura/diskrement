# Real-media smoke test

Run this with the Rust binary: `cargo build --release -p diskrement-cli`, then
`target/release/diskrement`. Item 5 (Tier-2 verification) and item 2 (udisks
mount) are the two the automated image-backed suite cannot reach at all —
`tests/xorriso_image.rs` covers osirrox extraction, but never a mount.

Run on the dev machine (Pioneer BD-RW BDR-UD04) with one expendable
BD-R (or BD-RE for repeatable drills) before declaring cycle 2 done.
The machine also has a PiKVM virtual boot drive, and the two swap `/dev/srN`
numbers between boots (the Pioneer was `sr1` on 2026-09-11 and `sr0` on
2026-10-11). Address the burner by its stable udev path, which `xorriso`, the
`udisksctl` mount in `verify` and `diskrement` all accept:

```
export DISKREMENT_DEVICE=/dev/disk/by-id/usb-PIONEER_BD-RW_BDR-UD04_<serial>-0:0
```

`ls -l /dev/disk/by-id/ | grep sr` shows which `srN` it currently is. The run
records below name the device as it was on the day.
Prereq: `sudo usermod -aG optical $USER` + re-login (else expect the
DevicePermissionDenied message — which is itself checklist item 0).
Use plain UNFORMATTED BD-R media: a POW (Pseudo Overwrite) formatted BD-R is
"is unsuitable" to libburn and is refused (one such disc was in the drive on
2026-08-30 — swap it out first).

- [ ] 0. Without group membership: `diskrement burn ...` prints the
      optical-group hint and exits 6.
      (Not performed on hardware 2026-09-11: a non-optical process cannot be
      arranged without root. The mapping is pinned by unit tests —
      `burn/media.rs` permission→`DevicePermissionDenied`,
      `error.rs` →exit 6, `pipeline` exit-code test.)
- [x] 1. `init` a scratch project (small source dir, a few MB), `scan`, `plan`,
      `burn` to a blank BD-R. Confirm: insert prompt appears, xorriso UPDATE
      progress streams, disc ejects, catalog commits (`status` shows the disc).
      (Rust, 2026-09-11 — blank CMC BD-R in sr1: both prompts appeared,
      `UPDATE : Writing:` streamed, `✔ Wrote disc 1`, drive ejected,
      `status` showed `#1 OPEN`. First burn: single `y` answers the plan
      prompt only — the insert prompt is a second confirmation.)
      (Correction, 2026-10-11: the `UPDATE` lines did not stream. xorriso
      writes them to stderr, which the runner then replayed only after the
      child exited; on a few-MB burn the two look the same. See item 10.)
- [x] 2. `verify` — confirm the disc mounts via udisks, all files OK, session
      marked VERIFIED.
      (Rust, 2026-09-11 — `disc 1: 3 files OK ✓`; `session` row advanced to
      `VERIFIED`.)
- [x] 3. Add/modify a file, `scan`/`plan`/`burn` again to the SAME disc
      (top-off). Confirm the identity check accepted it and `-toc` (or
      `xorriso -outdev "$DISKREMENT_DEVICE" -toc`) shows 2 sessions.
      (Rust, 2026-09-11 — `appending to open disc 1` accepted;
      `xorriso -outdev /dev/sr1 -toc`: `Media status : is written , is appendable`,
      `Media summary: 2 sessions`, `ISO session 1` + `ISO session 2`, both
      `P_DISC_1`. Note: `-indev` refuses an appendable disc outright — use
      `-outdev -toc` while the disc is open.)
- [x] 4. Insert a WRONG disc (any other disc) for a top-off: confirm refusal +
      re-prompt, then decline: "Aborted.", exit 0, catalog unchanged.
      (Rust, 2026-09-11 — `Not the expected disc: found unsuitable media
      (e.g. a POW-formatted BD-R) — use a plain unformatted BD-R. Swap discs
      and confirm again.` → re-prompt → `Aborted.`, exit 0; catalog still
      exactly the burned sessions, 0 PLANNED rows.)
- [x] 5. `verify` again: with a version superseded in-disc (the modified file),
      confirm Tier 2 ran (superseded version extracted and OK).
      (Rust, 2026-09-11 — `disc 1: 5 files OK ✓` where the merged mount holds
      4 files: 4 Tier-1 + 1 Tier-2 extraction of the superseded `file1.bin`
      v1 from session 1. An extraction or hash failure would have printed a
      mismatch — `verifier.rs` tier2.)
- [x] 6. `plan --finalize` + `burn`: confirm the disc closes. Record the exact
      `Media status` line of the closed disc here: `Media status : is written , is closed`
      (captured 2026-08-30; matches closedFixture verbatim — no parser change needed)
      (Rust, 2026-09-11 — re-confirmed: same line verbatim, `Media summary:
      3 sessions ... 0 free`; plan printed `will FINALIZE`, burn printed
      `Status: FINALIZED — set this disc aside, it is sealed.`).
- [x] 7. Attempt a top-off of the finalized disc: confirm refusal.
      (Rust, 2026-09-11 — `DiscClosed` was deliberately not ported
      (`burn/mod.rs`: four Scala variants nothing constructed and nothing
      pinned). The Rust guard is structural: the planner targets a new disc,
      and the identity check refuses the sealed disc live:
      `Not the expected disc: found a finalized (closed) disc. Swap discs and
      confirm again.` → re-prompt → `Aborted.`, exit 0.)
- [x] 8. Empty tray: run `burn` with no disc; record the exact failure text:
      NOT a failure — xorriso exits 0 with `Media status : is not present`
      (captured 2026-08-30). Fixed in `burn::media`'s parser
      (RawStatus::NotPresent → MediaStatus::NoMedia);
      `burn::xorriso::parse::classify_failure` unchanged.
      (Rust, 2026-09-11 — re-confirmed both halves: the raw xorriso probe on
      the empty tray printed `Media status : is not present`, and the CLI
      refuses pre-open via the identity check — `Not the expected disc: found
      no disc in the drive. Swap discs and confirm again.` → re-prompt →
      `Aborted.`, exit 0.)
- [ ] 9. `verify` with the disc in another machine's drive or after remount —
      optional bit-rot drill. (Not performed 2026-09-11 — no second machine.)
- [x] 10. Burn progress on a full-size disc (≥ 1 GB staged, ideally a full
      BD-R): a `burning disc <name> X/Y GB · R MB/s · ETA …` line on stderr
      advances during the write, not only at the end; no raw
      `xorriso : UPDATE` lines are printed; after 100% one `Writing done;
      closing the session …` line appears, and the close (~11 s for a sealed
      BD-R on 2026-10-11) passes without output. (#14)
      (Rust @ 4dd3482's branch build, 2026-10-11 — disc 1B of a `copies = 2`
      project, 24.5 GB on a blank BD-R, Pioneer at `/dev/sr0` via its by-id
      path: the line refreshed in place for the whole write (`burning disc 1B
      6.3/24.5 GB · 13 MB/s · ETA 22m47s` … `24.5/24.5 GB`), then exactly
      `Writing done; closing the session and finalizing disc 1B. This can take
      a minute.`, no `UPDATE` lines, then `✔ Wrote disc 1B`, `Status:
      FINALIZED`, `Copies: every current file has 2 copies.`)
- [ ] 11. Verify progress on a full disc: after the insert prompt, `Mounting
      disc <name>…` appears, then a `verifying disc <name> X/Y GB · N/M files ·
      R MB/s · ETA …` line on stderr that refreshes in place, including while
      one multi-GB file is read (the GB figure climbs while the file count
      stands still). A disc with versions hidden by a later session also shows
      an `extracting earlier versions on disc <name>` line. The stdout report is
      unchanged. (#15)

Run record: 2026-09-11, Rust binary @ commit b81b342 (release build), Pioneer
BDR-UD04 at `/dev/sr1`, one CMC BD-R (POW caveat above did not apply — media
was unformatted). Items 1–8 performed and passed; the only unticked items are
0 (needs root to arrange) and 9 (optional drill, no second machine).

Findings feed parser fixes; re-run `cargo test -p diskrement-shell` after any
parser change.

