# Change Log

All notable changes to this project will be documented in this file. This project adheres to [Semantic Versioning](http://semver.org/).

## [1.0.13] - 2026-08-XX

### Added

### Changed
- `apply` now annotates server ids in `set` errors with the plan reference (`#client-id`) that produced them.

### Fixed

## [1.0.12] - 2026-07-28

### Added
- `upsert` and `reconcile` accept a `scope` restricting the slice of a type the operation owns, for matching and deletion (#18).
- `"matchOn": "*"` opts an `upsert`/`reconcile` into value matching explicitly (#18).

### Changed
- `apply` rejects an `upsert`/`reconcile` op whose entries share a match key.
- `reconcile` on a multi-variant type now converges the whole type, not only the variants named in the plan; use `scope` to narrow it (#18).
- `apply` rejects a `scope` on any op other than `upsert`/`reconcile`, an entry that falls outside its own operation's scope, and a `scope` on a server-derived property.
- **Breaking:** `apply` now rejects any unrecognised top-level key in a plan operation instead of ignoring it, so that a misspelled `scope` cannot silently widen a `reconcile` to the whole type.

### Fixed
- `apply` no longer creates a duplicate when a plan upserts the same match key twice.
- Value matching no longer reads a server-returned `null` as drift when the plan body omits the property.
- `reconcile` no longer destroys an object that a later operation in the same plan created, matched, or updated.
- `reconcile` no longer fails when two operations in one plan mark the same object for deletion.
- `apply` reports which value was wrong when `matchOn` is malformed.
- An ambiguous value match is rejected instead of silently picking the first candidate.
- Every paginated read now returns the whole type. `upsert`/`reconcile` duplicated objects and `reconcile` stopped converging, `destroy` reported success while leaving objects behind, and `snapshot` emitted a plan that looked complete but held only the first `maxObjectsInGet` objects.
- `apply` no longer creates duplicates when two entries of a scoped operation share a match key they inherit from the `scope`.
- An entry now inherits its operation's `scope`, so an omitted scoped property no longer creates an object outside the scope.
- Concurrent invocations no longer corrupt the schema cache; each writer now uses its own temporary file.

## [1.0.11] - 2026-07-20

### Added
- `reconcile` plan op for `apply`: an exhaustive `upsert` that also destroys objects absent from the plan (#18).

### Changed

### Fixed
- `matchOn` now resolves `#client-id` references on reference fields (#17).

## [1.0.10] - 2026-07-02

### Added

### Changed

### Fixed
- `upsert` removed required `matchOn` field.

## [1.0.9] - 2026-06-24

### Added
- Idempotent snapshots with `upsert` support.

### Changed

### Fixed
- Return friendly error message with the HTTP schema is missing (#16).

## [1.0.8] - 2026-05-28

### Added

### Changed

### Fixed
- `snapshot` now handles multi-variant top-level Objects whose variants include marker-only ones and multi-variant Singletons.

## [1.0.7] - 2026-05-20

### Added

### Changed
- `apply`: missing update `id` error now points at the top-level field, not the `value` keys.

### Fixed
- Secret-typed fields are now printed verbatim from the server.
- `snapshot` no longer corrupts terminal output by interleaving progress messages mid-record.

## [1.0.6] - 2026-05-11

### Added
- `--debug` flag and `STALWART_DEBUG` env var to log HTTP traffic to stderr.

### Changed
- Schema-fetch parse errors now include status, content-type, byte length, and a body snippet.

### Fixed
- `create` on a multi-variant object whose selected variant carries no payload (#8).
- Schema cache no longer poisoned by non-JSON responses; the body is parsed before being written to disk (#9).
- Corrupt cached schema is now invalidated on the offline fallback so the next run fetches cleanly (#9).

## [1.0.5] - 2026-05-05

### Added

### Changed

### Fixed
- `snapshot` drops embedded multi-variant fields whose value is a marker-only variant (#7).

## [1.0.4] - 2026-04-28

### Added
- `aarch64-unknown-linux-musl` target.

### Changed

### Fixed

## [1.0.3] - 2026-04-27

### Added

### Changed
- `snapshot` errors more clearly when the user passes the name of an embedded type (e.g. `Credential`).
- When a remaining cycle has only immutable edges, the error now lists only the strongly-connected nodes.

### Fixed
- `snapshot` now breaks dependency cycles between selected types by deferring the cycle-closing field.
- `snapshot` recommends `--allow-unresolved <T>` instead of "add T" when adding `T` to the selection would itself form a cycle.

## [1.0.2] - 2026-04-25

### Added

### Changed
- `query --json` now emits NDJSON instead of a single JSON array.
- `snapshot` output is now NDJSON. `apply` reads the
  same format. The previous JSON-array form is no longer accepted.
- `update` now errors when the server returns neither `updated[id]` nor
  `notUpdated[id]`

### Fixed
- Fix `snapshot` and `apply` for multi-variant types (#4)

## [1.0.1] - 2026-04-25

### Added

### Changed

### Fixed
- Allow JSON schema to be uncompressed.

## [1.0.0] - 2026-04-18

### Added
- Initial release.

### Changed

### Fixed

