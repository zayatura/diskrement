//! What is in the drive (`MediaInspector.scala`).
//!
//! `inspect` classifies the medium; for written media it also reads the
//! identity the last session left in `/.diskrement/project.json`, which is what
//! lets the burner refuse the wrong disc before writing to it.

use std::fmt;

use crate::burn::cmd::Exec;
use crate::burn::json::ProjectJson;
use crate::burn::xorriso::{args, parse};
use crate::burn::{BurnDevice, BurnError, DiscIdentity};

/// The state of the medium in a device.
///
/// `Display` reproduces the Scala enum's `toString` because two user-visible
/// messages interpolate it (see the plan's D3); do not swap it for `Debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaStatus {
    NoMedia,
    Blank(u64),
    Appendable {
        free_bytes: u64,
        sessions: u32,
        identity: Option<DiscIdentity>,
    },
    Closed(Option<DiscIdentity>),
    /// Media libburn refuses to write sequentially, e.g. a POW-formatted BD-R.
    Unsuitable,
}

fn show(identity: &Option<DiscIdentity>) -> String {
    match identity {
        Some(id) => format!("Some({id})"),
        None => "None".to_string(),
    }
}

impl fmt::Display for MediaStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaStatus::NoMedia => f.write_str("NoMedia"),
            MediaStatus::Blank(free) => write!(f, "Blank({free})"),
            MediaStatus::Appendable {
                free_bytes,
                sessions,
                identity,
            } => write!(f, "Appendable({free_bytes},{sessions},{})", show(identity)),
            MediaStatus::Closed(identity) => write!(f, "Closed({})", show(identity)),
            MediaStatus::Unsuitable => f.write_str("Unsuitable"),
        }
    }
}

pub struct MediaInspector<'a> {
    device: BurnDevice,
    exec: &'a dyn Exec,
}

impl<'a> MediaInspector<'a> {
    pub fn new(device: BurnDevice, exec: &'a dyn Exec) -> Self {
        MediaInspector { device, exec }
    }

    pub fn inspect(&self) -> Result<MediaStatus, BurnError> {
        let result = self.exec.run_quiet(&args::inspect(&self.device))?;
        if result.exit_code != 0 {
            return match parse::classify_failure(&result.lines) {
                // An absent disc is a state, not a failure — the drive says so
                // both ways, at exit 0 and at exit 5.
                parse::FailureKind::NoMedia => Ok(MediaStatus::NoMedia),
                parse::FailureKind::Permission => Err(BurnError::DevicePermissionDenied(
                    self.device.xorriso_address(),
                )),
                parse::FailureKind::NotFound => {
                    Err(BurnError::DeviceNotFound(self.device.xorriso_address()))
                }
                parse::FailureKind::Other => Err(BurnError::BurnFailed {
                    exit_code: result.exit_code,
                    output_tail: result.tail(10),
                }),
            };
        }

        let info = parse::media(&result.lines).map_err(|complaint| BurnError::BurnFailed {
            exit_code: result.exit_code,
            output_tail: vec![complaint],
        })?;
        Ok(match info.status {
            parse::RawStatus::Blank => MediaStatus::Blank(info.free_bytes),
            parse::RawStatus::Appendable => MediaStatus::Appendable {
                free_bytes: info.free_bytes,
                sessions: info.sessions,
                identity: self.read_identity()?,
            },
            parse::RawStatus::Closed => MediaStatus::Closed(self.read_identity()?),
            parse::RawStatus::Unsuitable => MediaStatus::Unsuitable,
            parse::RawStatus::NotPresent => MediaStatus::NoMedia,
        })
    }

    /// The `/.diskrement/project.json` identity of the disc's merged view.
    ///
    /// `Ok(None)` covers every "this is not one of ours" case: the file is
    /// absent, the extraction failed, or the JSON does not parse. Only the
    /// temp-directory creation can fail here.
    pub fn read_identity(&self) -> Result<Option<DiscIdentity>, BurnError> {
        let dir = tempfile::TempDir::new().map_err(BurnError::Io)?;
        // A path inside a fresh directory, deliberately not yet created:
        // xorriso creates it, and nothing else can be holding it.
        let dest = dir.path().join("project.json");
        let result = self.exec.run_quiet(&args::extract(
            &self.device,
            "/.diskrement/project.json",
            &dest,
            None,
        ))?;
        if result.exit_code != 0 {
            return Ok(None);
        }
        let Ok(text) = std::fs::read_to_string(&dest) else {
            return Ok(None);
        };
        Ok(serde_json::from_str::<ProjectJson>(&text)
            .ok()
            .and_then(|p| p.to_identity().ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burn::cmd::CmdOutput;

    fn device() -> BurnDevice {
        BurnDevice::RealDevice("/dev/sr0".to_string())
    }

    fn out(code: i32, lines: &[&str]) -> Result<CmdOutput, BurnError> {
        Ok(CmdOutput {
            exit_code: code,
            lines: lines.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    const BLANK_TOC: &[&str] = &[
        "Media status : is blank",
        "Media summary: 0 sessions, 0 data blocks, 0 data, 23g free",
        "Media space  : 11826176s",
    ];

    const APPENDABLE_TOC: &[&str] = &[
        "Media status : is written , is appendable",
        "Media summary: 2 sessions, 57 data blocks, 114k data, 23g free",
        "Media space  : 11826000s",
    ];

    const IDENTITY_JSON: &str = r#"{"projectName":"photos","discOrdinal":1,"sessionOrdinal":2,"createdAt":"2026-08-30T10:00:00Z"}"#;

    #[test]
    fn blank_media_reports_its_free_space() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| out(0, BLANK_TOC);
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::Blank(11_826_176 * 2048)
        );
    }

    /// Written diskrement media: the `-toc` gives the state, a second
    /// invocation extracts `/.diskrement/project.json`, and the identity comes
    /// back parsed.
    #[test]
    fn an_appendable_diskrement_disc_yields_its_identity() {
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            if cmd.iter().any(|a| a == "-toc") {
                out(0, APPENDABLE_TOC)
            } else if cmd.iter().any(|a| a == "-extract") {
                std::fs::write(cmd.last().expect("a destination"), IDENTITY_JSON)
                    .expect("the destination is writable");
                out(0, &[])
            } else {
                out(1, &["unexpected"])
            }
        };
        let status = MediaInspector::new(device(), &exec).inspect().unwrap();
        match status {
            MediaStatus::Appendable {
                free_bytes,
                sessions,
                identity: Some(id),
            } => {
                assert_eq!(free_bytes, 11_826_000 * 2048);
                assert_eq!(sessions, 2);
                assert_eq!(id.project_name, "photos");
                assert_eq!(id.disc_ordinal, 1);
                assert_eq!(id.session_ordinal, 2);
            }
            other => panic!("unexpected: {other}"),
        }
    }

    /// A written disc that is not ours: the extraction fails, and that is an
    /// answer (`None`), not an error.
    #[test]
    fn an_appendable_foreign_disc_yields_no_identity() {
        let exec = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            if cmd.iter().any(|a| a == "-toc") {
                out(0, APPENDABLE_TOC)
            } else {
                out(5, &["FAILURE : cannot find path"])
            }
        };
        assert!(matches!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::Appendable { identity: None, .. }
        ));
    }

    /// The empty tray as the drive really reports it: exit 0 with
    /// "is not present" (captured 2026-08-30).
    #[test]
    fn an_empty_tray_at_exit_zero_is_no_media() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(
                0,
                &[
                    "Media current: is not recognizable",
                    "Media status : is not present",
                    "Media space  : 0s",
                ],
            )
        };
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::NoMedia
        );
    }

    /// A drive that fails with a no-disc message is also just "no media".
    #[test]
    fn a_no_disc_failure_is_no_media() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["libburn : SORRY : No disc loaded, not ready"])
        };
        assert_eq!(
            MediaInspector::new(device(), &exec).inspect().unwrap(),
            MediaStatus::NoMedia
        );
    }

    #[test]
    fn a_permission_failure_names_the_optical_group() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["Failed to open device '/dev/sr0' : Permission denied"])
        };
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert!(matches!(err, BurnError::DevicePermissionDenied(ref d) if d == "/dev/sr0"));
        assert!(err.to_string().contains("usermod -aG optical"), "{err}");
    }

    #[test]
    fn a_missing_device_is_device_not_found() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| {
            out(5, &["stdio: could not open, No such file or directory"])
        };
        assert!(matches!(
            MediaInspector::new(device(), &exec).inspect().unwrap_err(),
            BurnError::DeviceNotFound(_)
        ));
    }

    /// An unrecognised failure keeps xorriso's own words, so the operator can
    /// act on something.
    #[test]
    fn an_unclassifiable_failure_surfaces_the_output_tail() {
        let exec =
            |_: &[String], _: &mut dyn FnMut(&str)| out(5, &["xorriso : FAILURE : something else"]);
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert_eq!(
            err.to_string(),
            "xorriso failed (exit 5): xorriso : FAILURE : something else"
        );
    }

    /// Output with no status line at all is a `BurnFailed` quoting the parse
    /// complaint (`MediaParse.parse`'s `Left`).
    #[test]
    fn unparsable_output_is_a_burn_failure_quoting_the_complaint() {
        let exec = |_: &[String], _: &mut dyn FnMut(&str)| out(0, &["Drive current: ..."]);
        let err = MediaInspector::new(device(), &exec).inspect().unwrap_err();
        assert!(err.to_string().contains("no Media status line"), "{err}");
    }

    /// D3: these strings are interpolated into user-visible messages, so they
    /// reproduce Scala's `toString` exactly.
    #[test]
    fn the_display_shapes_match_the_scala_tostring() {
        let id = DiscIdentity {
            project_name: "photos".to_string(),
            disc_ordinal: 1,
            session_ordinal: 2,
            created_at: crate::catalog::time::parse("2026-08-30T10:00:00Z").unwrap(),
        };
        assert_eq!(
            id.to_string(),
            "DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)"
        );
        assert_eq!(MediaStatus::NoMedia.to_string(), "NoMedia");
        assert_eq!(MediaStatus::Unsuitable.to_string(), "Unsuitable");
        assert_eq!(
            MediaStatus::Blank(24_220_008_448).to_string(),
            "Blank(24220008448)"
        );
        assert_eq!(
            MediaStatus::Appendable {
                free_bytes: 123,
                sessions: 2,
                identity: Some(id.clone()),
            }
            .to_string(),
            "Appendable(123,2,Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))"
        );
        assert_eq!(
            MediaStatus::Appendable {
                free_bytes: 123,
                sessions: 2,
                identity: None,
            }
            .to_string(),
            "Appendable(123,2,None)"
        );
        assert_eq!(
            MediaStatus::Closed(Some(id)).to_string(),
            "Closed(Some(DiscIdentity(photos,1,2,2026-08-30T10:00:00Z)))"
        );
        assert_eq!(MediaStatus::Closed(None).to_string(), "Closed(None)");
    }
}
