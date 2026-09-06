//! The scenario content recipe, shared with `scripts/harvest-fixtures.sh:74`.

use sha2::{Digest, Sha256};

/// Deterministic bytes for one scenario file.
///
/// `sha256("<rel_path>@<version>")` rendered as lowercase hex, repeated and
/// truncated to `size`. Every byte is a function of `(rel_path, version, size)`,
/// so a scenario reproduces on any machine and is directly comparable with the
/// P0 corpus, which the harvest script generated with this same recipe.
pub fn content(rel_path: &str, version: u32, size: usize) -> Vec<u8> {
    let digest = Sha256::digest(format!("{rel_path}@{version}").as_bytes());
    let hex = format!("{digest:x}");
    hex.as_bytes().iter().copied().cycle().take(size).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_matches_the_committed_corpus() {
        // `fixtures/source/a.bin` is a.bin at version 1: the P0 scenario
        // modified it in cycle 2. Pinning against the corpus proves this
        // recipe is the harvest script's recipe, not merely a plausible one.
        let expected = std::fs::read(crate::repo_root().join("fixtures/source/a.bin"))
            .expect("the corpus is committed");
        assert_eq!(expected.len(), 800);
        assert_eq!(content("a.bin", 1, 800), expected);
    }

    #[test]
    fn content_is_the_hex_digest_repeated_and_truncated() {
        // sha256("a.bin@0") = aeeb7e5b02f890db450ac5e51ae3bf68…
        assert_eq!(content("a.bin", 0, 16), b"aeeb7e5b02f890db".to_vec());
        // 64 hex chars per cycle, so byte 64 restarts the digest.
        let long = content("a.bin", 0, 70);
        assert_eq!(&long[64..70], b"aeeb7e");
    }
}
