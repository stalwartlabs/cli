# Change Log

All notable changes to this project will be documented in this file. This project adheres to [Semantic Versioning](http://semver.org/).

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

