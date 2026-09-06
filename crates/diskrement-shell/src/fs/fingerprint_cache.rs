//! The fingerprint sidecar (`FingerprintCache.scala`): a disposable,
//! upsert-able SQLite cache of size+mtime→hash observations, deliberately
//! outside the append-only catalog. A corrupt or version-incompatible file is
//! deleted and recreated empty; every other failure is a soft error the
//! caller treats as a warning.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;
use diskrement_core::{ContentHash, RelPath};
use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::catalog::model::FileFingerprint;
use crate::catalog::time;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
#[error("fingerprint cache I/O error")]
pub struct CacheError(#[source] pub rusqlite::Error);

pub struct FingerprintCache {
    path: PathBuf,
}

#[derive(Clone, Copy)]
enum Op {
    Read,
    Write,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Read => "read",
            Op::Write => "write",
        }
    }
}

impl FingerprintCache {
    pub fn new(path: PathBuf) -> Self {
        FingerprintCache { path }
    }

    pub fn read_all(&self) -> Result<BTreeMap<RelPath, FileFingerprint>, CacheError> {
        self.guarded(Op::Read, |conn| {
            let mut stmt =
                conn.prepare("select rel_path, size_bytes, mtime, content_hash from fingerprint")?;
            let mut rows = stmt.query([])?;
            let mut out = BTreeMap::new();
            let mut skipped = 0usize;
            while let Some(row) = rows.next()? {
                let mtime: String = row.get(2)?;
                let Ok(mtime) = time::parse(&mtime) else {
                    skipped += 1;
                    continue;
                };
                out.insert(
                    RelPath(row.get::<_, String>(0)?),
                    FileFingerprint {
                        size_bytes: row.get::<_, i64>(1)?.max(0) as u64,
                        mtime,
                        content_hash: ContentHash(row.get(3)?),
                    },
                );
            }
            if skipped > 0 {
                eprintln!(
                    "fingerprint cache at {}: skipped {skipped} unreadable row(s)",
                    self.path.display()
                );
            }
            Ok(out)
        })
    }

    pub fn put_all(&self, entries: &BTreeMap<RelPath, FileFingerprint>) -> Result<(), CacheError> {
        let updated_at = time::format(&Utc::now()); // one stamp for the whole batch
        self.guarded(Op::Write, |conn| {
            // `unchecked_transaction`: the cache hands out `&Connection`; the
            // rollback-on-drop guard is the same as `transaction`.
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "insert into fingerprint (rel_path, size_bytes, mtime, content_hash, updated_at)
                     values (?1, ?2, ?3, ?4, ?5)
                     on conflict(rel_path) do update set
                       size_bytes = excluded.size_bytes, mtime = excluded.mtime,
                       content_hash = excluded.content_hash, updated_at = excluded.updated_at",
                )?;
                for (rel, fp) in entries {
                    stmt.execute(rusqlite::params![
                        rel.as_str(),
                        fp.size_bytes as i64,
                        time::format(&fp.mtime),
                        fp.content_hash.as_str(),
                        updated_at,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// One connection per call: the wipe-and-retry path must be able to
    /// delete the file between calls (`FingerprintCacheLive.openConn`).
    fn guarded<T>(
        &self,
        op: Op,
        f: impl Fn(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, CacheError> {
        match self.attempt(&f) {
            Ok(t) => Ok(t),
            Err(e) if is_corrupt(&e) => {
                eprintln!(
                    "fingerprint cache at {} is corrupt ({e}); recreating empty",
                    self.path.display()
                );
                self.wipe();
                self.attempt(&f).map_err(CacheError)
            }
            Err(e) => {
                eprintln!(
                    "fingerprint cache {} failed at {}: {e}",
                    op.name(),
                    self.path.display()
                );
                Err(CacheError(e))
            }
        }
    }

    fn attempt<T>(
        &self,
        f: &impl Fn(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, rusqlite::Error> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let uri = format!("file:{}", self.path.display());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(
                    "create table if not exists fingerprint (
                       rel_path     text primary key,
                       size_bytes   integer not null,
                       mtime        text not null,
                       content_hash text not null,
                       updated_at   text not null)",
                )?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            v if v == SCHEMA_VERSION => {}
            other => {
                // Our own corruption signal: consumed by `is_corrupt` and
                // turned into a wipe-and-recreate (the Scala code threw
                // IllegalStateException for this).
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                    Some(format!("cache schema v{other}, expected v{SCHEMA_VERSION}")),
                ));
            }
        }
        f(&conn)
    }

    /// Delete the file and its SQLite sidecars; errors ignored (`wipe`).
    fn wipe(&self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

/// The only triggers for wipe-and-recreate: SQLite saying the file is not a
/// usable database, or our own version-mismatch signal. Everything else —
/// locked, busy, permissions — is a soft failure that must NOT delete the
/// file (pinned by the lock test).
fn is_corrupt(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::SqliteFailure(ffi, _) => {
            matches!(
                ffi.code,
                ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt
            )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskrement_core::ContentHash;

    fn fp(size: u64, mtime: &str, h: &str) -> FileFingerprint {
        FileFingerprint {
            size_bytes: size,
            mtime: crate::catalog::time::parse(mtime).unwrap(),
            content_hash: ContentHash(h.into()),
        }
    }

    fn map(entries: &[(&str, FileFingerprint)]) -> BTreeMap<RelPath, FileFingerprint> {
        entries
            .iter()
            .map(|(r, f)| (RelPath(r.to_string()), f.clone()))
            .collect()
    }

    #[test]
    fn read_all_on_a_missing_file_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = FingerprintCache::new(dir.path().join("proj.scancache.sqlite"));
        assert!(cache.read_all().unwrap().is_empty());
    }

    #[test]
    fn put_all_then_read_all_round_trips_and_put_all_upserts() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = FingerprintCache::new(dir.path().join("proj.scancache.sqlite"));
        let first = map(&[
            ("a", fp(1, "2026-06-01T00:00:00Z", "h1")),
            ("b", fp(2, "2026-06-01T00:00:01Z", "h2")),
        ]);
        cache.put_all(&first).unwrap();
        let second = map(&[("a", fp(9, "2026-06-01T00:00:02Z", "h9"))]);
        cache.put_all(&second).unwrap();
        let got = cache.read_all().unwrap();
        assert_eq!(
            got.get(&RelPath("a".into())).unwrap().content_hash.as_str(),
            "h9"
        );
        assert_eq!(
            got.get(&RelPath("b".into())).unwrap().content_hash.as_str(),
            "h2"
        );
    }

    #[test]
    fn the_schema_version_stamp_is_1() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        let v: i64 = conn
            .query_row("pragma user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
    }

    #[test]
    fn a_corrupt_cache_is_recreated_empty_and_stays_usable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        std::fs::write(&path, b"not a database at all").unwrap();
        let cache = FingerprintCache::new(path);
        assert!(cache.read_all().unwrap().is_empty()); // recreate-on-read
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        assert_eq!(cache.read_all().unwrap().len(), 1);
    }

    #[test]
    fn a_future_schema_version_is_recreated_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("pragma user_version = 99").unwrap();
        drop(conn);
        assert!(cache.read_all().unwrap().is_empty());
    }

    #[test]
    fn a_row_with_an_undecodable_mtime_is_skipped_not_fatal() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        cache
            .put_all(&map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]))
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "insert into fingerprint values ('bad', 2, 'not-a-timestamp', 'h2', '2026-06-01T00:00:00Z')",
            [],
        )
        .unwrap();
        drop(conn);
        let got = cache.read_all().unwrap();
        assert_eq!(got.len(), 1);
        assert!(got.contains_key(&RelPath("a".into())));
    }

    #[test]
    fn a_locked_file_fails_softly_and_never_destroys_a_healthy_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("proj.scancache.sqlite");
        let cache = FingerprintCache::new(path.clone());
        let entries = map(&[("a", fp(1, "2026-06-01T00:00:00Z", "h1"))]);
        cache.put_all(&entries).unwrap();
        // Hold the write lock from a second connection; busy_timeout=5000
        // makes this test take ~5s, exactly as the Scala spec's does.
        let guard = rusqlite::Connection::open(&path).unwrap();
        guard.execute_batch("begin immediate").unwrap();
        let err = cache.put_all(&entries).unwrap_err();
        assert!(!err.0.to_string().is_empty());
        drop(guard);
        assert_eq!(cache.read_all().unwrap().len(), 1); // not wiped
    }
}
