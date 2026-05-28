# Change Log

All notable changes to this project will be documented in this file. This project adheres to [Semantic Versioning](http://semver.org/).

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

