//! Whether the drive holds a disc, asked of the kernel (continuous-run spec
//! §4). xorriso closes the tray whenever it acquires the drive, so it can
//! never be what polls. `CDROM_DRIVE_STATUS` on a non-blocking open neither
//! moves the tray nor reads the media.

use std::ffi::c_void;
use std::time::Duration;

use rustix::fs::{FileType, Mode, OFlags};
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

// SAFETY: the argument is an integer passed by value and, for the cdrom
// driver, the kernel writes no userspace memory. `SysDriveWatch` sends it to
// block devices only, whose drivers either are cdrom drivers or refuse an
// opcode they do not know (`ENOTTY`); a character device could read it as
// something else entirely.
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

    fn unavailable(&self, reason: impl ToString) -> BurnError {
        BurnError::DriveStatusUnavailable {
            device: self.device.clone(),
            reason: reason.to_string(),
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
        let mode = rustix::fs::fstat(&fd)
            .map_err(|e| self.unavailable(e))?
            .st_mode;
        if FileType::from_raw_mode(mode) != FileType::BlockDevice {
            return Err(self.unavailable("not a block device"));
        }
        // SAFETY: see `DriveStatus`; `fd` is a block device.
        let status =
            unsafe { rustix::ioctl::ioctl(&fd, DriveStatus) }.map_err(|e| self.unavailable(e))?;
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
        assert!(
            matches!(&err, BurnError::DeviceNotFound(d) if d.ends_with("sr9")),
            "{err:?}"
        );
    }

    /// A regular file opens fine but is no drive: it never reaches the
    /// ioctl, and the error names the device.
    #[test]
    fn a_file_that_is_not_a_drive_cannot_be_watched() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let shown = file.path().display().to_string();
        let err = SysDriveWatch::new(shown.clone()).state().unwrap_err();
        assert!(
            matches!(&err, BurnError::DriveStatusUnavailable { device, reason }
                if *device == shown && reason == "not a block device"),
            "{err:?}"
        );
    }

    /// `/dev/null` is a device, but a character device: the cdrom opcode
    /// could mean something else to its driver, so it is never sent.
    #[test]
    fn a_character_device_is_never_sent_the_ioctl() {
        let err = SysDriveWatch::new("/dev/null").state().unwrap_err();
        assert!(
            matches!(&err, BurnError::DriveStatusUnavailable { reason, .. }
                if reason == "not a block device"),
            "{err:?}"
        );
    }
}
