# Changelog

All notable changes to Rust Commander are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-11-28

### Added

- Show a notice when an open local file changes on disk. Press `r` to reload it
  in the background while keeping the displayed version available until the
  reload succeeds.
- Copy the viewer's loaded text or hex dump with `c` via OSC 52, up to
  449,991 bytes before encoding.

### Changed

- Store the selected skin in rc's own `settings.ini`. Settings and the default
  log live in `$XDG_CONFIG_HOME/rc/` when `XDG_CONFIG_HOME` is absolute, with
  `~/.config/rc/` as the fallback. Run `rc --import-mc-skin` to import a skin
  choice saved by Midnight Commander.
- Pipeline SFTP reads to speed up previews and copies on high-latency
  connections, handling out-of-order replies and short reads.
- Replace Chrono with Jiff for panel timestamps and ZIP, FTP/FTPS, and extfs
  listing dates.

### Fixed

- Preserve the completed copy and any overwrite backup when source cleanup
  fails during a cross-filesystem move. Report the partial move with the
  source, destination, and any backup path.
- Use short, randomized staging and backup names for copies, archive
  extraction, and overwrites to avoid filename-length failures.
- Preserve symlinked rc settings files when saving by updating their existing
  regular-file targets.

### Security

- Stage local copies before publishing them at the destination. On Unix,
  create staged files and directories with private permissions.

## [0.3.0] - 2026-11-15

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
- Open nested archives up to eight container levels, with bounded private temporary
  storage, navigation back to the enclosing archive, and selective previews/copies.
- Persist private, checksummed archive metadata indexes across sessions on Unix,
  rebuilding stale or corrupt entries without blocking access when caching fails.
- Browse installed MC extfs helper formats as read-only panels on Unix, including
  7z, RAR, RPM, DEB, ar, and cpio when the corresponding helpers are available.
- Add read-only SFTP and SSH Shell links on Unix with mandatory host-key
  verification, batch authentication, cancellable streaming reads, and reconnect
  workflows. Shell links use an in-memory Python helper without requiring SFTP.
- Add read-only FTP/FTPS links on Unix through curl, with verified TLS for
  authenticated connections, private netrc credentials, and anonymous-only plain FTP.
- Manage mounted connections through Active VFS list (`Ctrl-X a`): reopen them in
  either panel, reconnect remote links, or close mounts and cancel dependent jobs.
  Mounted panels support browsing, previews, selection totals, directory sizes,
  and F5 copies to local directories. Virtual FS settings control new mounts.
- Run commands in a persistent Unix PTY shell for Bash, Zsh, Fish, sh, and dash,
  retaining variables, functions, and working directories between commands.
  `Ctrl-O` switches between panels and the shell while programs keep running;
  terminal colors, full-screen applications, resizing, and bracketed paste work.
- Select groups using glob or regular-expression patterns, with case controls and
  optional directory inclusion, while preserving tags hidden by filters.
- Open arbitrary paths with View file and recall the session's viewed/edited-file
  history (`Alt-Shift-E`), including archive members.
- Recall command history (`Alt-Shift-H`) into the prompt without losing its draft.
- Stream Filtered view command output into a searchable text/hex viewer, with
  bounded capture, background execution, cancellation, and reopening from Jobs.
- Compare selected local files or panel directories in the output viewer, using
  difftastic when available for files and falling back to `diff`.
- Swap panels with `Ctrl-U`, preserving selection, tags, settings, navigation
  history, and pending asynchronous work.
- Calculate recursive directory sizes on demand with `Ctrl-Space` in cancellable
  background jobs, including archives and mounted backends.
- Assign and persist learned key bindings through Options → Learn keys, with
  immediate dispatch in the selected context.
- Define and save per-panel custom listing formats with validated columns and widths.
- Suspend and restore up to 16 open screens through Alt + backtick, preserving
  viewer/search state and allowing background searches and captured output to continue.
- Change permissions and ownership of selected or tagged entries on Unix with
  Chmod, Chown, and Advanced chown, including symbolic modes and cancellation.
- Create hard links, symlinks, and relative symlinks on Unix; atomically edit
  existing symlink targets on Linux/macOS with rollback and recovery reporting.
- Select and persist per-panel filename encodings on Unix, including legacy codecs,
  while retaining the original path bytes for filesystem operations.
- Edit effective MC extension, menu, and highlighting configuration files through
  the external editor, creating private user copies of installed defaults.
- Apply ordered `mc.ext.ini` Open/View/Edit rules and shared MC action macros,
  including native filename arguments, deferred prompts, `%view`, and `%cd`.
- Open conditional F2 user menus from trusted local, user, installed, or bundled
  files, using the shared action engine and MC condition syntax.
- Apply editable `filehighlight.ini` rules to panel colors, reload valid changes
  after editing, and retain the last valid rules when parsing fails.
- Scroll panels, quick previews, viewers, Quick cd suggestions, and scrollable
  modals with the mouse wheel or terminal-reported touchpad events.

### Changed

- Use typed locations and a shared cancellable read interface for native paths,
  archive members, and mounted backends, keeping virtual identities separate from
  local filesystem paths.
- Synchronize ready persistent-shell and panel directories while preserving newer
  panel navigation and unfinished shell input. Unsupported shells and custom
  command templates retain one-shot execution.
- Preserve Unix native filename bytes in path fields, search, editor arguments,
  shell insertion, completion, and directory changes through reversible path
  escapes and quoted native-argument references.

### Fixed

- Preserve safe non-UTF-8 TAR paths on Unix through navigation, previews, directory
  extraction, and Rename copies. Report unsupported names or destination filename
  encodings instead of silently omitting members from successful copies.
- Keep archive extraction staging private and recover safely from poisoned archive
  cache locks.
- Preserve local directory history across archive navigation and pending VFS mounts
  across directory refreshes.
- Let Quick cd leave virtual panels for their containing local directory, sharing
  native path parsing and home expansion across local and virtual panels.
- Reject virtual copy/move destinations before resolving local paths and decode
  unchanged copy-dialog destinations, including escaped filename bytes.
- Interrupt captured pipe readers during cancellation even when descendants retain
  output streams after the original command exits.
- Cancel superseded paused Find searches so stale work cannot resume or replace
  newer search results.
- Evaluate chmod's conditional execute (`X`) against permissions left by preceding
  symbolic clauses.
- Update release-package validation for the current internal crate dependencies.
- Allow the `bzip2-1.0.6` license used by the Rust bzip2 backend in dependency-policy
  checks.

### Security

- Require Tokio 1.53.3 or newer to include its RustSec advisory fix.
- Require rustix 1.1.6 or newer to address
  [RUSTSEC-2026-0486](https://rustsec.org/advisories/RUSTSEC-2026-0486.html).

### Documentation

- Refresh the README for 0.3.0 with release highlights, remote connection
  requirements, and expanded default controls.
- Keep the roadmap focused on deferred Windows work after the Unix features above.

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

[Unreleased]: https://github.com/hbbio/rc/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/hbbio/rc/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/hbbio/rc/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/hbbio/rc/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/hbbio/rc/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/hbbio/rc/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/hbbio/rc/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/hbbio/rc/releases/tag/v0.1.0
