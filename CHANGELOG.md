# Changelog

All notable changes to Rust Commander are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Browse ZIP archives as read-only folders in either pane without decompressing
  their contents. Copy selected or tagged files and directories out with F5,
  decompressing only those entries, and preview individual files with Enter,
  F3, or Quick view.
- Browse TAR, gzip TAR (`.tar.gz`, `.tgz`), and bzip2 TAR (`.tar.bz2`, `.tbz2`,
  `.tbz`) through the same read-only archive backend, with selective F5 extraction.
  Share cached metadata between panes, seek directly in plain TAR, and resume
  compressed TAR reads through bounded gzip checkpoints or bzip2 block indexes.
  Validate compressed streams during the first scan and invalidate stale indexes.

### Fixed

- Preserve safe non-UTF-8 TAR paths on Unix through navigation, previews, directory
  extraction, and Rename copies. Report unsupported names or destination filename
  encodings instead of silently omitting members from successful copies.
- Allow the `bzip2-1.0.6` license used by the Rust bzip2 backend in dependency-policy
  checks.

## [0.2.2] - 2026-10-12

### Fixed

- Kept completed overwrite copies and moves successful when backup cleanup fails,
  logging the leftover backup path instead of reporting a failed transfer.
- Preserved dangling symlinks under Skip and Rename copy/move policies and corrected
  related staging, backup, and rollback handling.
- Fixed hex-view search crashes and incorrect match offsets around multibyte or
  invalid UTF-8, including cursor-crossing matches and switches between text and hex modes.
- Rejected named pipes and other unsupported special files before copying, preventing
  FIFO opens from blocking cancellation and shutdown even if the source is replaced before opening.
- Kept panelize cancellation active after the shell exits while descendants retain
  captured output streams.
- Removed quadratic tag-retention work from directory refreshes without discarding
  tags hidden by active filters.
- Kept large viewer jumps responsive with incremental syntax-highlighting budgets
  and indexed access to visible lines, showing plain text while highlighting catches
  up or after an oversized line. Syntax detection now uses the loaded file contents.

## [0.2.1] - 2026-10-11

### Added

- Added automatic refresh of both file panes and the open directory tree when
  filesystem changes occur, including changes made by other programs, while
  preserving selection and tags.
- Added click-to-select and double-click activation in either file pane across
  Full, Brief, and Long layouts, preserving the visible window during mouse selection.

## [0.2.0] - 2026-08-30

### Added

- Added a modal Unix command line in the active panel directory with configurable
  Auto, Fish, and custom-shell execution, bounded history, draft preservation,
  Fish-aware or generic completion, and synchronized literal `cd`.
- Added cancelable background completion with live prefix filtering and
  current-directory files ranked ahead of shell-provider candidates.
- Added MC-compatible insertion of selected files, full paths, and tagged files
  from either panel, with literal shell quoting.
- Added contextual command-line help and a responsive bottom bar containing only
  prompt actions.
- Added `Ctrl-X y` to copy the active selected entry's full path to the terminal
  clipboard via OSC 52.

### Changed

- Abbreviate refresh-resolved, filesystem-equivalent current-home paths as `~` in pane toplines,
  align paths left and listing summaries right, and omit empty tag counts.
- Split Enter activation from F3 viewing: Enter now executes runnable files or opens
  documents with the operating system's configured application, while F3 always uses
  the internal viewer.
- Run executable files with a dedicated foreground process group on Unix, and protect rc with
  a process-local console-control handler on Windows, so interrupting a child does not terminate
  the file manager.
- Dispatch desktop document launches asynchronously and fall back to the internal viewer on
  failure. Linux prefers the status-aware desktop portal, tries the complete xdg/GIO/GNOME/KDE
  chain without requiring `xdg-mime`, and releases accepted blocking launchers to an independent
  reaper so shutdown never terminates the opened application. Windows delegates
  association-backed script formats to `ShellExecuteExW` instead of passing them to
  `CreateProcessW`.

### Fixed

- Kept terminal restoration owned by the UI thread after caught worker panics and
  bounded completion shutdown independently of uncooperative filesystem calls.
- Made literal `cd` normalize dot components, preserve symlink semantics, commit
  only after panel refresh, preserve repeated-slash home expansion, and delegate
  unsupported Fish escapes.
- Corrected completion for prompt-relative `PATH` entries, directory symlinks,
  terminal `.` and `..` components, and Fish keyword executables.
- Finalized completion and literal `cd` transactions beneath Help overlays,
  cleared successful `cd` drafts after closing the prompt, and rendered the
  configured Help shortcut in command-line help.
- Normalized shifted character input and kept command-line cursor metrics aligned
  with rendered Unicode widths.
- Escaped persisted custom-shell fields and quoted panel-derived filenames
  literally, including a file named exactly `~`.

### Documentation

- Reworked the README around supported capabilities and default controls.
- Consolidated unfinished product work into the root-level `ROADMAP.md` and
  removed superseded planning documents.

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

[Unreleased]: https://github.com/hbbio/rc/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/hbbio/rc/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/hbbio/rc/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/hbbio/rc/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/hbbio/rc/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/hbbio/rc/releases/tag/v0.1.0
