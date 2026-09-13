//! The one place that converts between `chrono::DateTime<Utc>` and the
//! timestamp format frozen by the data-compatibility contract (spec §3.1).
//!
//! That format is whatever `java.time.Instant.toString()` produced: ISO-8601,
//! `Z`-suffixed, with zero fractional digits when the instant is whole-second
//! and otherwise 3, 6, or 9 — the shortest width that loses nothing.
//! `SecondsFormat::AutoSi` has the same rule. `tests/timestamps.rs` holds it to
//! that against every timestamp string in the fixture corpus.
//!
//! Both directions live here so the JSON and sidecar-cache writers
//! cannot quietly grow a second, differently-behaved formatter.

use chrono::{DateTime, SecondsFormat, Utc};

/// Render an instant the way the Scala build wrote it.
pub fn format(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// Parse a timestamp written by either implementation.
pub fn parse(s: &str) -> Result<DateTime<Utc>, TimeError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|source| TimeError {
            value: s.to_string(),
            source,
        })
}

/// A stored string that is not a timestamp. Reachable only from a corrupt or
/// hand-edited catalog, but it is data from outside the process, so it is an
/// error rather than a panic.
#[derive(Debug, thiserror::Error)]
#[error("not an ISO-8601 instant: {value:?}")]
pub struct TimeError {
    pub value: String,
    #[source]
    pub source: chrono::ParseError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// The four fraction widths `java.time.Instant.toString()` can emit, plus
    /// two shapes that look like edge cases and are not: a trailing-zero
    /// millisecond value and a single nanosecond.
    const SHAPES: &[&str] = &[
        "2026-06-01T00:00:00Z",
        "2026-09-05T20:01:03.260Z",
        "2026-09-05T20:01:06.100Z",
        "2026-09-05T20:01:06.123456Z",
        "2026-09-05T20:01:06.623959846Z",
        "2026-09-05T20:01:06.000000001Z",
    ];

    #[test]
    fn every_fraction_width_round_trips_byte_for_byte() {
        for s in SHAPES {
            let parsed = parse(s).expect("fixture-shaped timestamp parses");
            assert_eq!(&format(&parsed), s, "round trip changed {s}");
        }
    }

    #[test]
    fn a_whole_second_instant_has_no_fraction() {
        let at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert_eq!(format(&at), "2026-06-01T00:00:00Z");
    }

    #[test]
    fn a_non_utc_offset_parses_and_renders_as_utc() {
        // Nothing diskrement writes carries an offset, but a hand-edited
        // catalog might, and silently mis-parsing it would corrupt an ordering.
        let at = parse("2026-06-01T02:00:00+02:00").expect("offset form parses");
        assert_eq!(format(&at), "2026-06-01T00:00:00Z");
    }

    #[test]
    fn a_non_timestamp_is_rejected() {
        assert!(parse("").is_err());
        assert!(parse("yesterday").is_err());
        assert!(parse("2026-06-01").is_err());
    }
}
