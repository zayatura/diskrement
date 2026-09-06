//! The parallel scanner (`FileScanner.scala`, `HashParallelism.scala`,
//! `FileSystem.scala`).

/// `DISKREMENT_HASH_PARALLELISM` resolution: default 6, clamped to 64,
/// empty/whitespace-only treated as unset, unusable values fall back to 6 with
/// a warning carrying the raw value (`HashParallelism.scala`).
pub struct Parallelism {
    pub value: usize,
    pub warnings: Vec<String>,
}

pub const DEFAULT_PARALLELISM: usize = 6;
const MAX_PARALLELISM: usize = 64;

pub fn parallelism_from_env() -> Parallelism {
    resolve_parallelism(|k| std::env::var(k).ok())
}

pub fn resolve_parallelism(get_env: impl Fn(&str) -> Option<String>) -> Parallelism {
    let Some(raw) = get_env("DISKREMENT_HASH_PARALLELISM") else {
        return Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: Vec::new(),
        };
    };
    if raw.trim().is_empty() {
        return Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: Vec::new(),
        };
    }
    match raw.trim().parse::<usize>() {
        Ok(n) if (1..=MAX_PARALLELISM).contains(&n) => {
            Parallelism { value: n, warnings: Vec::new() }
        }
        Ok(n) => Parallelism {
            value: MAX_PARALLELISM,
            warnings: vec![format!(
                "clamping DISKREMENT_HASH_PARALLELISM {n} to {MAX_PARALLELISM}"
            )],
        },
        Err(_) => Parallelism {
            value: DEFAULT_PARALLELISM,
            warnings: vec![format!(
                "ignoring unusable DISKREMENT_HASH_PARALLELISM '{raw}' (want a positive int); using {DEFAULT_PARALLELISM}"
            )],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults_to_6_when_the_env_var_is_unset() {
        assert_eq!(resolve_parallelism(env(&[])).value, 6);
    }

    #[test]
    fn reads_a_positive_value() {
        let r = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "3")]));
        assert_eq!(r.value, 3);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn ignores_non_numeric_and_non_positive_values_with_a_warning() {
        for raw in ["banana", "0", "-1", ""] {
            let r = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", raw)]));
            assert_eq!(r.value, 6, "raw={raw:?}");
            assert!(!r.warnings.is_empty(), "raw={raw:?} must warn");
        }
    }

    #[test]
    fn trims_whitespace_and_clamps_huge_values() {
        assert_eq!(
            resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", " 3")])).value,
            3
        );
        let clamped = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "100000")]));
        assert_eq!(clamped.value, 64);
        assert_eq!(
            clamped.warnings[0],
            "clamping DISKREMENT_HASH_PARALLELISM 100000 to 64"
        );
        // whitespace-only is treated as unset, with no warning
        let ws = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "  ")]));
        assert_eq!(ws.value, 6);
        assert!(ws.warnings.is_empty());
        // the unusable warning carries the raw, untrimmed value verbatim
        let bad = resolve_parallelism(env(&[("DISKREMENT_HASH_PARALLELISM", "banana")]));
        assert_eq!(
            bad.warnings[0],
            "ignoring unusable DISKREMENT_HASH_PARALLELISM 'banana' (want a positive int); using 6"
        );
    }
}
