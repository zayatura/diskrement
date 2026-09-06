//! SHA-256 file hashing (`Hasher.scala`).

use std::io::Read;
use std::path::Path;

use diskrement_core::ContentHash;
use sha2::Digest;

/// Stream `path` in 64 KiB chunks and return the lowercase-hex SHA-256.
pub fn hash(path: &Path) -> std::io::Result<ContentHash> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(ContentHash(format!("{:x}", digest.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_an_empty_file_to_the_sha256_of_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(
            hash(&p).unwrap(),
            ContentHash("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into())
        );
    }

    #[test]
    fn hashes_abc() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash(&p).unwrap(),
            ContentHash("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into())
        );
    }

    #[test]
    fn chunked_digest_matches_a_one_shot_digest_for_a_large_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("big");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let mut one_shot = sha2::Sha256::new();
        one_shot.update(&data);
        let expected = ContentHash(format!("{:x}", one_shot.finalize()));
        assert_eq!(hash(&p).unwrap(), expected);
    }

    #[test]
    fn fails_on_a_nonexistent_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = hash(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
