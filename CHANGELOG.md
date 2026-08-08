# Changelog

All notable changes to Rust Commander are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-08-05

### Fixed

- Restored Midnight Commander function-key semantics: `F2` is reserved for the user menu,
  `F6` performs rename/move, and `F9` opens the pull-down menu bar.
- Made the button bar resolve the command bound to each physical function key instead of
  borrowing another shortcut and potentially displaying duplicate key numbers.

### Documentation

- Added crates.io installation instructions and a current interface screenshot.

## [0.1.0] - 2026-08-05

### Added

- Responsive dual-pane file management with asynchronous copy, move, rename, create,
  delete, progress, cancellation, and overwrite handling.
- Read-only file viewer with search, goto, wrapping, hexadecimal mode, and syntax
  highlighting, plus an external-editor workflow.
- Find, directory tree, labeled hotlist, external panelize, and Quick CD workflows.
- Per-pane Full, Brief, and Long listing formats; complete sort controls; glob and regular
  expression filters; Info and Quick view modes.
- Keyboard and mouse interaction, including stable double-click activation and responsive
  horizontal navigation in Brief format.
- Persisted settings and embedded `mc`-compatible skins.

### Reliability

- Preserved operation targets when filters hide tagged entries.
- Counted complete, deduplicated directory contents in tagged-selection totals.
- Kept panel state usable when directory access is denied and entered newly created
  directories automatically.
- Made streaming whole-word search boundary-correct and tree searches selection-safe.
- Added bounded, cancelable, request-correlated background work throughout expensive
  filesystem workflows.

[0.1.1]: https://github.com/hbbio/rc/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/hbbio/rc/releases/tag/v0.1.0
