//! Reading a burned disc back (`DiscReader.scala`).
//!
//! Real devices are mounted through `udisksctl` — polkit lets a desktop
//! session mount removable media without root — and image files are extracted
//! through xorriso's osirrox. Both expose the same thing to the verifier: a
//! local directory holding the disc's merged `/data` tree.

use std::path::{Path, PathBuf};

use crate::burn::cmd::Exec;
use crate::burn::xorriso::args;
use crate::burn::{BurnDevice, BurnError};

pub struct DiscReader<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
}

impl<'a> DiscReader<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec) -> Self {
        DiscReader { device, exec }
    }

    /// Expose the disc's merged `/data` tree as a local directory for `use_it`.
    ///
    /// The mount is released, and the temporary extraction directory removed,
    /// whether `use_it` succeeded or not.
    pub fn with_data_tree<A>(
        &self,
        use_it: impl FnOnce(&Path) -> Result<A, BurnError>,
    ) -> Result<A, BurnError> {
        match &self.device {
            BurnDevice::RealDevice(path) => {
                let mount_point = self.mount(path)?;
                let out = use_it(&mount_point.join("data"));
                self.unmount(path);
                out
            }
            BurnDevice::ImageFile(_) => {
                // Dropped at the end of this scope, which removes the tree.
                let dir = tempfile::TempDir::new().map_err(BurnError::Io)?;
                let data_dir = dir.path().join("data");
                let result =
                    self.exec
                        .run_quiet(&args::extract(&self.device, "/data", &data_dir, None))?;
                if result.exit_code != 0 {
                    return Err(BurnError::MountFailed(format!(
                        "osirrox extraction failed: {}",
                        result.tail(10).join(" | ")
                    )));
                }
                use_it(&data_dir)
            }
        }
    }

    /// Extract the disc's `/data` tree into `dest`, which must not exist yet,
    /// through xorriso's own reader rather than a kernel mount, passing each
    /// output line to `on_line` (the restore pacifier is the progress). This
    /// is how `burn --verify` reads a disc the kernel has not noticed yet
    /// (burn-verify spec §8). `Err` carries xorriso's words.
    pub fn extract_data_tree(
        &self,
        dest: &Path,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let result = self
            .exec
            .run(&args::extract(&self.device, "/data", dest, None), on_line)
            .map_err(|e| crate::error::with_causes(&e))?;
        if result.exit_code != 0 {
            return Err(format!(
                "xorriso exited {}: {}",
                result.exit_code,
                result.tail(5).join(" | ")
            ));
        }
        Ok(())
    }

    /// Extract one file as recorded by a specific session — the tier-2 read of
    /// a version a later session hid.
    pub fn extract_from_session(
        &self,
        session_ordinal: u32,
        disc_path: &str,
        dest: &Path,
    ) -> Result<(), BurnError> {
        let result = self.exec.run_quiet(&args::extract(
            &self.device,
            disc_path,
            dest,
            Some(session_ordinal),
        ))?;
        if result.exit_code != 0 {
            return Err(BurnError::MountFailed(format!(
                "session {session_ordinal} extraction of {disc_path} failed: {}",
                result.tail(10).join(" | ")
            )));
        }
        Ok(())
    }

    fn mount(&self, device_path: &str) -> Result<PathBuf, BurnError> {
        let result = self
            .exec
            .run_quiet(&[
                "udisksctl".to_string(),
                "mount".to_string(),
                "-b".to_string(),
                device_path.to_string(),
            ])
            .map_err(|e| match e {
                BurnError::ToolMissing(detail) => {
                    BurnError::MountFailed(format!("udisksctl not available: {detail}"))
                }
                other => other,
            })?;
        parse_mount_point(&result.lines).ok_or_else(|| {
            BurnError::MountFailed(format!(
                "udisksctl gave no mount point: {}",
                result.tail(10).join(" | ")
            ))
        })
    }

    /// Best-effort: a failed unmount must not fail a verification that has
    /// already read everything it needed.
    fn unmount(&self, device_path: &str) {
        let _ = self.exec.run_quiet(&[
            "udisksctl".to_string(),
            "unmount".to_string(),
            "-b".to_string(),
            device_path.to_string(),
        ]);
    }
}

/// The mount point from `udisksctl mount` output.
///
/// Two shapes, replacing `DiscReaderLive`'s two regexes: the success line
/// `Mounted <dev> at <path>` (with an optional trailing period, which some
/// udisks versions add) and the AlreadyMounted error, which names the path
/// between backtick and apostrophe.
pub fn parse_mount_point(lines: &[String]) -> Option<PathBuf> {
    lines.iter().find_map(|line| {
        if let Some((_, rest)) = line.split_once("already mounted at `") {
            return rest.split_once('\'').map(|(p, _)| PathBuf::from(p));
        }
        let (_, rest) = line.split_once("Mounted ")?;
        let (_, path) = rest.split_once(" at ")?;
        let path = path.trim();
        // Only the *sentence-final* period is dropped, so a directory whose
        // name genuinely ends in one survives everywhere else in the path.
        Some(PathBuf::from(path.strip_suffix('.').unwrap_or(path)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::cmd::CmdOutput;
    use std::cell::RefCell;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_mount_line_gives_the_mount_point() {
        assert_eq!(
            parse_mount_point(&lines(&["Mounted /dev/sr0 at /run/media/z/PHOTOS_DISC_1"])),
            Some(PathBuf::from("/run/media/z/PHOTOS_DISC_1"))
        );
    }

    /// udisksctl ends the sentence with a period on some versions; it is not
    /// part of the path.
    #[test]
    fn a_trailing_period_is_not_part_of_the_path() {
        assert_eq!(
            parse_mount_point(&lines(&["Mounted /dev/sr0 at /run/media/z/D."])),
            Some(PathBuf::from("/run/media/z/D"))
        );
    }

    /// An already-mounted disc is not a failure: the error message carries the
    /// mount point we wanted.
    #[test]
    fn an_already_mounted_disc_reports_where_it_is() {
        assert_eq!(
            parse_mount_point(&lines(&[
                "Error mounting: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted: \
                 Device /dev/sr0 is already mounted at `/run/media/z/OLD'."
            ])),
            Some(PathBuf::from("/run/media/z/OLD"))
        );
    }

    #[test]
    fn unrecognised_output_gives_no_mount_point() {
        assert_eq!(parse_mount_point(&lines(&["garbage"])), None);
    }

    /// Unmount runs even when the caller's closure failed — otherwise a failed
    /// verify would leave the disc mounted and the next one would hit the
    /// AlreadyMounted path.
    #[test]
    fn the_disc_is_unmounted_even_when_the_body_fails() {
        let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            seen.borrow_mut().push(cmd.join(" "));
            Ok(CmdOutput {
                exit_code: 0,
                lines: lines(&["Mounted /dev/sr0 at /mnt/disc"]),
            })
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader
            .with_data_tree(|data| {
                assert_eq!(data, std::path::Path::new("/mnt/disc/data"));
                Err::<(), _>(BurnError::MountFailed("body failed".to_string()))
            })
            .unwrap_err();
        assert!(matches!(err, BurnError::MountFailed(_)));
        let calls = seen.borrow();
        assert!(
            calls[0].contains("udisksctl mount -b /dev/sr0"),
            "{calls:?}"
        );
        assert!(
            calls[1].contains("udisksctl unmount -b /dev/sr0"),
            "{calls:?}"
        );
    }

    /// udisksctl that says nothing useful is a `MountFailed` quoting it,
    /// rather than a mount at some guessed path.
    #[test]
    fn a_mount_with_no_mount_point_fails_loudly() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            Ok(CmdOutput {
                exit_code: 1,
                lines: lines(&["something went wrong"]),
            })
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader.with_data_tree(|_| Ok(())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "could not mount the disc for verification: udisksctl gave no mount point: \
             something went wrong"
        );
    }

    /// A missing `udisksctl` is reported as a mount failure naming the tool,
    /// not as "xorriso is not available" (`DiscReaderLive.mount`'s mapError).
    #[test]
    fn a_missing_udisksctl_is_a_mount_failure() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            Err(BurnError::ToolMissing(
                "cannot launch udisksctl: nope".to_string(),
            ))
        };
        let reader = DiscReader::new(BurnDevice::RealDevice("/dev/sr0".to_string()), &exec);
        let err = reader.with_data_tree(|_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("udisksctl not available"), "{err}");
    }
}
