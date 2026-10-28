# rust commander (rc)

![Rust Commander dual-pane terminal interface](https://raw.githubusercontent.com/hbbio/rc/main/assets/rc.png)

*rust commander* is a Rust terminal file manager inspired by GNU Midnight
Commander. It keeps the familiar dual-pane workflow and MC-style default
keybindings while adding responsive background operations, a modal shell prompt,
and fast directory navigation.

## Features

### Panels and navigation

- Two independent directory panels with Full, Brief, and Long layouts.
- Browse ZIP, TAR, gzip TAR, and bzip2 TAR archives as read-only folders, with
  normal navigation, sorting, filtering, and tagging.
- Native filesystem notifications automatically refresh both panels and the open
  directory tree, including changes made by other programs. Updates are coalesced
  and scanned in the background while preserving selection and tags.
- Per-panel sorting, reverse order, glob or regular-expression filters, hidden-file
  control, and persisted configuration.
- Tagging for batch operations, recursive selection totals, responsive Brief-mode
  columns, and mouse selection or activation.
- Select or unselect groups with glob or regular-expression patterns, optional
  case-insensitive matching, and control over including directories. These
  commands affect visible entries and preserve tags hidden by a listing filter.
- Quick view and file information in the passive panel.
- Quick CD for exact paths, `~`, Unix `~user`, `cd -`, and ranked
  case-insensitive directory search.
- Directory tree navigation with static or dynamic movement, incremental search,
  subtree rescan, and subtree forgetting.
- A labeled, persistent directory hotlist with add, edit, remove, and quick-add.

### Files and jobs

- Copy, move, rename, mkdir, and delete for individual or tagged entries.
- Background execution with progress, cancellation, overwrite policies, and a jobs
  screen.
- Metadata preservation, symlink-aware operations, validation against recursive
  destinations, and rollback when an overwrite fails.
- Incremental, cancelable directory refreshes that preserve selection and tags.

### Archives

Press `Enter` on a `.zip`, `.tar`, `.tar.gz` (`.tgz`), or `.tar.bz2` (`.tbz2`,
`.tbz`) file to browse it in the current pane. Extensions are case-insensitive.
`Enter` opens folders and previews files; `F3` and Quick view also preview files.
`Backspace` or `..` goes up, leaving the archive at its root. Normal sorting,
filtering, hidden-file control, and tagging work inside archives. Unsorted mode
follows archive order. Inferred folders use their newest descendant's modification
time for date sorting.

Use `F5` to copy selected or tagged files and directories to a local directory.
Only selected files and the descendants of selected directories are written.
Copy jobs support progress, cancellation, and Skip, Rename, and Overwrite policies.
Move, delete, rename, edit, mkdir, and copying into archives are blocked; archive
panes are labeled read-only. The command line, Find, directory tree, external
panelize, and adding the current directory to the hotlist require a local directory.

ZIP browsing reads directory metadata without decompressing payloads. Plain TAR
browsing reads headers and seeks past payloads. Opening a compressed TAR for the
first time requires a cancellable decompression scan to build its directory and
seek index. No temporary directory tree or full decompressed TAR is written.
Indexes are shared between panes and reused for navigation, sorting, size totals,
and previews/copies during the session. Gzip reads resume from decoder checkpoints;
bzip2 reads decode the required blocks and reuse one block per reader. Reading a
selected file may therefore also decompress preceding bytes in its checkpoint
interval or compression block. ZIP copies decompress only selected entries.

The archive cache retains up to eight recent indexes with a soft 64 MiB budget;
gzip checkpoint memory is separately bounded by adapting checkpoint spacing as
archives grow. A large metadata index can exceed the soft budget to keep navigation
usable. Indexes are invalidated when the source file's identity, size, or timestamps
change. Use manual refresh to update an open pane after external changes.

ZIP Stored/Deflate entries and TAR regular files/directories are supported,
including GNU long names and PAX paths, sizes, and modification times. Repeated TAR
names use the last record. Encrypted ZIP contents, archive symlinks, hard links,
sparse files, special files, and nested archive browsing are not supported for
viewing/extraction. Unsafe member paths are omitted.

On Unix, member paths retain their original bytes, including non-UTF-8 names;
display labels may contain replacement characters. Unsupported names produce an
explicit error during indexing or extraction. Directory copies restore an
overwritten destination if its filesystem cannot represent a member name.

Archive panes and jobs use a common read-only backend interface in
[`crates/core/src/archive`](crates/core/src/archive.rs). Container backends supply
immutable metadata and member readers; TAR compression readers supply indexed
seeking independently. Adding another format does not require format-specific
navigation, viewer, or copy flows. This archive layer does not yet provide the
unified local/remote VFS or nested mounts described in the roadmap.

Automatic refresh uses FSEvents on macOS, inotify on Linux, and
ReadDirectoryChangesW on Windows. Watches are shared across directory aliases;
the tree only refreshes affected cached subtrees. Native notifications may be
unavailable on network or virtual filesystems; use manual refresh there. Watch
registration failures are reported in the status line and retried in the background.
Writes to the active tracing log are excluded from automatic invalidations so
refresh logging cannot trigger another refresh. Log creation, deletion, and
renaming still update the panels.

### Find and panelize

- Filename search with shell patterns or regular expressions.
- Optional content search, whole-word matching, ignored directories, pause,
  cancellation, and bounded partial-result reporting.
- Find results can be opened directly or placed into a panel.
- External panelize streams command output into a virtual panel and supports named,
  persistent presets and per-panel result history.

### Viewer and external programs

- Internal text and hex viewer with syntax highlighting, search in both directions,
  goto, wrapping, and bounded previews for large files.
- External editing through a configured command, `$EDITOR`, `$VISUAL`, or
  detected editors (`hx`, `nvim`, `vim`, `vi`, `emacs`).
- Executables run in the terminal; documents open with the operating system's
  configured application and fall back to the internal viewer when needed.
- Terminal state is restored around foreground programs and after fatal errors.

### Unix command line

- `>` opens a modal prompt in the active panel directory.
- Fish-aware or generic completion, live prefix filtering, bounded history, and
  draft preservation.
- Successful literal `cd` commands refresh and synchronize the active panel.
- `Alt-Enter` / `Ctrl-Enter` inserts the selected name,
  `Ctrl-Shift-Enter` inserts its full path, and `Ctrl-X t` /
  `Ctrl-X Ctrl-T` inserts tagged names from either panel.
- Panel-derived arguments are quoted literally for the configured shell.
- The bottom hint bar switches to prompt actions, and `F1` opens contextual
  command-line help without discarding the draft.

### Configuration and interface

- MC-style top menus, contextual help, mouse support, and a parsed
  `mc.default.keymap` with user overrides.
- Bundled MC skins plus custom and system skin discovery.
- Typed settings for layout, panels, confirmations, appearance, display, shell,
  and command history.
- Atomic settings persistence and terminal-safe tracing.

## Installation

Requirements:

- Rust 1.88.0 or newer
- A terminal with ANSI support

Install from [crates.io](https://crates.io/crates/rust-commander):

```bash
cargo install rust-commander --locked
```

The package is named `rust-commander`; the installed executable is `rc`.

Run a local checkout with:

```bash
cargo run -p rust-commander --locked
```

Useful launch options:

```bash
rc --path /some/start/dir --tick-rate-ms 200
rc --skin modarin256
rc --skin julia256 --skin-dir /path/to/mc/skins
```

## Settings

The Options menu configures layout, panels, confirmations, appearance, display,
and setup persistence.

Settings use this precedence:

```text
built-in defaults -> persisted configuration -> environment -> CLI
```

`Save setup` writes rc-owned settings to `~/.config/rc/settings.ini` and the
MC-compatible skin choice to `~/.config/mc/ini`.

Runtime tracing is written to `~/.config/rc/rc.log`. Set `RC_LOG_FILE` to use
another path and `RUST_LOG` to change the default `warn` filter. The log is
kept away from the terminal so diagnostics cannot corrupt the alternate-screen
interface.

## Default controls

### File manager

| Keys | Action |
| --- | --- |
| `Tab` | Switch active panel |
| `Enter` | Enter a directory or archive, run an executable, or open a document |
| `Backspace` | Go to the parent directory |
| `Space` / `Insert` / `Ctrl-T` | Toggle the selected entry |
| `/` / `Alt-C` | Open Quick CD |
| `>` | Open the Unix command line |
| `F3` | View the selected file internally |
| `F4` | Edit with the configured external editor |
| `F5` / `F6` / `F7` / `F8` | Copy / move or rename / mkdir / delete |
| `Ctrl-J` / `Alt-J` | Open jobs / cancel the latest or selected job |
| `Alt-F` / `Alt-?` / `Ctrl-/` | Find files |
| `Alt-T` | Open the directory tree |
| `Alt-H` | Open the directory hotlist |
| `Alt-P` / `Ctrl-P` / `Ctrl-X !` | Open external panelize |
| `Ctrl-X i` / `Ctrl-X q` | Show Info / Quick view in the passive panel |
| `Ctrl-X y` | Copy the active selected entry's full path to the clipboard |
| `Alt-Shift-T` | Cycle Full, Brief, and Long panel layouts |
| `Shift-F6` / `Shift-F8` | Cycle sort field / reverse sort order |
| `F9` | Open the menu bar |
| `q` / `Esc` / `F10` | Quit |

Left-click an entry in either pane to select it and make that pane active.
Double-click to open the entry.

### Command line

| Keys | Action |
| --- | --- |
| `Tab` / `Shift-Tab` | Complete and cycle candidates |
| `Up` / `Down` | Browse command history |
| `Enter` | Accept a completion or run the command |
| `Alt-Enter` / `Ctrl-Enter` | Insert the active panel's selected name |
| `Ctrl-Shift-Enter` | Insert the selected entry's full path |
| `Ctrl-X t` | Insert active-panel tagged names, or its selected name |
| `Ctrl-X Ctrl-T` | Insert passive-panel tagged names, or its selected name |
| `Ctrl-X y` | Copy the active selected entry's full path to the clipboard |
| `F1` | Open command-line help |
| `Esc` | Return to the panels while preserving the draft |

### Find, tree, hotlist, and panelize

- Find results: `F4` searches again, `F5` panelizes, `F6`
  pauses or continues, and `Alt-J` cancels the search.
- Tree: arrows navigate; `F2` rescans, `F3` forgets a subtree, `F4`
  changes navigation mode, and `F5`–`F8` perform file operations.
- Hotlist: `a` adds, `e` / `F4` edits, `d` / `Delete` removes,
  and `Enter` opens.
- Panelize presets: `Tab` selects custom input, `F2` adds, `F4`
  edits, `F8` removes, and `Enter` runs.

### Viewer

| Keys | Action |
| --- | --- |
| `F7` / `Ctrl-S` | Search forward |
| `Shift-F7` | Search backward |
| `n` / `Shift-N` | Continue forward / backward |
| `g` / `Alt-L` | Go to a line or offset |
| `w` | Toggle wrapping |
| `h` | Toggle text / hex mode |
| `Esc` / `q` / `F10` | Close |

Default bindings live in
`crates/core/assets/mc.default.keymap`. Common macOS Option-symbol variants are
normalized before keymap matching.

## Platform behavior

The modal command line is available on Unix. Foreground commands receive their
own terminal process group so `Ctrl-C` interrupts the command without terminating
rc. Windows foreground programs retain normal console interrupt behavior while rc
temporarily handles control events in the parent.

Linux desktop opens prefer the desktop portal. Accepted legacy launchers are reaped
independently and are not terminated when rc exits.

Clipboard copying uses the OSC 52 terminal sequence. The terminal emulator and any
intermediate multiplexer must allow OSC 52 clipboard writes.

## Project layout

- `crates/app` (`rust-commander`): terminal lifecycle, event loop, and input
  normalization
- `crates/core` (`rust-commander-core`): state, commands, routes, jobs, and
  filesystem workflows
- `crates/platform` (`rust-commander-platform`): platform-specific terminal and
  process integration
- `crates/ui` (`rust-commander-ui`): Ratatui rendering and skin support
- `crates/shell` (`rust-commander-shell`): shell selection, completion, and
  cancelable process primitives
- [ROADMAP.md](https://github.com/hbbio/rc/blob/main/ROADMAP.md): remaining product work

## Development

With `cargo-nextest` installed, run the same baseline checks used by CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-targets --all-features --locked
```

Security, dependency-policy, coverage, and release-package checks live in
`scripts/` and `.github/workflows/`.

## License

GPL-3.0-or-later, as the project derives from
[Midnight Commander](https://github.com/MidnightCommander/mc). See
[LICENSE](LICENSE) for the complete terms.
