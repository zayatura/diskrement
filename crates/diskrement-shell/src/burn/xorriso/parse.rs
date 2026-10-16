//! Pure parsers for what xorriso prints (`MediaParse.scala`, plus
//! `Xorriso.parseVersion`).
//!
//! Line shapes were validated against xorriso 1.5.8; the "closed" and
//! empty-tray shapes were captured from a real drive on 2026-08-30 and are
//! recorded verbatim in this module's tests. No regex engine: the workspace
//! has none (spec §5.4), and each function below names the Scala pattern it
//! replaces so the two can be compared directly.

/// The medium's state, before the identity read-back turns it into a
/// `MediaStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawStatus {
    Blank,
    Appendable,
    Closed,
    Unsuitable,
    NotPresent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMediaInfo {
    pub status: RawStatus,
    pub sessions: u32,
    pub free_bytes: u64,
}

/// Why an inspection or a burn failed. Substrings chosen from observed and
/// documented libburn messages; unknown text falls through to `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    NoMedia,
    Permission,
    NotFound,
    Other,
}

/// The leading run of ASCII digits of `s` and its length, if `s` starts with
/// one. The length is returned rather than recomputed from the value, so a
/// zero-padded count cannot desynchronise the caller's slice.
fn leading_number(s: &str) -> Option<(u64, usize)> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let value = digits.parse().ok()?;
    Some((value, digits.len()))
}

/// Parse `xorriso -toc -tell_media_space` output.
///
/// Replaces three Scala patterns: the `Media status` substring match, the
/// `Media summary:\s*(\d+)\s+session` count, and the
/// `Media space\s*:\s*(\d+)s` block count (2048 bytes per block). A missing
/// count or space line is 0, exactly as `getOrElse(0)` had it.
pub fn media(lines: &[String]) -> Result<RawMediaInfo, String> {
    let status = lines.iter().find_map(|l| {
        if !l.contains("Media status") {
            return None;
        }
        // Order matters: "is written , is appendable" contains both, and
        // appendable must win — the Scala arms are in this same order.
        if l.contains("is not present") {
            Some(RawStatus::NotPresent)
        } else if l.contains("is blank") {
            Some(RawStatus::Blank)
        } else if l.contains("is unsuitable") {
            Some(RawStatus::Unsuitable)
        } else if l.contains("is appendable") {
            Some(RawStatus::Appendable)
        } else if l.contains("is written") {
            Some(RawStatus::Closed)
        } else {
            None
        }
    });

    let sessions = lines
        .iter()
        .find_map(|l| {
            let rest = l.split_once("Media summary:")?.1.trim_start();
            let (n, len) = leading_number(rest)?;
            rest[len..]
                .trim_start()
                .starts_with("session")
                .then_some(n as u32)
        })
        .unwrap_or(0);

    let free_blocks = lines
        .iter()
        .find_map(|l| {
            let rest = l.split_once("Media space")?.1;
            let rest = rest.trim_start().strip_prefix(':')?.trim_start();
            let (n, len) = leading_number(rest)?;
            rest[len..].starts_with('s').then_some(n)
        })
        .unwrap_or(0);

    match status {
        Some(status) => Ok(RawMediaInfo {
            status,
            sessions,
            free_bytes: free_blocks * 2048,
        }),
        None => Err(format!(
            "no Media status line in xorriso output: {}",
            lines
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        )),
    }
}

/// Why an invocation failed, from its output.
pub fn classify_failure(lines: &[String]) -> FailureKind {
    let text = lines.join("\n").to_lowercase();
    if text.contains("permission denied") {
        FailureKind::Permission
    } else if text.contains("no such file") || text.contains("cannot find device") {
        FailureKind::NotFound
    } else if ["no disc", "not ready", "no medium", "not present"]
        .iter()
        .any(|s| text.contains(s))
    {
        FailureKind::NoMedia
    } else {
        FailureKind::Other
    }
}

/// `(major, minor, raw)` from `xorriso --version` output.
///
/// Replaces `xorriso\s+((\d+)\.(\d+)\S*).*`, which Scala matched against the
/// whole line: the line must *start* with `xorriso`, and the raw version is
/// the token after it — `1.5.8.pl02`, not `1.5`.
pub fn version(lines: &[String]) -> Option<(u32, u32, String)> {
    lines.iter().find_map(|line| {
        let rest = line.strip_prefix("xorriso")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let raw = rest.split_whitespace().next()?;
        let (major, tail) = raw.split_once('.')?;
        let minor: String = tail.chars().take_while(char::is_ascii_digit).collect();
        Some((major.parse().ok()?, minor.parse().ok()?, raw.to_string()))
    })
}

/// How far a burn is, in tenths of a percent, from one of xorriso's pacifier
/// lines: `xorriso : UPDATE : Writing:  848140s   69.5%   fifo  99% …` → 695.
/// `None` for every other line.
///
/// xorriso prints these on stderr, about once a second while writing.
pub fn writing_per_mille(line: &str) -> Option<u32> {
    let (_, rest) = line.split_once("UPDATE : Writing:")?;
    let mut words = rest.split_whitespace();
    let sectors = words.next()?.strip_suffix('s')?;
    if sectors.is_empty() || !sectors.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let percent = words.next()?.strip_suffix('%')?;
    let (whole, tenths) = percent.split_once('.').unwrap_or((percent, "0"));
    let whole: u32 = whole.parse().ok()?;
    let tenth = tenths.chars().next()?.to_digit(10)?;
    Some((whole * 10 + tenth).min(1000))
}

/// How many bytes an `-extract` has restored so far, from xorriso's pacifier:
/// `xorriso : UPDATE :  31 files restored ( 490.0m) in 1 seconds` → 490 MiB.
/// The suffix is a binary unit (k, m, g, t) or absent for bytes. `None` for
/// every other line.
pub fn restored_bytes(line: &str) -> Option<u64> {
    let (_, rest) = line.split_once("files restored (")?;
    let (amount, _) = rest.split_once(')')?;
    let amount = amount.trim();
    let (number, scale) = match amount.chars().last()? {
        'k' => (&amount[..amount.len() - 1], 1u64 << 10),
        'm' => (&amount[..amount.len() - 1], 1 << 20),
        'g' => (&amount[..amount.len() - 1], 1 << 30),
        't' => (&amount[..amount.len() - 1], 1 << 40),
        _ => (amount, 1),
    };
    let value: f64 = number.parse().ok()?;
    (value >= 0.0).then_some((value * scale as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorriso's restore pacifier, as printed by `-extract` (1.5.8.pl02),
    /// scaled by its binary-unit suffix; anything else is `None`.
    #[test]
    fn a_restore_pacifier_gives_the_bytes_restored_so_far() {
        assert_eq!(
            restored_bytes(
                "xorriso : UPDATE :      31 files restored ( 490.0m) in 1 seconds = 371.0xD"
            ),
            Some(490 * 1024 * 1024)
        );
        assert_eq!(
            restored_bytes("xorriso : UPDATE :    2614 files restored ( 22.7g) in 1200 seconds"),
            Some((22.7 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(
            restored_bytes("xorriso : UPDATE :       1 files restored (   12k) in 1 seconds"),
            Some(12 * 1024)
        );
        assert_eq!(
            restored_bytes("xorriso : UPDATE :       1 files restored (  512) in 1 seconds"),
            Some(512)
        );
        assert_eq!(
            restored_bytes("xorriso : UPDATE :       9 files restored ( 1.5t) in 9 seconds"),
            Some((1.5 * 1024f64.powi(4)) as u64)
        );
        assert_eq!(
            restored_bytes("xorriso : UPDATE :      32 nodes read in 1 seconds"),
            None
        );
        assert_eq!(restored_bytes("files restored ( lots) in 1 seconds"), None);
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_string()).collect()
    }

    const BLANK: &[&str] = &[
        "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project.",
        "Drive current: -outdev 'stdio:empty.iso'",
        "Media current: stdio file, overwriteable",
        "Media status : is blank",
        "Media summary: 0 sessions, 0 data blocks, 0 data,  660g free",
        "Media space  : 346016270s",
        "After commit : 346016270s",
    ];

    const APPENDABLE: &[&str] = &[
        "Media current: stdio file, overwriteable",
        "Media status : is written , is appendable",
        "ISO session  :   1 ,        32 ,        29s , ISOIMAGE",
        "ISO session  :   2 ,        64 ,        28s , ISOIMAGE",
        "Media summary: 2 sessions, 57 data blocks,  114k data,  660g free",
        "Media space  : 346016430s",
    ];

    const CLOSED: &[&str] = &[
        "Media current: BD-R sequential recording",
        "Media status : is written , is closed",
        "Media summary: 3 sessions, 57 data blocks,  114k data, 0 free",
        "Media space  : 0s",
    ];

    // Real captured output (2026-08-30, Pioneer BDR-UD04, a POW-formatted
    // BD-R): such media cannot take sequential multisession burns.
    const UNSUITABLE: &[&str] = &[
        "Media current: BD-R sequential recording, Pseudo Overwrite formatted",
        "Media status : is unsuitable , is POW formatted",
        "Media summary: unsuitable Pseudo Overwrite formatted BD-R",
        "Media space  : 248768s",
    ];

    // Real captured output (2026-08-30, Pioneer BDR-UD04, empty tray): xorriso
    // exits 0 — an absent disc is a media status, not a failure.
    const EMPTY_TRAY: &[&str] = &[
        "xorriso : NOTE : Disc status unsuitable for writing",
        "Drive current: -outdev '/dev/sr0'",
        "Media current: is not recognizable",
        "Media status : is not present",
        "Media space  : 0s",
    ];

    #[test]
    fn blank_media() {
        assert_eq!(
            media(&lines(BLANK)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Blank,
                sessions: 0,
                free_bytes: 346_016_270 * 2048,
            }
        );
    }

    #[test]
    fn appendable_media_carries_its_session_count() {
        assert_eq!(
            media(&lines(APPENDABLE)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Appendable,
                sessions: 2,
                free_bytes: 346_016_430 * 2048,
            }
        );
    }

    #[test]
    fn written_but_not_appendable_is_closed() {
        assert_eq!(
            media(&lines(CLOSED)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Closed,
                sessions: 3,
                free_bytes: 0,
            }
        );
    }

    #[test]
    fn a_pow_formatted_bd_r_is_unsuitable() {
        assert_eq!(
            media(&lines(UNSUITABLE)).unwrap(),
            RawMediaInfo {
                status: RawStatus::Unsuitable,
                sessions: 0,
                free_bytes: 248_768 * 2048,
            }
        );
    }

    #[test]
    fn an_empty_tray_exits_zero_and_parses_as_not_present() {
        assert_eq!(
            media(&lines(EMPTY_TRAY)).unwrap(),
            RawMediaInfo {
                status: RawStatus::NotPresent,
                sessions: 0,
                free_bytes: 0,
            }
        );
    }

    /// No recognisable status line is an error carrying the first few lines,
    /// so the operator sees what xorriso actually said.
    #[test]
    fn output_with_no_status_line_is_an_error_quoting_it() {
        let err = media(&lines(&["Drive current: ..."])).unwrap_err();
        assert!(
            err.starts_with("no Media status line in xorriso output: "),
            "{err}"
        );
        assert!(err.contains("Drive current: ..."), "{err}");
    }

    #[test]
    fn failures_are_classified_by_their_libburn_wording() {
        assert_eq!(
            classify_failure(&lines(&["libburn : SORRY : No disc loaded, not ready"])),
            FailureKind::NoMedia
        );
        assert_eq!(
            classify_failure(&lines(&[
                "libburn : NOTE : Failed to open device '/dev/sr0' : Permission denied"
            ])),
            FailureKind::Permission
        );
        assert_eq!(
            classify_failure(&lines(&[
                "stdio: could not open, No such file or directory"
            ])),
            FailureKind::NotFound
        );
        assert_eq!(
            classify_failure(&lines(&["xorriso : FAILURE : something else"])),
            FailureKind::Other
        );
    }

    #[test]
    fn the_version_line_is_parsed_and_an_old_one_still_parses() {
        assert_eq!(
            version(&lines(&["xorriso 1.5.8.pl02", "ISO 9660 ..."])),
            Some((1, 5, "1.5.8.pl02".to_string()))
        );
        assert_eq!(
            version(&lines(&["xorriso 1.4.2"])),
            Some((1, 4, "1.4.2".to_string()))
        );
        assert_eq!(version(&lines(&["garbage"])), None);
    }

    /// The real banner: the version is followed by the rest of the line, and
    /// only the version token belongs to the raw string.
    #[test]
    fn the_real_banner_line_parses() {
        assert_eq!(
            version(&lines(&[
                "xorriso 1.5.8.pl02 : RockRidge filesystem manipulator, libburnia project."
            ])),
            Some((1, 5, "1.5.8.pl02".to_string()))
        );
    }

    /// Captured from xorriso 1.5.8.pl02 on 2026-10-11; these go to stderr.
    #[test]
    fn a_writing_line_yields_its_tenths_of_a_percent() {
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Writing:      57344s    4.7%   fifo 100%  buf  50%"
            ),
            Some(47)
        );
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Writing:     848140s   69.5%   fifo  99%  buf  50%  902.2xD "
            ),
            Some(695)
        );
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Writing:   12207040s  100.0%   fifo   0%  buf   0%  3.1xB"
            ),
            Some(1000)
        );
    }

    /// From a real drive: disc 1A of a two-copy project burned on the Pioneer
    /// BDR-UD04 on 2026-10-11 (BD-R, `-close on`).
    #[test]
    fn real_drive_writing_lines_parse() {
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Writing:    1784672s   14.9%   fifo 100%  buf 100%    3.4xB"
            ),
            Some(149)
        );
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Writing:   11974920s  100.0%   fifo   0%  buf 100%    3.4xB"
            ),
            Some(1000)
        );
        assert_eq!(
            writing_per_mille(
                "xorriso : UPDATE : Closing track/session. Working since 1292 seconds"
            ),
            None
        );
    }

    #[test]
    fn other_lines_are_not_writing_progress() {
        for line in [
            "xorriso : UPDATE :       1 files added in 1 seconds",
            "ISO image produced: 1220727 sectors",
            "xorriso : UPDATE : Writing: garbage",
            "xorriso : UPDATE : Writing:      57344s    abc%",
            "",
        ] {
            assert_eq!(writing_per_mille(line), None, "{line:?}");
        }
    }
}
