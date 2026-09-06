//! The SQLite catalog: schema, migrations, and every persistence operation.
//!
//! One `rusqlite::Connection`, no pool. diskrement opens the catalog, does its
//! work, and exits; there is no concurrency to pool for (spec §5.5).

pub mod time;
