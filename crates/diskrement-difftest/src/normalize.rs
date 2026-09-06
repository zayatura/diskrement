//! Volatile-value normalization.
//!
//! **Every rule in this module is a hole punched in the differential oracle.**
//! A rule that erases more than it must silently converts a real difference into
//! a pass, which is the one failure mode this harness cannot detect in itself.
//! So each rule is a named method carrying a doc comment that says exactly what
//! varies between two runs and why erasing it is safe. Adding a rule without
//! that justification is a review failure, not a style preference.
//!
//! One `Normalizer` serves one run, and every value from that run — command
//! stdout, catalog dumps, staged JSON — passes through the same instance, so
//! instant ordinals are consistent across the whole run.
//!
//! Deliberately *not* normalized: the wording of clap's and zio-cli's argument
//! errors, which spec §3.2 sanctions as free to drift. That exemption is
//! declared per step, as `Expect::ExitOnly` in the scenario, so it is visible
//! where it applies instead of being a pattern that quietly covers everything.

use std::path::Path;

use serde_json::{Map, Value};

pub struct Normalizer {
    scratch_root: String,
    seen_instants: Vec<String>,
}

impl Normalizer {
    pub fn new(scratch_root: &Path) -> Self {
        Self {
            scratch_root: scratch_root.display().to_string(),
            seen_instants: Vec::new(),
        }
    }

    /// Apply every rule to one blob of text.
    pub fn text(&mut self, input: &str) -> String {
        let stage = self.rule_scratch_root(input);
        self.rule_instant_ordinals(&stage)
    }

    /// Recursively normalize every string in a JSON value — keys included,
    /// since a key is as capable of carrying a path as a value is. Numbers and
    /// booleans pass through: nothing volatile is encoded as one.
    pub fn json(&mut self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    let key = self.text(k);
                    let val = self.json(v);
                    out.insert(key, val);
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }

    /// **Rule: scratch root.** The two runs being compared live in different
    /// temp directories by construction, and the path appears both in recorded
    /// argv and in `status` output. Safe because the *relative* structure below
    /// the root is preserved verbatim, so a real layout difference still shows.
    fn rule_scratch_root(&self, input: &str) -> String {
        input.replace(&self.scratch_root, "<scratch>")
    }

    /// **Rule: instant ordinals.** Wall-clock values differ between runs by
    /// nature. The `n`th distinct instant becomes `<tn>` in first-appearance
    /// order, which erases absolute values while preserving every equality and
    /// distinctness relation between them — so two rows written in one batch
    /// still compare equal in both runs, and would still diverge if one
    /// implementation stopped batching.
    fn rule_instant_ordinals(&mut self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut i = 0usize;
        while i < input.len() {
            if let Some(end) = instant_at(input, i) {
                let text = &input[i..end];
                let n = match self.seen_instants.iter().position(|s| s == text) {
                    Some(n) => n,
                    None => {
                        self.seen_instants.push(text.to_string());
                        self.seen_instants.len() - 1
                    }
                };
                out.push_str(&format!("<t{n}>"));
                i = end;
                continue;
            }
            // `i` is always on a char boundary: it starts at 0 and advances
            // either by a whole char or to `end`, which follows an ASCII 'Z'.
            let ch = input[i..].chars().next().expect("i is a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }
}

/// If a `java.time.Instant.toString()` value starts at byte `i`, its exclusive
/// end index.
///
/// Shape: `YYYY-MM-DDTHH:MM:SS`, an optional `.` plus one or more digits, then
/// `Z`. Hand-written rather than a regex so the harness adds no dependency, and
/// so the accepted shape is legible next to spec §3.1's description of it.
fn instant_at(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let digit = |k: usize| b.get(k).is_some_and(|c| c.is_ascii_digit());
    let lit = |k: usize, c: u8| b.get(k) == Some(&c);

    for k in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !digit(i + k) {
            return None;
        }
    }
    if !lit(i + 4, b'-') || !lit(i + 7, b'-') || !lit(i + 10, b'T') {
        return None;
    }
    if !lit(i + 13, b':') || !lit(i + 16, b':') {
        return None;
    }
    let mut end = i + 19;
    if lit(end, b'.') {
        let mut j = end + 1;
        while digit(j) {
            j += 1;
        }
        // A lone '.' with no digits is not a fraction; leave `end` where it is
        // and let the 'Z' check below reject the whole match.
        if j > end + 1 {
            end = j;
        }
    }
    lit(end, b'Z').then_some(end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn norm() -> Normalizer {
        Normalizer::new(Path::new("/tmp/scratch-abc"))
    }

    #[test]
    fn the_scratch_root_becomes_a_placeholder() {
        let mut n = norm();
        assert_eq!(
            n.text("staged at /tmp/scratch-abc/work/staging/disc-1"),
            "staged at <scratch>/work/staging/disc-1"
        );
    }

    #[test]
    fn distinct_instants_get_ordinals_in_first_appearance_order() {
        let mut n = norm();
        let input = "a=2026-09-05T20:01:03.260Z b=2026-09-05T20:01:04.900788637Z \
                     c=2026-09-05T20:01:03.260Z";
        assert_eq!(n.text(input), "a=<t0> b=<t1> c=<t0>");
    }

    #[test]
    fn ordinals_continue_across_calls_so_a_whole_run_shares_one_numbering() {
        let mut n = norm();
        assert_eq!(n.text("2026-01-01T00:00:00Z"), "<t0>");
        assert_eq!(n.text("2026-01-02T00:00:00Z"), "<t1>");
        assert_eq!(n.text("2026-01-01T00:00:00Z"), "<t0>");
    }

    #[test]
    fn whole_second_and_fractional_instants_are_both_recognised() {
        // java.time.Instant.toString() omits the fraction when it is zero and
        // otherwise emits 3, 6 or 9 digits (spec §3.1).
        let mut n = norm();
        assert_eq!(
            n.text(
                "2026-01-01T00:00:00Z 2026-01-01T00:00:00.123Z \
                    2026-01-01T00:00:00.123456Z 2026-01-01T00:00:00.123456789Z"
            ),
            "<t0> <t1> <t2> <t3>"
        );
    }

    #[test]
    fn near_misses_are_left_alone() {
        let mut n = norm();
        // No trailing Z: a local timestamp is not an instant, and silently
        // erasing one would hide a real format difference.
        assert_eq!(n.text("2026-01-01T00:00:00"), "2026-01-01T00:00:00");
        // Not a date at all.
        assert_eq!(n.text("version 2026-01"), "version 2026-01");
        assert_eq!(n.text("12345678901234567890"), "12345678901234567890");
    }

    #[test]
    fn json_strings_and_keys_are_normalised_but_numbers_are_not() {
        let mut n = norm();
        let v: serde_json::Value = serde_json::from_str(
            r#"{"burnedAt":"2026-01-01T00:00:00Z","sizeBytes":800,
                "paths":["/tmp/scratch-abc/source/a.bin"]}"#,
        )
        .unwrap();
        let got = n.json(&v);
        assert_eq!(got["burnedAt"], serde_json::json!("<t0>"));
        assert_eq!(got["sizeBytes"], serde_json::json!(800));
        assert_eq!(got["paths"][0], serde_json::json!("<scratch>/source/a.bin"));
    }
}
