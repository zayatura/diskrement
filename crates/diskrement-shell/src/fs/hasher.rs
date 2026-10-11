//! SHA-256 file hashing (`Hasher.scala`).

use std::io::Read;
use std::path::Path;

use diskrement_core::ContentHash;
use sha2::Digest;

/// Stream `path` in 64 KiB chunks and return the lowercase-hex SHA-256.
pub fn hash(path: &Path) -> std::io::Result<ContentHash> {
    hash_with(path, &mut |_| {})
}

/// `hash`, calling `on_read` with the size of every chunk as it is read. A
/// per-file progress bar stands still for minutes on one large file read from
/// an optical drive; this lets the caller advance it during the read.
pub fn hash_with(path: &Path, on_read: &mut dyn FnMut(u64)) -> std::io::Result<ContentHash> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
        on_read(n as u64);
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

    /// Reporting bytes as they are read: the callbacks add up to the file's
    /// size, and the digest is the one `hash` returns. 300,000 bytes spans
    /// five 64 KiB reads, the last one short.
    #[test]
    fn hash_with_reports_every_byte_read_and_agrees_with_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        for (name, len) in [("empty", 0usize), ("big", 300_000)] {
            let p = dir.path().join(name);
            let data: Vec<u8> = (0..len as u32).map(|i| (i % 251) as u8).collect();
            std::fs::write(&p, &data).unwrap();

            let mut reads = Vec::new();
            let digest = hash_with(&p, &mut |n| reads.push(n)).unwrap();
            assert_eq!(reads.iter().sum::<u64>(), len as u64, "{name}");
            assert!(reads.iter().all(|&n| n > 0), "{name}: {reads:?}");
            assert_eq!(digest, hash(&p).unwrap(), "{name}");
        }
    }

    #[test]
    fn fails_on_a_nonexistent_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = hash(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
