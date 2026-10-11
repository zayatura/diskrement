# diskrement-shell

Every effect diskrement performs: the SQLite catalog, file scanning and
hashing, CoW snapshots and staging, and the burners. The functional core
(`diskrement-core`) knows nothing about any of it.

## The catalog (phase P2)

One `rusqlite::Connection` per catalog file, no pool: diskrement opens the
catalog, does its work, and exits. Readers take `&self`; writers take
`&mut self` and go through `Catalog::tx`, whose transaction guard rolls back on
drop unless committed.

Three things here are frozen by the data-compatibility contract
(`docs/superpowers/specs/2026-09-05-rust-rewrite-design.md`, §3.1) and must not
be "improved":

- **The schema**, in `catalog/schema.rs`, transcribed statement for statement
  including both `check` constraints. `disc.status` accepts only `OPEN` and
  `FINALIZED`; `DEAD` is derived from `disc_event` at read time and never
  stored.
- **The timestamp format**, in `catalog/time.rs`: `java.time.Instant.toString()`
  shape, with 0, 3, 6, or 9 fractional digits.
- **The enum spellings** — `BD_R`, `ADDED`, `FINALIZED`, `MARKED_DEAD` and the
  rest — which live as `as_str`/`FromStr` pairs rather than serde renames.

## Gotchas

- **Open fixture databases with `?immutable=1`.** A plain read-only open of a
  WAL database creates `-wal`/`-shm` files beside it, which would dirty
  `fixtures/` and invalidate `SHA256SUMS`. `tests/support/mod.rs` does this
  correctly; copy that helper rather than reinventing it.
- **Test against a real file, not `:memory:`.** An in-memory database reports
  `journal_mode=memory` no matter what pragma you set, so a WAL assertion
  against one is vacuous. `catalog::testutil::temp_catalog` gives every test a
  real file in its own `TempDir`.
- **`VACUUM INTO` refuses an existing destination**, and must run outside a
  transaction. Both are relied on by the burn flow.

## Tests

Unit tests live beside their code. The files under `tests/` use the frozen
`fixtures/` corpus (V2 catalogs and staged discs) as input: `timestamps.rs`
(the wire format), `fixture_catalog.rs` (V2 catalogs migrate and read) and
`fixture_verify.rs` (staged discs the verifier must accept).
`xorriso_image.rs` runs the real `xorriso` against `stdio:` image files and
skips when it is not installed.
