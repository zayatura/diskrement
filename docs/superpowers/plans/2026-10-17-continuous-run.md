# Continuous run Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `diskrement run NAME` plans and burns cycle after cycle until the backlog is on disc, waiting for each disc to be seated instead of asking for it.

**Architecture:** A new `burn::drive` module asks the kernel whether a disc is seated (`CDROM_DRIVE_STATUS` on a non-blocking open, which never moves the tray). `XorrisoBurner` gains a detect mode that replaces the insert prompt with "poll the kernel, then run the existing strict identity check once per insertion". The CLI's new `run` module loops over the existing `Pipeline::plan` and `Pipeline::burn_with`, with its stop rules as pure functions. `Pipeline` itself is unchanged except for one switch on `VerifyAfterBurn` that stops a failed first session instead of retrying it.

**Tech Stack:** Rust 1.98.1 workspace, `rustix` 1.1 (already a dependency; its `ioctl` module is ungated), clap, the existing scripted-exec testkit.

**Spec:** `docs/superpowers/specs/2026-10-17-continuous-run-design.md`. **Branch:** `feat/continuous-run` (already exists, holds the spec).

**Verification, every task:** `cargo test -p <crate touched>`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`. **Read clippy's output before committing**; never chain `clippy … ; git commit`.

---

## File map

| File | Change |
|---|---|
| `crates/diskrement-shell/src/burn/drive.rs` | **New.** `DriveState`, `DriveWatch`, `SysDriveWatch` (the ioctl). |
| `crates/diskrement-shell/src/burn/mod.rs` | `pub mod drive;`, `BurnError::DriveStatusUnavailable`. |
| `crates/diskrement-shell/src/burn/testkit.rs` | `ScriptedWatch`, `NOT_PRESENT`. |
| `crates/diskrement-shell/src/error.rs` | Exit-6 table gains the new variant. |
| `crates/diskrement-shell/src/burn/xorriso/burner.rs` | `Mismatch`, `detecting_media`, `detect_media`. |
| `crates/diskrement-shell/src/pipeline/read_back.rs` | `VerifyAfterBurn::retry_failed_first_session`. |
| `crates/diskrement-shell/src/pipeline/read_back_tests.rs` | The two literals, plus one test. |
| `crates/diskrement-cli/src/args.rs` | `Command::Run { name, yes }`. |
| `crates/diskrement-cli/src/render.rs` | `burn_discs`, split out of `burn_report`. |
| `crates/diskrement-cli/src/run.rs` | **New.** Stop rules, renderings, the loop. |
| `crates/diskrement-cli/src/main.rs` | `mod run;`, the `Run` dispatch, `retry_failed_first_session: true` for `burn`. |
| `crates/diskrement-cli/src/prompt.rs` | Doc comment on `ConsoleOperator`. |
| `crates/diskrement-cli/tests/scenarios/s17_run.rs` + `main.rs` | **New** scenario. |
| `crates/diskrement-cli/tests/binary.rs` | Two contract tests. |
| `CLAUDE.md`, `docs/smoke-test-real-media.md` | Gotcha, invariant, smoke item 14. |

---

### Task 0: Baseline

- [ ] **Step 1: Record the test count before any change**

Run: `DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | grep -E "^test result" | awk '{s+=$4} END {print s}'`
Expected: one number (about 730). Note it at the top of `## Execution notes` (Task 11) so the final count can be compared with it.

---

### Task 1: `DriveState`, `DriveWatch` and the kernel watch

**Files:**
- Create: `crates/diskrement-shell/src/burn/drive.rs`
- Modify: `crates/diskrement-shell/src/burn/mod.rs` (module list near line 9; `BurnError` before `Aborted`, line ~214)
- Modify: `crates/diskrement-shell/src/burn/testkit.rs` (append)
- Modify: `crates/diskrement-shell/src/error.rs` (`the_media_and_tool_errors_are_exit_6_with_their_scala_messages`, line ~758)

- [ ] **Step 1: Add the error variant and its exit-6 test case**

In `burn/mod.rs`, insert before `#[error("aborted by operator")]`:

```rust
    /// `run` could not ask the kernel whether a disc is seated
    /// (continuous-run spec §4.2): not an optical drive, or one that does not
    /// answer `CDROM_DRIVE_STATUS`. A missing device or a permission problem
    /// keeps its own variant.
    #[error("cannot read the drive status of {device}: {reason}")]
    DriveStatusUnavailable { device: String, reason: String },
```

In `error.rs`'s `cases` vector, after the `NoMedia` case, add:

```rust
            (
                BurnError::DriveStatusUnavailable {
                    device: "/dev/sr0".into(),
                    reason: "Inappropriate ioctl for device".into(),
                },
                "cannot read the drive status of /dev/sr0: Inappropriate ioctl for device",
            ),
```

- [ ] **Step 2: Write `drive.rs` with its tests**

```rust
//! Whether the drive holds a disc, asked of the kernel (continuous-run spec
//! §4). xorriso closes the tray whenever it acquires the drive, so it can
//! never be what polls. `CDROM_DRIVE_STATUS` on a non-blocking open neither
//! moves the tray nor reads the media.

use std::ffi::c_void;
use std::time::Duration;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::ioctl::{Ioctl, IoctlOutput, Opcode};

use crate::burn::BurnError;

/// What the kernel says about the drive, `CDS_*` in `<linux/cdrom.h>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveState {
    NoInfo,
    NoDisc,
    TrayOpen,
    NotReady,
    DiscOk,
}

impl DriveState {
    /// Anything the header does not define reads as `NoInfo`: keep polling.
    pub fn from_cdrom_status(status: i32) -> DriveState {
        match status {
            1 => DriveState::NoDisc,
            2 => DriveState::TrayOpen,
            3 => DriveState::NotReady,
            4 => DriveState::DiscOk,
            _ => DriveState::NoInfo,
        }
    }
}

/// The port `XorrisoBurner`'s detect mode polls. `pause` is the wait between
/// two polls, so a scripted watch can make it free.
pub trait DriveWatch {
    fn state(&self) -> Result<DriveState, BurnError>;
    fn pause(&self);
}

const CDROM_DRIVE_STATUS: Opcode = 0x5326;
/// `CDSL_CURRENT`, `(int)(~0U >> 1)`: the slot currently loaded.
const CDSL_CURRENT: usize = i32::MAX as usize;
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// `ioctl(fd, CDROM_DRIVE_STATUS, CDSL_CURRENT)`: the slot goes in by value
/// and the status comes back as the return value.
struct DriveStatus;

// SAFETY: the argument is an integer passed by value, the kernel writes no
// userspace memory, and the opcode is this ioctl's alone.
unsafe impl Ioctl for DriveStatus {
    type Output = IoctlOutput;
    const IS_MUTATING: bool = false;

    fn opcode(&self) -> Opcode {
        CDROM_DRIVE_STATUS
    }

    fn as_ptr(&mut self) -> *mut c_void {
        std::ptr::without_provenance_mut(CDSL_CURRENT)
    }

    unsafe fn output_from_ptr(out: IoctlOutput, _: *mut c_void) -> rustix::io::Result<IoctlOutput> {
        Ok(out)
    }
}

/// The real watch. Each poll opens and closes the device, so no descriptor
/// is held while xorriso runs, and a `/dev/disk/by-id` symlink is followed
/// afresh every time.
pub struct SysDriveWatch {
    device: String,
}

impl SysDriveWatch {
    pub fn new(device: impl Into<String>) -> Self {
        SysDriveWatch {
            device: device.into(),
        }
    }

    fn unavailable(&self, e: Errno) -> BurnError {
        BurnError::DriveStatusUnavailable {
            device: self.device.clone(),
            reason: e.to_string(),
        }
    }
}

impl DriveWatch for SysDriveWatch {
    fn state(&self) -> Result<DriveState, BurnError> {
        let fd = rustix::fs::open(
            self.device.as_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| match e {
            Errno::NOENT => BurnError::DeviceNotFound(self.device.clone()),
            Errno::ACCESS | Errno::PERM => BurnError::DevicePermissionDenied(self.device.clone()),
            other => self.unavailable(other),
        })?;
        // SAFETY: see `DriveStatus`.
        let status = unsafe { rustix::ioctl::ioctl(&fd, DriveStatus) }
            .map_err(|e| self.unavailable(e))?;
        Ok(DriveState::from_cdrom_status(status))
    }

    fn pause(&self) {
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cdrom_status_values_map_to_drive_states() {
        assert_eq!(DriveState::from_cdrom_status(0), DriveState::NoInfo);
        assert_eq!(DriveState::from_cdrom_status(1), DriveState::NoDisc);
        assert_eq!(DriveState::from_cdrom_status(2), DriveState::TrayOpen);
        assert_eq!(DriveState::from_cdrom_status(3), DriveState::NotReady);
        assert_eq!(DriveState::from_cdrom_status(4), DriveState::DiscOk);
        assert_eq!(DriveState::from_cdrom_status(99), DriveState::NoInfo);
    }

    #[test]
    fn a_missing_device_is_device_not_found() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sr9");
        let err = SysDriveWatch::new(path.display().to_string())
            .state()
            .unwrap_err();
        assert!(matches!(&err, BurnError::DeviceNotFound(d) if d.ends_with("sr9")), "{err:?}");
    }

    /// A regular file opens fine but is no drive: the ioctl is refused
    /// (`ENOTTY`), and the error names the device.
    #[test]
    fn a_file_that_is_not_a_drive_cannot_be_watched() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let shown = file.path().display().to_string();
        let err = SysDriveWatch::new(shown.clone()).state().unwrap_err();
        assert!(
            matches!(&err, BurnError::DriveStatusUnavailable { device, .. } if *device == shown),
            "{err:?}"
        );
    }
}
```

In `burn/mod.rs`, add `pub mod drive;` between `pub mod cmd;` and `pub mod json;`.

- [ ] **Step 3: Add `ScriptedWatch` and `NOT_PRESENT` to the testkit**

Append to `burn/testkit.rs` (add `use std::cell::Cell;` next to the `RefCell` import, and `use crate::burn::drive::{DriveState, DriveWatch};`):

```rust
/// `-toc` output for a drive xorriso finds empty.
pub const NOT_PRESENT: &[&str] = &["Media status : is not present"];

/// A `DriveWatch` answering from a queue. Running out panics, so a wait loop
/// that polls more than the test expects fails instead of hanging; `failing`
/// answers every poll with `DriveStatusUnavailable`.
pub struct ScriptedWatch {
    states: RefCell<VecDeque<DriveState>>,
    fail: bool,
    pauses: Cell<usize>,
}

impl ScriptedWatch {
    pub fn new(states: &[DriveState]) -> Self {
        ScriptedWatch {
            states: RefCell::new(states.iter().copied().collect()),
            fail: false,
            pauses: Cell::new(0),
        }
    }

    pub fn failing() -> Self {
        ScriptedWatch {
            fail: true,
            ..ScriptedWatch::new(&[])
        }
    }

    pub fn pauses(&self) -> usize {
        self.pauses.get()
    }
}

impl DriveWatch for ScriptedWatch {
    fn state(&self) -> Result<DriveState, BurnError> {
        if self.fail {
            return Err(BurnError::DriveStatusUnavailable {
                device: "/dev/sr0".to_string(),
                reason: "Inappropriate ioctl for device".to_string(),
            });
        }
        Ok(self
            .states
            .borrow_mut()
            .pop_front()
            .expect("the script ran out of drive states"))
    }

    fn pause(&self) {
        self.pauses.set(self.pauses.get() + 1);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell burn::drive && cargo test -p diskrement-shell error::`
Expected: the three `drive` tests and the exit-6 table pass. If `without_provenance_mut` or an `Errno` constant does not resolve, check the rustix 1.1.4 source under `~/.cargo/registry/src/*/rustix-1.1.4/src/ioctl/` before improvising.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings   # read the output
cargo fmt --check
git add crates/diskrement-shell/src/burn/drive.rs crates/diskrement-shell/src/burn/mod.rs \
        crates/diskrement-shell/src/burn/testkit.rs crates/diskrement-shell/src/error.rs
git commit -m "feat(burn): ask the kernel whether a disc is seated, without touching the tray"
```

---

### Task 2: `check_media` tells "no disc" from "the wrong disc"

The detect loop must re-inspect when xorriso sees no disc that the kernel already reports (a drive spinning up), but wait for the tray to open on a wrong disc. Today both are a `String`.

**Files:**
- Modify: `crates/diskrement-shell/src/burn/xorriso/burner.rs` (`check_media`, line ~128; its caller in `await_correct_media`)

- [ ] **Step 1: Introduce `Mismatch`**

Above `impl<'a> XorrisoBurner<'a>`, add:

```rust
/// Why the disc in the drive is not the one to write.
enum Mismatch {
    /// xorriso finds the drive empty.
    NoDisc,
    /// Some other disc, worded for the operator.
    Wrong(String),
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::NoDisc => f.write_str("no disc in the drive"),
            Mismatch::Wrong(found) => f.write_str(found),
        }
    }
}
```

Change `check_media`'s signature to `-> Result<Result<u64, Mismatch>, BurnError>`. Its body: the `NoMedia` arm becomes `Err(Mismatch::NoDisc)`, and every other `Err(…)` arm wraps its string in `Mismatch::Wrong(…)`, for example `Err(Mismatch::Wrong("a blank disc (expected written media to append to)".to_string()))`. The prompt loop's `Err(found) => … format!("Not the expected disc: found {found}. …")` keeps working through `Display`.

- [ ] **Step 2: Run the burner tests unchanged**

Run: `cargo test -p diskrement-shell burn::xorriso::burner`
Expected: all pass, including `session_one_refuses_written_media_and_succeeds_on_the_swap`: the wording is unchanged.

- [ ] **Step 3: Clippy, fmt, commit**

```bash
git add crates/diskrement-shell/src/burn/xorriso/burner.rs
git commit -m "refactor(burn): the media check tells an empty drive from the wrong disc"
```

---

### Task 3: `XorrisoBurner` detect mode

**Files:**
- Modify: `crates/diskrement-shell/src/burn/xorriso/burner.rs`

- [ ] **Step 1: Write the failing tests**

In the `tests` module, extend the testkit import with `NOT_PRESENT, ScriptedWatch`, add `use crate::burn::drive::DriveState::{self, *};`, and add:

```rust
    const WAITING_1: &str =
        "Waiting for a blank disc for 'photos' disc 1 in /dev/sr0 (Ctrl-C to stop; nothing is written yet).";

    /// A detecting burner on /dev/sr0 over `script`, burning `image`.
    fn detect_burn(
        watch: &ScriptedWatch,
        script: &Script,
        op: &ScriptedOperator,
        image: &DiscImage,
    ) -> Result<BurnResult, BurnError> {
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        XorrisoBurner::new(
            BurnDevice::RealDevice("/dev/sr0".to_string()),
            &exec,
            op,
            &NoopSink,
            2,
        )
        .detecting_media(watch)
        .burn(image)
    }

    /// A blank already seated is burned with no question asked: the
    /// operator has no answers, so any `confirm_insert` would abort.
    #[test]
    fn a_seated_blank_is_burned_without_asking() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::new(&[DiscOk]);
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        assert_eq!(op.log()[0], WAITING_1);
        assert_eq!(watch.pauses(), 0);
    }

    /// Only a seated disc is inspected: the script holds one `-toc`, so a
    /// second inspection would panic.
    #[test]
    fn the_wait_polls_through_an_open_tray_and_an_empty_drive() {
        let dir = tempfile::TempDir::new().unwrap();
        let states: &[DriveState] = &[TrayOpen, NoDisc, NotReady, NoInfo, DiscOk];
        let watch = ScriptedWatch::new(states);
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        assert_eq!(watch.pauses(), 4);
    }

    /// A wrong disc is inspected once and named once; the next inspection
    /// waits until the tray has been opened and a disc seated again.
    #[test]
    fn a_wrong_disc_is_inspected_once_and_named_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let states: &[DriveState] = &[DiscOk, DiscOk, DiscOk, TrayOpen, NoDisc, DiscOk];
        let watch = ScriptedWatch::new(states);
        let script = Script::new()
            .toc(&[APPENDABLE, BLANK])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_1)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the swapped-in blank is burned");
        let found: Vec<String> = op.log().into_iter().filter(|l| l.starts_with("Found")).collect();
        assert_eq!(
            found,
            vec!["Found written media (expected a blank disc); waiting for a blank disc for 'photos' disc 1."]
        );
        assert_eq!(watch.pauses(), 3);
    }

    /// The kernel can report a disc before xorriso sees one. That is not a
    /// wrong disc: look again rather than wait for the tray to open.
    #[test]
    fn xorriso_not_seeing_the_disc_yet_looks_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::new(&[DiscOk, DiscOk]);
        let script = Script::new()
            .toc(&[NOT_PRESENT, BLANK])
            .identities(&[Some(IDENTITY_1)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .expect("the burn succeeds");
        assert!(!op.said("Found"), "{:?}", op.log());
        assert_eq!(watch.pauses(), 1);
    }

    #[test]
    fn an_append_waits_for_this_disc_at_the_previous_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::new(&[DiscOk]);
        let script = Script::new()
            .toc(&[APPENDABLE])
            .identities(&[Some(IDENTITY_1), Some(IDENTITY_2)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 2, &[("a.txt", "hello")], false))
            .expect("the append succeeds");
        assert_eq!(
            op.log()[0],
            "Waiting for disc 1 of 'photos' (last burned session 1) in /dev/sr0 \
             (Ctrl-C to stop; nothing is written yet)."
        );
    }

    #[test]
    fn the_waiting_line_names_the_seal() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::new(&[DiscOk]);
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let op = ScriptedOperator::new(&[]);
        detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], true))
            .expect("the burn succeeds");
        assert_eq!(op.log()[0], format!("{WAITING_1} This burn will FINALIZE the disc."));
    }

    /// An image file has no drive to watch: it is asked for as before, and
    /// the watch (which would panic) is never polled.
    #[test]
    fn an_image_file_is_never_polled() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::new(&[]);
        let script = Script::new().toc(&[BLANK]).identities(&[Some(IDENTITY_1)]);
        let exec = |c: &[String], l: &mut dyn FnMut(&str)| script.respond(c, l);
        let op = ScriptedOperator::new(&[true]);
        XorrisoBurner::new(
            BurnDevice::ImageFile(dir.path().join("d.iso")),
            &exec,
            &op,
            &NoopSink,
            2,
        )
        .detecting_media(&watch)
        .burn(&image_of(dir.path(), 1, &[("a.txt", "hello")], false))
        .expect("the burn succeeds");
        assert!(op.log()[0].starts_with("Insert"), "{:?}", op.log());
    }

    #[test]
    fn a_drive_that_cannot_be_watched_fails_before_any_inspection() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch = ScriptedWatch::failing();
        let script = Script::new().toc(&[]).identities(&[]);
        let op = ScriptedOperator::new(&[]);
        let err = detect_burn(&watch, &script, &op, &image_of(dir.path(), 1, &[("a.txt", "hello")], false))
            .unwrap_err();
        assert!(matches!(err, BurnError::DriveStatusUnavailable { .. }), "{err:?}");
        assert!(!script.ran("-toc"));
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p diskrement-shell burn::xorriso::burner`
Expected: compile error, no method `detecting_media`.

- [ ] **Step 3: Implement detect mode**

Add `use crate::burn::drive::{DriveState, DriveWatch};`. Give the struct a field `watch: Option<&'a dyn DriveWatch>,` (with the doc comment `/// \`run\`: wait for the right disc instead of asking for it (\`detecting_media\`).`), initialised to `None` in `new`. Add the builder after `keeping_disc_in_drive`:

```rust
    /// `run` (continuous-run spec §4.3): instead of asking for the disc, wait
    /// until the kernel reports one seated, then check it as the prompt
    /// would. An image file has no drive and is still asked for.
    pub fn detecting_media(mut self, watch: &'a dyn DriveWatch) -> Self {
        self.watch = Some(watch);
        self
    }
```

Replace `await_correct_media` with:

```rust
    /// Get the right disc into the drive and return its free space: asked for
    /// and inspected until it is right (a decline is `Aborted`; the wrong
    /// disc re-prompts forever, which is what an operator swapping discs
    /// actually wants), or, in detect mode, waited for.
    fn await_correct_media(&self, image: &DiscImage) -> Result<u64, BurnError> {
        let id = &image.metadata.identity;
        let expected = if id.session_ordinal == 1 {
            format!(
                "a blank disc for '{}' disc {}",
                id.project_name,
                id.disc_name()
            )
        } else {
            format!(
                "disc {} of '{}' (last burned session {})",
                id.disc_name(),
                id.project_name,
                id.session_ordinal - 1
            )
        };
        let seal_note = if image.close_disc {
            " This burn will FINALIZE the disc."
        } else {
            ""
        };
        let address = self.device.xorriso_address();
        if let (Some(watch), BurnDevice::RealDevice(_)) = (self.watch, &self.device) {
            self.operator.progress(&format!(
                "Waiting for {expected} in {address} (Ctrl-C to stop; nothing is written yet).{seal_note}"
            ));
            return self.detect_media(watch, id, &expected);
        }
        let prompt = format!("Insert {expected} into {address} and confirm.{seal_note}");

        loop {
            if !self.operator.confirm_insert(&prompt) {
                return Err(BurnError::Aborted);
            }
            match self.check_media(id)? {
                Ok(free_bytes) => return Ok(free_bytes),
                Err(found) => self.operator.progress(&format!(
                    "Not the expected disc: found {found}. Swap discs and confirm again."
                )),
            }
        }
    }

    /// Detect mode's wait. xorriso runs only once the kernel reports a disc
    /// seated, so it never pulls the tray in on an operator swapping discs,
    /// and a wrong disc is inspected and named once per insertion: the next
    /// inspection waits for the tray to open.
    fn detect_media(
        &self,
        watch: &dyn DriveWatch,
        id: &DiscIdentity,
        expected: &str,
    ) -> Result<u64, BurnError> {
        loop {
            while watch.state()? != DriveState::DiscOk {
                watch.pause();
            }
            match self.check_media(id)? {
                Ok(free_bytes) => return Ok(free_bytes),
                // The kernel saw the disc before xorriso could: look again.
                Err(Mismatch::NoDisc) => watch.pause(),
                Err(found) => {
                    self.operator
                        .progress(&format!("Found {found}; waiting for {expected}."));
                    while watch.state()? == DriveState::DiscOk {
                        watch.pause();
                    }
                }
            }
        }
    }
```

Update the module doc's first line to "Get the disc (asked for, or waited for under `run`), inspect it (strict identity check), stage with re-hash, …".

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-shell burn::xorriso::burner`
Expected: all pass, old and new.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
git add crates/diskrement-shell/src/burn/xorriso/burner.rs
git commit -m "feat(burn): a detecting burner waits for the right disc instead of asking for it"
```

---

### Task 4: `run` does not retry a failed first session

**Files:**
- Modify: `crates/diskrement-shell/src/pipeline/read_back.rs` (struct line 14; loop line ~95)
- Modify: `crates/diskrement-shell/src/pipeline/read_back_tests.rs` (literals at lines ~115 and ~134; new test after `declining_the_retry_stops_with_verify_failed_and_records_nothing`)
- Modify: `crates/diskrement-cli/src/main.rs` (literal at line ~237)

- [ ] **Step 1: Write the failing test**

In `read_back_tests.rs`, add `retry_failed_first_session: true,` to both existing `VerifyAfterBurn` literals, then add:

```rust
/// `run` (continuous-run spec §5): a failed first session stops at once.
/// Under detection a retry would just wait for the next blank and carry on.
#[test]
fn without_retries_a_failed_first_session_stops_at_once() {
    let mut env = env(1);
    stage(&mut env, &[("a.bin", b"aaaa")]);
    let tree = env.work.join("staging/disc-1");
    let script = ScriptedExec::new()
        .toc(&[BLANK, APPENDABLE])
        .identities(&[Some(ID_1), Some(ID_1)])
        .mounts(&[Some(&tree)]);
    let calls = RefCell::new(Vec::new());
    let rec = recording(&script, &calls);
    let exec = |c: &[String], l: &mut dyn FnMut(&str)| {
        if c.iter().any(|a| a == "-commit") {
            std::fs::write(tree.join("data/a.bin"), b"rot!").unwrap();
        }
        rec.run(c, l)
    };
    let op = ScriptedOperator::new(&[true]);
    let device = BurnDevice::RealDevice("/dev/sr0".to_string());
    let burner = Burner::Real(XorrisoBurner::new(device.clone(), &exec, &op, &NoopSink, 2));
    let verifier = Verifier::Disc(DiscVerifier::new(device, &exec, &op, &NoopSink));
    let step = VerifyAfterBurn {
        verifier: &verifier,
        operator: &op,
        retry_failed_first_session: false,
    };

    let err = pipeline(&mut env)
        .burn_with("p", now(), &burner, Some(&step))
        .unwrap_err();

    assert!(
        matches!(&err, PipelineError::VerifyFailed { session: 1, appended: false, .. }),
        "{err:?}"
    );
    assert!(!op.said("To retry"), "{:?}", op.log());
    assert_eq!(
        calls.borrow().iter().filter(|c| c.contains("-commit")).count(),
        1,
        "burned once, never retried"
    );
    assert!(sessions(&env, SessionState::Burned).is_empty());
    assert!(script.ran("-eject"), "the failed disc is ejected");
}
```

(`rec.run(c, l)` is `Exec::run`; `Exec` is already imported in this file.)

In `main.rs`, add `retry_failed_first_session: true,` to the `burn` command's literal.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -p diskrement-shell without_retries`
Expected: compile error, no field `retry_failed_first_session`.

- [ ] **Step 3: Implement**

In `read_back.rs`, add the field to `VerifyAfterBurn`:

```rust
    /// Offer a failed first session again on a new blank disc. `burn
    /// --verify` does; `run` stops instead (continuous-run spec §5), because
    /// under disc detection the retry would just wait for the next blank and
    /// the run would carry on past a failure.
    pub retry_failed_first_session: bool,
```

and in `burn_and_read_back` replace `if session > 1 {` with `if session > 1 || !step.retry_failed_first_session {`. Update the function's doc comment: "A failed first session is offered again on a new blank disc when `step` allows it, through the burner's own prompt …".

- [ ] **Step 4: Run the shell and CLI tests**

Run: `cargo test -p diskrement-shell pipeline:: && cargo build -p diskrement-cli`
Expected: all pass; the CLI builds.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
git add crates/diskrement-shell/src/pipeline/read_back.rs \
        crates/diskrement-shell/src/pipeline/read_back_tests.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(burn): a read-back step can stop a failed first session instead of retrying it"
```

---

### Task 5: `burn_discs`, the per-disc half of the burn report

`run` prints each cycle's discs (their labels are what the operator writes on them). The full report's "Run: plan → burn" advice would be wrong mid-run, so it is printed only at the end.

**Files:**
- Modify: `crates/diskrement-cli/src/render.rs` (`burn_report`, line ~168)

- [ ] **Step 1: Write the failing test**

In `render.rs`'s tests, next to `a_two_copy_burn_lists_both_discs_and_the_coverage`:

```rust
    /// `run`'s per-cycle output: the disc blocks of the burn report, without
    /// its coverage or its "Run: plan → burn" advice.
    #[test]
    fn burn_discs_is_the_disc_part_of_the_burn_report() {
        let r = BurnReport {
            discs: vec![DiscBurn {
                disc: DiscKey::new(5, 1),
                label: "immich-dsz disc 5B".to_string(),
                finalized: true,
                staging_dir: "/w/staging".into(),
                image_bytes: 1,
                read_back: Some(DiscReadBack::Verified {
                    files: 9042,
                    through_xorriso: true,
                }),
            }],
            remaining: RemainingEstimate {
                more_discs: 9,
                more_files: 1,
                more_bytes: 1,
            },
            coverage: diskrement_core::Coverage {
                sets: vec![SetCoverage::default(); 2],
                no_live_copy: SetCoverage::default(),
            },
            deferred_deletions: None,
        };
        assert_eq!(
            burn_discs(&r, 2),
            "✔ Wrote disc 5B  —  label: \"immich-dsz disc 5B\"\n\
             Status: FINALIZED — set this disc aside, it is sealed.\n\
             Read back: verified (9042 files, read through xorriso)."
        );
        assert!(burn_report(&r, 2).starts_with(&burn_discs(&r, 2)));
    }
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -p diskrement-cli render::tests::burn_discs`
Expected: compile error, no function `burn_discs`.

- [ ] **Step 3: Split `burn_report`**

Move the `for d in &r.discs { … }` loop out of `burn_report` into:

```rust
/// One block per disc written: the line with its label, its status and, under
/// `burn --verify`, its read-back. `run` prints these after every cycle.
pub fn burn_discs(r: &BurnReport, copies: u32) -> String {
    disc_lines(r, copies).join("\n")
}

fn disc_lines(r: &BurnReport, copies: u32) -> Vec<String> {
    let mut lines = Vec::new();
    for d in &r.discs {
        // the loop body, unchanged
    }
    lines
}
```

In `burn_report`, replace the loop with `lines.extend(disc_lines(r, copies));`. The "Nothing burned" lines stay before it, so the order is unchanged.

- [ ] **Step 4: Run all CLI tests**

Run: `cargo test -p diskrement-cli`
Expected: all pass, the `burn.snap` snapshots included.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
git add crates/diskrement-cli/src/render.rs
git commit -m "refactor(cli): the burn report's disc blocks render on their own"
```

---

### Task 6: `run NAME [--yes]` parses

**Files:**
- Modify: `crates/diskrement-cli/src/args.rs` (`Command`, after `Burn`; tests)
- Modify: `crates/diskrement-cli/src/main.rs` (`project_name` match, line ~84; a stub dispatch arm)

- [ ] **Step 1: Write the failing test**

In `args.rs`'s tests:

```rust
    #[test]
    fn run_takes_a_name_and_an_optional_yes() {
        let parse = |argv: &[&str]| match Cli::try_parse_from(argv).unwrap().command {
            Command::Run { name, yes } => (name, yes),
            _ => panic!("not run"),
        };
        assert_eq!(parse(&["diskrement", "run", "p"]), ("p".to_string(), false));
        assert_eq!(parse(&["diskrement", "run", "p", "--yes"]), ("p".to_string(), true));
        assert_eq!(parse(&["diskrement", "run", "p", "-y"]), ("p".to_string(), true));
        assert!(Cli::try_parse_from(["diskrement", "run"]).is_err());
    }
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -p diskrement-cli args::tests::run_takes`
Expected: compile error, no variant `Run`.

- [ ] **Step 3: Add the variant**

After `Burn { … },` in `Command`:

```rust
    /// Plan and burn, cycle after cycle, until the backlog is on disc.
    ///
    /// Each cycle is `plan`, then `burn --verify`, but each disc is waited
    /// for rather than asked for: the burn starts once a disc that is exactly
    /// the one expected is in the drive. Stops at the first error, and after
    /// the cycle that leaves its discs open. Ctrl-C while waiting for a disc
    /// stops it with nothing written.
    Run {
        name: String,
        /// Skip the one confirmation at the start.
        #[arg(long, short = 'y', default_value_t = false)]
        yes: bool,
    },
```

In `main.rs`'s `project_name` match, add `Command::Run { name, .. } => name,`. In `dispatch`, add a temporary arm that Task 8 replaces:

```rust
        Command::Run { .. } => Err(PipelineError::Failed("run is not wired yet".to_string())),
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p diskrement-cli args::`
Expected: pass.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
git add crates/diskrement-cli/src/args.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): run NAME [--yes] parses"
```

---

### Task 7: `run`'s stop rules and renderings

**Files:**
- Create: `crates/diskrement-cli/src/run.rs`
- Modify: `crates/diskrement-cli/src/main.rs` (`mod run;` after `mod render;`)

- [ ] **Step 1: Write `run.rs` with the pure parts and their tests**

```rust
//! `diskrement run`: plan and burn cycle after cycle until the backlog is on
//! disc, each disc waited for rather than asked for (continuous-run spec).
//! The stop rules and the renderings are pure; `backlog` is the loop.

use std::time::Duration;

use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::reports::{BurnReport, DiscBurn, DiscReadBack, PlanReport};

use crate::render;

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Continue,
    Done,
}

/// A plan with no disc: nothing left to burn.
pub fn after_plan(plan: &PlanReport) -> Next {
    if plan.sets.is_empty() {
        Next::Done
    } else {
        Next::Continue
    }
}

/// A burned cycle that finalized nothing left its discs open: everything
/// left fitted, and what comes later is appended by ordinary cycles.
pub fn after_burn(burn: &BurnReport) -> Next {
    if burn.discs.iter().any(|d| d.finalized) {
        Next::Continue
    } else {
        Next::Done
    }
}

/// Cycles after this one: the longest spill among the sets.
fn more_cycles(plan: &PlanReport) -> usize {
    plan.sets
        .iter()
        .map(|s| s.remaining.more_discs)
        .max()
        .unwrap_or(0)
}

fn cycles(n: usize) -> String {
    if n == 1 {
        "1 cycle".to_string()
    } else {
        format!("{n} cycles")
    }
}

/// The one question a run asks, before its first burn.
pub fn confirm_question(name: &str, plan: &PlanReport) -> String {
    format!(
        "Burn the backlog of {name} (about {})?",
        cycles(1 + more_cycles(plan))
    )
}

/// `Cycle 3: 7A (24.5 GB, finalizes), 7B (24.5 GB, finalizes); about 10 more
/// cycles after this.`
pub fn cycle_line(n: usize, plan: &PlanReport) -> String {
    let discs: Vec<String> = plan
        .sets
        .iter()
        .map(|s| {
            format!(
                "{} ({}, {})",
                s.disc.key().name(plan.copies),
                render::gb(s.disc.total_used_bytes()),
                if s.disc.will_finalize {
                    "finalizes"
                } else {
                    "stays open"
                }
            )
        })
        .collect();
    let tail = match more_cycles(plan) {
        0 => "the last cycle".to_string(),
        1 => "about 1 more cycle after this".to_string(),
        more => format!("about {more} more cycles after this"),
    };
    format!("Cycle {n}: {}; {tail}.", discs.join(", "))
}

/// `1h 32m`, `12m`, `45s`.
pub fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// What the run burned, printed when it ends, however it ends.
pub fn summary(burned: &[DiscBurn], copies: u32, took: Duration) -> String {
    if burned.is_empty() {
        return format!("This run: no disc burned ({}).", elapsed(took));
    }
    let names = |want: fn(&Option<DiscReadBack>) -> bool| -> Vec<String> {
        burned
            .iter()
            .filter(|d| want(&d.read_back))
            .map(|d| d.disc.name(copies))
            .collect()
    };
    let verified = names(|r| matches!(r, Some(DiscReadBack::Verified { .. })));
    let unverified = names(|r| matches!(r, Some(DiscReadBack::Unverified)));
    let unchecked = names(|r| r.is_none());
    let mut parts = Vec::new();
    if !verified.is_empty() {
        parts.push(format!("verified: {}", verified.join(", ")));
    }
    if !unverified.is_empty() {
        let advice: Vec<String> = unverified
            .iter()
            .map(|n| format!("verify --disc {n}"))
            .collect();
        parts.push(format!(
            "recorded unverified: {} (run {})",
            unverified.join(", "),
            advice.join(", ")
        ));
    }
    if !unchecked.is_empty() {
        parts.push(format!("written: {}", unchecked.join(", ")));
    }
    let count = if burned.len() == 1 {
        "1 disc".to_string()
    } else {
        format!("{} discs", burned.len())
    };
    format!("This run: {count} in {} — {}.", elapsed(took), parts.join("; "))
}

/// The line `run` adds after a burn error: re-running `run` re-plans, so it
/// picks up where it stopped. Not after a failed append, whose disc must be
/// marked dead first.
pub fn rerun_advice(name: &str, err: &PipelineError) -> Option<String> {
    let cause = match err {
        PipelineError::PartialBurn { cause, .. } => cause.as_ref(),
        other => other,
    };
    match cause {
        PipelineError::VerifyFailed { appended: true, .. } => None,
        _ => Some(format!(
            "Or re-run diskrement run {name}; it re-plans first."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::{Coverage, DiscKey, DiscType, PlannedDisc};
    use diskrement_shell::pipeline::reports::{RemainingEstimate, SetPlan};

    fn set(ordinal: u32, copy_set: u32, will_finalize: bool, more_discs: usize) -> SetPlan {
        SetPlan {
            disc: PlannedDisc {
                ordinal,
                copy_set,
                is_new: true,
                will_finalize,
                files: vec![],
                prior_used_bytes: 24_500_000_000,
                usable_bytes: 24_500_000_000,
            },
            catch_up: 0,
            session_ordinal: 1,
            remaining: RemainingEstimate {
                more_discs,
                more_files: 0,
                more_bytes: 0,
            },
        }
    }

    fn plan(sets: Vec<SetPlan>) -> PlanReport {
        PlanReport {
            copies: 2,
            sets,
            disc_type: DiscType::BdR,
            tombstones: vec![],
            oversized: vec![],
            unplaceable_catch_up: vec![],
        }
    }

    fn burned(ordinal: u32, copy_set: u32, finalized: bool, read_back: Option<DiscReadBack>) -> DiscBurn {
        DiscBurn {
            disc: DiscKey::new(ordinal, copy_set),
            label: String::new(),
            finalized,
            staging_dir: "/w/staging".into(),
            image_bytes: 1,
            read_back,
        }
    }

    fn report(discs: Vec<DiscBurn>) -> BurnReport {
        BurnReport {
            discs,
            remaining: RemainingEstimate {
                more_discs: 0,
                more_files: 0,
                more_bytes: 0,
            },
            coverage: Coverage {
                sets: vec![],
                no_live_copy: Default::default(),
            },
            deferred_deletions: None,
        }
    }

    const OK: Option<DiscReadBack> = Some(DiscReadBack::Verified {
        files: 1,
        through_xorriso: false,
    });

    #[test]
    fn a_plan_with_no_disc_ends_the_run() {
        assert_eq!(after_plan(&plan(vec![])), Next::Done);
        assert_eq!(after_plan(&plan(vec![set(5, 0, true, 9)])), Next::Continue);
    }

    #[test]
    fn a_cycle_that_finalizes_nothing_ends_the_run() {
        let open = report(vec![burned(18, 0, false, OK), burned(18, 1, false, OK)]);
        assert_eq!(after_burn(&open), Next::Done);
        let sealed = report(vec![burned(5, 0, true, OK), burned(5, 1, true, OK)]);
        assert_eq!(after_burn(&sealed), Next::Continue);
        // A lagging set still sealing a disc keeps the run going.
        let mixed = report(vec![burned(5, 0, false, OK), burned(4, 1, true, OK)]);
        assert_eq!(after_burn(&mixed), Next::Continue);
        assert_eq!(after_burn(&report(vec![])), Next::Done);
    }

    #[test]
    fn the_question_counts_this_cycle_and_the_longest_spill() {
        let p = plan(vec![set(5, 0, true, 9), set(5, 1, true, 10)]);
        assert_eq!(
            confirm_question("immich-dsz", &p),
            "Burn the backlog of immich-dsz (about 11 cycles)?"
        );
        let last = plan(vec![set(18, 0, false, 0)]);
        assert_eq!(
            confirm_question("p", &last),
            "Burn the backlog of p (about 1 cycle)?"
        );
    }

    #[test]
    fn the_cycle_line_names_each_disc_and_what_is_left() {
        let p = plan(vec![set(7, 0, true, 10), set(7, 1, true, 10)]);
        assert_eq!(
            cycle_line(3, &p),
            "Cycle 3: 7A (24.5 GB, finalizes), 7B (24.5 GB, finalizes); \
             about 10 more cycles after this."
        );
        let last = plan(vec![set(18, 0, false, 0), set(18, 1, false, 0)]);
        assert_eq!(
            cycle_line(14, &last),
            "Cycle 14: 18A (24.5 GB, stays open), 18B (24.5 GB, stays open); the last cycle."
        );
    }

    #[test]
    fn elapsed_time_reads_in_hours_minutes_or_seconds() {
        assert_eq!(elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(elapsed(Duration::from_secs(12 * 60 + 5)), "12m");
        assert_eq!(elapsed(Duration::from_secs(3600 + 32 * 60)), "1h 32m");
    }

    #[test]
    fn the_summary_groups_discs_by_their_read_back() {
        let discs = vec![
            burned(5, 0, true, OK),
            burned(5, 1, true, OK),
            burned(6, 0, true, OK),
            burned(6, 1, true, Some(DiscReadBack::Unverified)),
        ];
        assert_eq!(
            summary(&discs, 2, Duration::from_secs(3 * 3600)),
            "This run: 4 discs in 3h 0m — verified: 5A, 5B, 6A; \
             recorded unverified: 6B (run verify --disc 6B)."
        );
        assert_eq!(
            summary(&[], 2, Duration::from_secs(5)),
            "This run: no disc burned (5s)."
        );
    }

    #[test]
    fn rerunning_run_is_advised_except_after_a_failed_append() {
        let failed = |appended| PipelineError::VerifyFailed {
            disc: "6B".to_string(),
            session: if appended { 2 } else { 1 },
            bad: 1,
            checked: 9,
            appended,
        };
        assert_eq!(
            rerun_advice("p", &failed(false)).as_deref(),
            Some("Or re-run diskrement run p; it re-plans first.")
        );
        assert_eq!(rerun_advice("p", &failed(true)), None);
        let partial = PipelineError::PartialBurn {
            committed: vec!["6A".to_string()],
            unrecorded: None,
            pending: vec![],
            cause: Box::new(failed(true)),
        };
        assert_eq!(rerun_advice("p", &partial), None);
    }
}
```

In `main.rs`, add `mod run;` after `mod render;`. Until Task 8 uses these functions, the binary crate will warn `dead_code`, which is an error here. Put `#![allow(dead_code)]` as the first line of `run.rs` for this commit only; Task 8 removes it.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p diskrement-cli run::`
Expected: all seven pass (`SetCoverage` derives `Default`, so `Default::default()` builds `no_live_copy`).

- [ ] **Step 3: Clippy, fmt, commit**

```bash
git add crates/diskrement-cli/src/run.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): run's stop rules, cycle line and summary"
```

---

### Task 8: The loop, wired into `dispatch`

**Files:**
- Modify: `crates/diskrement-cli/src/run.rs` (append `backlog`; remove `#![allow(dead_code)]`)
- Modify: `crates/diskrement-cli/src/main.rs` (replace the stub `Run` arm)

- [ ] **Step 1: Write `backlog`**

Append to `run.rs` (add imports `std::time::Instant`, `diskrement_shell::burn::Burner`, `diskrement_shell::pipeline::{Pipeline, VerifyAfterBurn}`, `crate::{prompt, say}`):

```rust
/// The loop (spec §3): plan, ask once, then burn and re-plan until a plan
/// has nothing to place or a cycle leaves its discs open. The first error
/// stops it. An error in the first plan is returned as usual; any later one
/// is reported here, after the summary, and becomes the exit code.
pub fn backlog(
    pipeline: &mut Pipeline<'_>,
    name: &str,
    copies: u32,
    yes: bool,
    burner: &Burner<'_>,
    step: &VerifyAfterBurn<'_>,
) -> Result<u8, PipelineError> {
    let started = Instant::now();
    let mut burned: Vec<DiscBurn> = Vec::new();
    let stop = |burned: &[DiscBurn], err: PipelineError, advice: Option<String>| -> u8 {
        say(&summary(burned, copies, started.elapsed()));
        eprintln!("{}", render::error_line(&err));
        if let Some(advice) = advice {
            eprintln!("{advice}");
        }
        err.exit_code() as u8
    };
    for cycle in 1.. {
        let outcome = match pipeline.plan(name, None, false, false) {
            Ok(outcome) => outcome,
            Err(err) if cycle == 1 => return Err(err),
            Err(err) => return Ok(stop(&burned, err, None)),
        };
        if cycle == 1 {
            say(&render::scan_report(&outcome.scan));
            say(&render::plan_report(&outcome.plan));
        }
        if after_plan(&outcome.plan) == Next::Done {
            if cycle > 1 {
                say(&render::plan_report(&outcome.plan));
                say(&summary(&burned, copies, started.elapsed()));
            }
            return Ok(0);
        }
        if cycle == 1 && !prompt::confirm(&confirm_question(name, &outcome.plan), yes) {
            say("Aborted.");
            return Ok(0);
        }
        say(&cycle_line(cycle, &outcome.plan));
        let report = match pipeline.burn_with(name, chrono::Utc::now(), burner, Some(step)) {
            Ok(report) => report,
            Err(err) => {
                let advice = rerun_advice(name, &err);
                return Ok(stop(&burned, err, advice));
            }
        };
        burned.extend(report.discs.iter().cloned());
        if after_burn(&report) == Next::Done {
            say(&render::burn_report(&report, copies));
            say(&summary(&burned, copies, started.elapsed()));
            return Ok(0);
        }
        say(&render::burn_discs(&report, copies));
    }
    unreachable!("the cycle counter does not run out")
}
```

`say` is private to `main.rs`. A child module can use it as `crate::say`. Remove `#![allow(dead_code)]`.

- [ ] **Step 2: Replace the stub arm in `dispatch`**

Add `use diskrement_shell::burn::drive::{DriveWatch as _, SysDriveWatch};` and `use diskrement_shell::burn::BurnDevice;` to `main.rs`, then:

```rust
        Command::Run { name, yes } => {
            let sink = progress::ConsoleProgress::new();
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let par = diskrement_shell::fs::scanner::parallelism_from_env();
            for w in &par.warnings {
                eprintln!("{w}");
            }
            let exec = SYSTEM_EXEC;
            let operator = prompt::ConsoleOperator;
            let backend = backend::resolve(cli.burner, cli.device.as_deref(), &env_lookup);
            // Up front (spec §4.4): a drive that cannot be watched fails
            // before minutes of scanning, not at the first wait.
            let watch = match &backend {
                backend::Backend::Real(BurnDevice::RealDevice(path)) => {
                    let watch = SysDriveWatch::new(path.clone());
                    watch.state()?;
                    Some(watch)
                }
                _ => None,
            };
            let burner = match backend {
                backend::Backend::Simulated => {
                    Burner::Simulated(SimulatedBurner::new(&sink, par.value))
                }
                backend::Backend::Real(device) => {
                    let burner = XorrisoBurner::new(device, &exec, &operator, &sink, par.value);
                    Burner::Real(match &watch {
                        Some(watch) => burner.detecting_media(watch),
                        None => burner,
                    })
                }
            };
            let verifier = resolve_verifier(
                cli.burner,
                cli.device.as_deref(),
                std::path::Path::new(&p.work_dir),
                true,
                &exec,
                &operator,
                &sink,
            );
            let step = VerifyAfterBurn {
                verifier: &verifier,
                operator: &operator,
                retry_failed_first_session: false,
            };
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &sink,
            );
            run::backlog(&mut pipeline, &name, p.copies, yes, &burner, &step)
        }
```

`watch.state()?` relies on `From<BurnError> for PipelineError` (`#[from]` on `PipelineError::Burn`), so a missing device exits 6.

- [ ] **Step 3: Build and run the CLI tests**

Run: `cargo test -p diskrement-cli`
Expected: all pass. The loop itself is exercised in Task 9.

- [ ] **Step 4: Clippy, fmt, commit**

```bash
git add crates/diskrement-cli/src/run.rs crates/diskrement-cli/src/main.rs
git commit -m "feat(cli): run plans and burns cycle after cycle until the backlog is on disc"
```

---

### Task 9: Scenario 17 and the binary contract

**Files:**
- Create: `crates/diskrement-cli/tests/scenarios/s17_run.rs`
- Modify: `crates/diskrement-cli/tests/scenarios/main.rs` (`mod s17_run;` after `mod s16_burn_verify;`)
- Modify: `crates/diskrement-cli/tests/binary.rs` (next to `a_real_burn_with_verify_still_asks_for_the_disc_first`)

- [ ] **Step 1: Write the scenario**

```rust
//! Scenario 17: `run` at two copies burns a three-pair backlog in one
//! command, each disc read back verified, and stops at the open last pair.

use crate::support;

#[test]
fn run_burns_the_backlog_and_stops_at_the_open_last_pair() {
    let Some(s) = support::scratch_or_skip("s17 run") else {
        return;
    };
    // One 800-byte file per 1000-byte disc: three discs per set.
    let cli = s.cli_with_disc_bytes(1000);
    for f in ["a.bin", "b.bin", "c.bin"] {
        support::write(&s.source, f, 800);
    }
    let init = cli.run(&[
        "init",
        "p",
        "--source",
        &s.source.display().to_string(),
        "--work-dir",
        &s.work.display().to_string(),
        "--copies",
        "2",
    ]);
    assert_eq!(init.code, 0, "init: {}{}", init.stdout, init.stderr);

    let run = cli.run(&["run", "p", "--yes"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.out_has("Cycle 1: 1A ("), "{}", run.stdout);
    assert!(run.out_has("Cycle 3: 3A ("), "{}", run.stdout);
    assert!(run.out_has("stays open"), "{}", run.stdout);
    assert!(!run.out_has("Cycle 4"), "{}", run.stdout);
    assert_eq!(
        run.stdout.matches("Read back: verified (1 files).").count(),
        6,
        "{}",
        run.stdout
    );
    assert!(
        run.out_has("verified: 1A, 1B, 2A, 2B, 3A, 3B."),
        "{}",
        run.stdout
    );

    let status = cli.run(&["status", "p"]);
    assert!(status.out_has("#3A  OPEN"), "{}", status.stdout);
    assert!(status.out_has("#3B  OPEN"), "{}", status.stdout);
    assert!(status.out_has("#2B  FINALIZED"), "{}", status.stdout);

    let again = cli.run(&["run", "p", "--yes"]);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(again.out_has("Nothing to burn"), "{}", again.stdout);

    let verify = cli.run(&["verify", "p"]);
    assert_eq!(verify.code, 0, "{}{}", verify.stdout, verify.stderr);
}
```

- [ ] **Step 2: Write the two binary tests**

```rust
    /// `run` asks once, before its first burn. With no stdin the answer is
    /// "no": exit 0, nothing burned.
    #[test]
    fn run_asks_once_and_an_unanswered_question_burns_nothing() {
        let p = project();
        base(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .assert()
            .success();
        base(&p)
            .args(["run", "p"])
            .assert()
            .code(0)
            .stdout(contains("Burn the backlog of p (about 1 cycle)? [y/N]"))
            .stdout(contains("Aborted."));
        base(&p)
            .args(["status", "p"])
            .assert()
            .success()
            .stdout(contains("Last burn: never"));
    }

    /// A drive that cannot be watched fails before anything is scanned or
    /// staged (spec §4.4): exit 6, naming the device.
    #[test]
    fn run_on_a_missing_drive_fails_before_planning() {
        let p = project();
        base(&p)
            .args(["init", "p", "--source"])
            .arg(&p.source)
            .arg("--work-dir")
            .arg(&p.work)
            .assert()
            .success();
        let missing = p.work.join("no-such-drive");
        Command::cargo_bin("diskrement")
            .expect("binary builds")
            .arg("--catalog")
            .arg(&p.catalog)
            .args(["--burner", "real", "--device"])
            .arg(&missing)
            .args(["run", "p", "--yes"])
            .env_remove("DISKREMENT_BURNER")
            .env_remove("DISKREMENT_DEVICE")
            .assert()
            .code(6)
            .stderr(contains("burner device not found"))
            .stderr(contains("no-such-drive"));
        assert!(!p.work.join("staged/plan.json").exists(), "nothing was planned");
    }
```

- [ ] **Step 3: Run them with the scenarios required**

Run: `DISKREMENT_SCENARIOS_REQUIRED=1 cargo test -p diskrement-cli`
Expected: all pass, with s17 reported as run, not SKIPPED. Without a CoW `TMPDIR` this fails by design; point `TMPDIR` at a btrfs directory (e.g. under `/data/ext`) and re-run.

If the summary's verified count or the status layout differs (for example, `status` pads columns differently), adjust the assertion to the real output, after checking that the behaviour itself is right.

- [ ] **Step 4: Clippy, fmt, commit**

```bash
git add crates/diskrement-cli/tests/scenarios/s17_run.rs \
        crates/diskrement-cli/tests/scenarios/main.rs crates/diskrement-cli/tests/binary.rs
git commit -m "test(cli): scenario 17, run burns a three-pair backlog; run's contract at the binary"
```

---

### Task 10: Docs

**Files:**
- Modify: `CLAUDE.md` (Gotchas; the "Exit codes"-adjacent invariants stay as they are)
- Modify: `crates/diskrement-cli/src/prompt.rs` (`ConsoleOperator` doc comment)
- Modify: `docs/smoke-test-real-media.md` (new item 14 after item 13)

- [ ] **Step 1: CLAUDE.md gotcha**

After the "The real backend prompts before it opens the device." bullet, add:

```markdown
- **xorriso closes the tray whenever it acquires the drive.** So `run` never
  polls with it: `burn::drive::SysDriveWatch` asks the kernel
  (`CDROM_DRIVE_STATUS` on an `O_NONBLOCK` open, which neither moves the tray nor
  reads the media), and the burner inspects through xorriso only once a disc is
  seated, once per insertion. A wrong disc waits for the tray to open; xorriso
  seeing no disc yet is looked at again. `run` does not retry a failed first
  session (`VerifyAfterBurn::retry_failed_first_session`): under detection a
  retry would carry the run on past a failure.
```

and, at the end of the "The real backend prompts before it opens the device." bullet, append: "`run` replaces that prompt with waiting for a seated disc that passes the same identity check; plain `burn` keeps it."

- [ ] **Step 2: `ConsoleOperator` doc comment**

Replace the second paragraph of the comment above `pub struct ConsoleOperator;` with:

```rust
/// `confirm_insert` never assumes yes: `burn --yes` answers "burn the staged
/// plan?", not "is the right disc in the drive?", and no flag may skip the
/// second question. `run` does not skip it either: its burner waits for a
/// seated disc that passes the same identity check (continuous-run spec
/// §4.5), and only the reinsert question still comes here.
```

- [ ] **Step 3: Smoke item 14**

Append to `docs/smoke-test-real-media.md`, after item 13:

```markdown
- [ ] 14. `run` (continuous-run spec §7.4), on the first immich-dsz pair:
      `diskrement run immich-dsz` with
      `DISKREMENT_DEVICE=/dev/disk/by-id/usb-PIONEER_BD-RW_BDR-UD04_<serial>-0:0`.
      - (a) With the tray open and no disc, the run prints `Waiting for a blank
        disc …` and the tray stays open: nothing pulls it in.
      - (b) A blank is picked up within seconds of the tray closing, and the
        burn starts with no `[y/N]`.
      - (c) After the eject, closing the tray on the disc just burned prints
        `Found disc …; waiting for …` exactly once, and nothing more until the
        tray is opened again.
      - (d) Ctrl-C while waiting: `status` is unchanged.
      - (e) Record whether the first inspection after a tray closes ever
        misreads the disc. If it does, add the 5 s settle (spec §4.3).
```

- [ ] **Step 4: Check `--help`**

Run: `cargo run -q -p diskrement-cli -- run --help`
Expected: the `Run` doc comment from Task 6, and `-y, --yes`.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md crates/diskrement-cli/src/prompt.rs docs/smoke-test-real-media.md
git commit -m "docs: run waits for each disc; the kernel is polled because xorriso closes the tray"
```

---

### Task 11: Full verification

- [ ] **Step 1: The whole suite, as evidence**

```bash
DISKREMENT_SCENARIOS_REQUIRED=1 cargo test --workspace 2>&1 | tee /tmp/run-tests.log | grep -E "^test result|SKIP"
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Expected: every `test result: ok`. The total is the baseline (Task 0) plus the tests this plan adds: 3 drive, 8 burner, 1 read-back, 1 render, 1 args, 7 run, 1 scenario, 2 binary. No new SKIPs except `xorriso_image.rs` when xorriso is absent.

- [ ] **Step 2: Release build for the smoke test**

Run: `cargo build --release`
Expected: `target/release/diskrement run --help` works.

- [ ] **Step 3: Append execution deviations, if any, to this plan**

Under a new `## Execution notes` heading at the end of this file, list every place the code had to differ from the plan, and why. Commit as `docs(plan): continuous run execution notes`.

- [ ] **Step 4: Hand over for review and merge**

A final whole-branch review, then the merge per the repo etiquette:
`git switch main && git merge --no-ff feat/continuous-run -m "Merge feat/continuous-run: run burns the backlog, waiting for each disc"`, then delete the branch locally and on `origin`.
