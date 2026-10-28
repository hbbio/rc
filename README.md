# rust commander (rc)

![Rust Commander dual-pane terminal interface](https://raw.githubusercontent.com/hbbio/rc/main/assets/rc.png)

*rust commander* is a Rust terminal file manager inspired by GNU Midnight
Commander. It keeps the familiar dual-pane workflow and MC-style default
keybindings while adding responsive background operations, a modal shell prompt,
and fast directory navigation.

## Features

### Panels and navigation

- Two independent directory panels with Full, Brief, Long, and user-defined layouts.
  Choose **Listing format → User defined** to arrange `name`, `size`, `mtime`,
  `perm`, `nlink`, `uid`, `gid`, and `inode` columns, separated by spaces or commas.
  Add `:width` (1–256) for a fixed width; an unsized `name` fills available space.
  For example, `perm size:9 name`. Formats are saved per panel with Save setup.
- `Ctrl-U` swaps the panels with their selection, tags, listing settings, and
  navigation history. Focus follows the active listing.
- Browse ZIP, TAR, gzip TAR, and bzip2 TAR archives as read-only folders, with
  normal navigation, sorting, filtering, and tagging.
- Native filesystem notifications automatically refresh both panels and the open
  directory tree, including changes made by other programs. Updates are coalesced
  and scanned in the background while preserving selection and tags.
- Per-panel sorting, reverse order, glob or regular-expression filters, hidden-file
  control, and persisted configuration.
- **Left/Right → Encoding** (`Alt-E`) selects the filename encoding on Unix:
  UTF-8, Windows-1252/1251, ISO-8859-2, KOI8-R, Shift_JIS, EUC-JP, GBK, Big5,
  EUC-KR, or CP437. Listings, sorting, and panel filters use decoded names; filesystem
  operations retain original path bytes. The setting also applies to archive and
  panelized names, follows panel swaps, and persists with Save setup. It does not
  change file contents or the terminal encoding.
  ZIP names that are not UTF-8 default to CP437 unless another codec is selected.
- UTF-8 listings and editable path fields preserve Unix filenames with reversible
  `\xHH` byte escapes; a literal backslash appears as `\\`. Copy/move, rename,
  mkdir, link targets, hotlist paths, View file, and Find's start directory accept
  this spelling. Batch rename masks operate on original bytes. Regex filename
  search can match raw bytes with patterns such as `(?-u:\xFF)`; editor `{path}`
  placeholders also retain native bytes. Saved path settings use separate encoded
  keys when needed, while existing plain-path entries retain their meaning.
- Panel-to-command insertion and filesystem completion retain native bytes through
  quoted `RC_NATIVE_HEX_…` references when a filename contains invalid UTF-8 or
  control characters. Keep these references intact when editing a command; rc
  supplies their values to the selected POSIX or Fish shell. They survive session
  history and work with literal `cd`, Quick cd, and completion inside such
  directories. Clipboard paths use the same reversible spelling as path fields.
- Tagging for batch operations, recursive selection totals, responsive Brief-mode
  columns, and mouse selection or activation.
- `Ctrl-Space` calculates recursive apparent sizes for tagged directories, or all
  visible directories when none are tagged. Results update the size column and
  size sorting without moving the selection. The cancelable scan supports archives,
  does not follow local symlinks, and reports unreadable entries as partial results.
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
- On Unix, **File → Chmod** (`Ctrl-X c`), **Chown** (`Ctrl-X o`), and
  **Advanced chown** change selected or tagged entries in a cancelable background
  job. Chmod accepts octal modes or explicit `u/g/o/a` clauses with `+`, `-`, `=`
  and `rwxXst`, such as `u+rw,g-w,o=r`. Chown accepts account names or numeric IDs;
  blank fields keep existing values. Advanced chown combines `owner:group` and an
  optional mode. Operations affect only the selected entries, without recursion.
  Chmod rejects symlinks; chown changes the link itself. Completed changes remain
  if a later entry fails or the job is canceled.
- **File → Link / Symlink / Relative symlink** (`Ctrl-X l/s/v`) creates links
  for selected or tagged entries on Unix. Destinations are literal paths relative
  to the passive panel; `.` uses that directory and the original names. Tagged
  batches require an existing directory. Existing destinations are never replaced.
  Hard links require regular files on the same filesystem; relative symlinks are
  calculated from each destination directory, including directory aliases.
  **Edit symlink** (`Ctrl-X Ctrl-S`) changes each selected link to the entered
  literal target, including dangling links, without following it. Editing uses
  atomic exchange on Linux/macOS; filesystems lacking it report an error. If an
  entry changes concurrently, rc rolls back and reports a preserved recovery path.
  Completed batch entries remain if a later entry fails or the job is canceled.
- Background execution with progress, cancellation, overwrite policies, and a jobs
  screen.
- Alt + backtick opens the screen list from panels, viewers, searches, trees, help, and
  jobs. Switch to File manager to keep the current screen open, then restore it
  later with its cursor and search state intact. Background searches and captured
  output continue while their screens are suspended. Up to 16 screens can be
  suspended; restore a screen and close it normally to release it.
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
first time without a saved index requires a cancellable decompression scan to build its directory and
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

On Unix, metadata indexes also survive application restarts in
`$XDG_CACHE_HOME/rust-commander/archive-indexes`, or
`~/.cache/rust-commander/archive-indexes` when that variable is unset. The directory
and files are private to the current user. Atomic, versioned cache files include
source identities and checksums; missing, corrupt, or outdated entries are rebuilt.
The disk cache uses a soft budget of 64 indexes and 256 MiB, with a 64 MiB limit per index.
Larger indexes remain available through the in-memory cache. Nested metadata keys
include the outer source and every container boundary. Compressed seek checkpoints
are rebuilt on the first member read after restarting; metadata-only browsing
does not require that scan. An unwritable cache does not prevent archive access.

ZIP Stored/Deflate entries and TAR regular files/directories are supported,
including GNU long names and PAX paths, sizes, and modification times. Repeated TAR
names use the last record. Encrypted ZIP contents, archive symlinks, hard links,
sparse files, and special files are not supported for
viewing/extraction. Unsafe member paths are omitted.

Enter opens supported archives inside other archives, up to eight container levels.
`..` returns to the enclosing archive and selects the container just left. Viewing,
file history, size totals, and copying work at every level. Inner containers are
materialized into private temporary files, reused while cached, and removed on
eviction or exit. Each inner container is limited to 512 MiB, with a shared 1 GiB
temporary-storage limit. Cancellation and errors remove incomplete files; changes
to any enclosing container invalidate its descendants. Copies cannot replace the
outer source archive.

On Unix, member paths retain their original bytes, including non-UTF-8 names;
display labels may contain replacement characters. Unsupported names produce an
explicit error during indexing or extraction. Directory copies restore an
overwritten destination if its filesystem cannot represent a member name.

Archive panes and jobs use a common read-only backend interface in
[`crates/core/src/archive`](crates/core/src/archive.rs). Container backends supply
immutable metadata and member readers; TAR compression readers supply indexed
seeking independently. Adding another format does not require format-specific
navigation, viewer, or copy flows. This archive layer does not yet provide the
unified local/remote VFS described in the roadmap.

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

- Local **Open**, **View**, and **Edit** actions honor ordered `mc.ext.ini` rules
  on Enter, F3, and F4. Files require `[mc.ext.ini]` with `Version=4.0`.
  Matching supports `Directory`, `Regex`, literal `Shell` names/suffixes, and
  `Type` patterns against cancelable background `file -L -b` output. Case flags
  apply to each matcher. `Include=name` supplies actions from `[Include/name]`;
  missing actions fall back to `[Default]`, while empty actions suppress that
  fallback. Invalid configurations report errors; correcting them and closing the
  configuration editor reloads the rules. Unmatched actions retain normal behavior.
  Directory navigation changes only when an explicit Directory rule matches.
- Actions use `/bin/sh` and MC macros: `%f`, `%p`, `%n`, `%x`, `%d`, `%t`, `%s`,
  `%u`, their uppercase passive-panel forms, `%%`, `%{prompt}`, and
  `%var{NAME:default}`. Values are quoted by default and preserve native filename
  bytes. `%1` explicitly enables quoting; `%0` inserts raw shell source.
  `%u` clears the expanded tags only after the action is scheduled. Extension
  helpers receive `MC_EXT_FILENAME`, `MC_EXT_BASENAME`, `MC_EXT_CURRENTDIR`,
  `MC_EXT_SELECTED`, and `MC_EXT_ONLYTAGGED`.
- `%view{ascii,hex,nroff,unform}` routes action output to the internal viewer;
  a bare `%view` loads the selected file. Hex retains original bytes; nroff makes
  overstrikes and ANSI formatting readable as plain text. `%cd` changes to a
  literal expanded path; native TAR/ZIP paths ending in `/utar://` or `/uzip://`
  open an archive. Prompt cancellation launches nothing and preserves tags.
  Templates and expanded arguments are bounded to 64 KiB. Here-document templates
  report an unsupported-template error; pass macros as arguments to a separate
  script instead. Regex patterns use the same Rust regex syntax as panel filters,
  with doubled backslashes for INI escaping.
- **Command → Edit extension file / Edit menu file / Edit highlighting group file**
  opens `mc.ext.ini`, `menu`, or `filehighlight.ini` in the configured external
  editor. Files in `~/.config/mc` take precedence over MC files installed under
  `/opt/homebrew`, `/usr/local`, and `/usr`. Editing an installed or bundled default
  first creates a private user copy without replacing an existing file.
  On Unix, a regular `.mc.menu` in the active directory takes precedence when it is
  owned by the current user or root and is not writable by group or others;
  local symlinks are ignored. The same discovery is shared by configuration readers.
- F2 / **Command → User menu** reads the effective menu on each open: trusted
  `.mc.menu`, `~/.config/mc/menu` (with `mc.menu` as a compatibility fallback),
  installed `mc.menu`, then bundled entries. Shortcut letters or Enter launch the
  selected entry through the shared macro engine, including prompts and `%view`.
  Addition (`+`) and default (`=`) conditions support active/passive filename,
  directory, file type and tag tests, executable checks, negation, and left-to-right
  `&`/`|` logic. `shell_patterns=0/1` selects regex/glob matching; `?` after a
  condition prefix shows its result before the menu. User menus require a local
  active directory; syntax conditions (`y`) are false outside an editor.
- `filehighlight.ini` controls panel colors through groups in the active skin.
  Rules match in file order and support `type`, `regexp`, and semicolon-separated
  `extensions`, with optional `extensions_case=true`. Type takes precedence over
  regexp, which takes precedence over extensions. Regex backslashes must be doubled
  for INI escaping. Link, directory, executable, device, special-file, and hardlink
  types use cached listing metadata. Tagged and selected colors retain precedence.
  Rules load at startup and reload when the external editor closes; invalid edits
  report an error and retain the last valid rules.
- Internal text and hex viewer with syntax highlighting, search in both directions,
  goto, wrapping, and bounded previews for large files.
- File → View file opens an entered local path, including relative paths and `~/`.
- File → Filtered view (`Alt-!`) runs a shell command in the active local
  directory and streams stdout and stderr into the searchable text/hex viewer.
  `Esc` returns to the panels while the command continues; `Alt-J` cancels it.
  Open Jobs, select the command, and press `Enter` to return to its output.
  The title records completion or failure. Capture stops at 8 MiB per command;
  up to eight outputs are retained, evicting the oldest completed, closed output.
  Captured commands use the configured shell and have no interactive standard input.
- Command → Compare files (`Ctrl-X Ctrl-D`) compares the selected local file in
  each panel, preferring `difft` (difftastic) and falling back to unified `diff`.
  Compare directories (`Ctrl-X D`) reports recursive differences between the two
  local panel directories with `diff`. Both use the output viewer; differences are
  a successful comparison, while read/tool errors are reported as failed jobs.
- `Alt-Shift-E` opens the session's viewed/edited-file history. Its 100 most recent
  entries reopen with the original action, including files inside archives.
- External editing through a configured command, `$EDITOR`, `$VISUAL`, or
  detected editors (`hx`, `nvim`, `vim`, `vi`, `emacs`).
- Executables run in the terminal; documents open with the operating system's
  configured application and fall back to the internal viewer when needed.
- Terminal state is restored around foreground programs and after fatal errors.

### Unix command line

- `>` opens a modal prompt in the active panel directory.
- Commands run in a persistent PTY shell: variables, functions, and directory
  changes survive between commands. Bash, Zsh, Fish, sh, and dash are supported;
  other shells and custom command templates retain one-shot execution.
- `Ctrl-O` or Command → Switch panels on/off shows the shell and returns to the
  panels. The shell also appears in the Screens selector. Programs keep running
  while the panels are visible; terminal colors, full-screen applications,
  resizing, mouse input, and bracketed paste are supported.
- Panel and shell directories synchronize when the shell is ready. A running
  command cannot overwrite a directory subsequently chosen in the panels.
  Unfinished terminal input blocks injected commands and directory changes;
  use `Ctrl-C` to clear it. This can also be needed after leaving a program with
  a single key because unread typeahead cannot be distinguished reliably.
  Fish sessions reserve `Ctrl-Alt-Shift-F12` for this input recovery.
- `Shift-PgUp` / `Shift-PgDn` scroll shell output. `exit` or `Ctrl-D` at an empty
  shell prompt closes the shell; `Ctrl-O` starts another when needed.
- Fish-aware or generic completion, live prefix filtering, bounded history, and
  draft preservation.
- Command → Command history (`Alt-Shift-H`) lists recent commands and recalls a
  selection into the prompt for editing. Cancel preserves the current draft.
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
- Options → Learn keys captures a chord and assigns a command in a chosen context.
  Learned bindings apply immediately over the bundled/custom keymap. Select a
  learned binding to edit it, or clear its command to restore the underlying
  binding. Save setup persists them in `settings.ini`.
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
| `Alt-Shift-T` | Cycle Full, Brief, Long, and user-defined panel layouts |
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
- `crates/shell` (`rust-commander-shell`): shell selection, completion, cancelable
  process primitives, and the Unix persistent PTY transport and prompt protocol
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
