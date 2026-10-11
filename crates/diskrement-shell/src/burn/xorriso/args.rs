//! Pure xorriso argument vectors (`Xorriso.scala`).
//!
//! Every vector starts with the binary name; `cmd::Exec` runs it. The shapes
//! are validated facts from the real-burning plan
//! (`docs/superpowers/plans/2026-08-30-diskrement-plan-4a-real-burning.md`,
//! "Validated xorriso facts", xorriso 1.5.8): `-compliance iso_9660_level=3` is the native-mode spelling —
//! `-iso_level` is mkisofs-emulation syntax and silently does nothing here —
//! and `-dev` merges a new session where `-outdev` starts a fresh image.

use std::path::Path;

use crate::burn::BurnDevice;

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// Master `staged_dir` onto the medium as one session.
pub fn burn(
    device: &BurnDevice,
    staged_dir: &Path,
    first_session: bool,
    close_disc: bool,
    volid: &str,
) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE"]);
    out.push(if first_session { "-outdev" } else { "-dev" }.to_string());
    out.push(device.xorriso_address());
    out.extend(words(&["-compliance", "iso_9660_level=3", "-volid"]));
    out.push(volid.to_string());
    out.push("-map".to_string());
    out.push(staged_dir.display().to_string());
    out.push("/".to_string());
    out.push("-close".to_string());
    out.push(if close_disc { "on" } else { "off" }.to_string());
    out.push("-commit".to_string());
    out
}

/// Report the medium's state and its free space.
pub fn inspect(device: &BurnDevice) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE", "-outdev"]);
    out.push(device.xorriso_address());
    out.extend(words(&["-toc", "-tell_media_space"]));
    out
}

/// Copy `disc_path` off the medium. `session` picks one recorded session
/// instead of the merged view — the tier-2 read.
pub fn extract(
    device: &BurnDevice,
    disc_path: &str,
    dest: &Path,
    session: Option<u32>,
) -> Vec<String> {
    let mut out = words(&["xorriso", "-abort_on", "FAILURE", "-indev"]);
    out.push(device.xorriso_address());
    if let Some(n) = session {
        out.extend(words(&["-load", "session"]));
        out.push(n.to_string());
    }
    out.extend(words(&["-osirrox", "on", "-extract"]));
    out.push(disc_path.to_string());
    out.push(dest.display().to_string());
    out
}

pub fn eject(device: &BurnDevice) -> Vec<String> {
    let mut out = words(&["xorriso", "-outdev"]);
    out.push(device.xorriso_address());
    out.extend(words(&["-eject", "all"]));
    out
}

/// `xorriso --version`, whose output the preflight parses.
pub fn version() -> Vec<String> {
    words(&["xorriso", "--version"])
}

/// An ISO volume id: uppercase, `[A-Z0-9_]`, at most 32 characters.
pub fn volid(project_name: &str, disc_name: &str) -> String {
    format!("{project_name}_DISC_{disc_name}")
        .to_uppercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(32)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn image() -> BurnDevice {
        BurnDevice::ImageFile(PathBuf::from("/tmp/d.iso"))
    }

    #[test]
    fn the_first_session_writes_to_outdev_and_leaves_the_disc_open() {
        assert_eq!(
            burn(
                &image(),
                Path::new("/w/staging/disc-1"),
                true,
                false,
                "PHOTOS_DISC_1"
            ),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-outdev",
                "stdio:/tmp/d.iso",
                "-compliance",
                "iso_9660_level=3",
                "-volid",
                "PHOTOS_DISC_1",
                "-map",
                "/w/staging/disc-1",
                "/",
                "-close",
                "off",
                "-commit",
            ]
        );
    }

    /// An append uses `-dev` (which merges) rather than `-outdev` (which
    /// starts a new image), and a sealing session closes the disc.
    #[test]
    fn an_appending_session_uses_dev_and_can_close() {
        let args = burn(
            &BurnDevice::RealDevice("/dev/sr0".to_string()),
            Path::new("/w/s"),
            false,
            true,
            "V",
        );
        assert!(args.contains(&"-dev".to_string()), "{args:?}");
        assert!(args.contains(&"/dev/sr0".to_string()), "{args:?}");
        assert!(!args.contains(&"-outdev".to_string()), "{args:?}");
        assert!(
            args.windows(2)
                .any(|w| w == ["-close".to_string(), "on".to_string()]),
            "{args:?}"
        );
    }

    #[test]
    fn inspect_reads_the_toc_and_the_media_space() {
        assert_eq!(
            inspect(&image()),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-outdev",
                "stdio:/tmp/d.iso",
                "-toc",
                "-tell_media_space",
            ]
        );
    }

    #[test]
    fn extract_without_a_session_reads_the_merged_view() {
        assert_eq!(
            extract(
                &image(),
                "/.diskrement/project.json",
                Path::new("/tmp/p.json"),
                None
            ),
            vec![
                "xorriso",
                "-abort_on",
                "FAILURE",
                "-indev",
                "stdio:/tmp/d.iso",
                "-osirrox",
                "on",
                "-extract",
                "/.diskrement/project.json",
                "/tmp/p.json",
            ]
        );
    }

    /// Tier-2 verification reads a version a later session hid, so the session
    /// is loaded before osirrox is switched on.
    #[test]
    fn extract_with_a_session_loads_that_session_first() {
        let args = extract(&image(), "/data/a.txt", Path::new("/tmp/a"), Some(1));
        let expected: Vec<String> = [
            "-indev",
            "stdio:/tmp/d.iso",
            "-load",
            "session",
            "1",
            "-osirrox",
            "on",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(
            args.windows(expected.len()).any(|w| w == expected),
            "{args:?}"
        );
    }

    #[test]
    fn eject_targets_the_device() {
        assert_eq!(
            eject(&BurnDevice::RealDevice("/dev/sr0".to_string())),
            vec!["xorriso", "-outdev", "/dev/sr0", "-eject", "all"]
        );
    }

    /// An ISO volume id is uppercase `[A-Z0-9_]`, at most 32 characters.
    #[test]
    fn the_volume_id_carries_the_set_letter() {
        assert_eq!(volid("photos", "3B"), "PHOTOS_DISC_3B");
    }

    #[test]
    fn volid_uppercases_replaces_and_truncates() {
        assert_eq!(volid("my photos!", "3"), "MY_PHOTOS__DISC_3");
        assert_eq!(volid(&"x".repeat(40), "1").len(), 32);
    }
}
