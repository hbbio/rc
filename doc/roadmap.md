You are rebuilding a mature, stateful TUI file manager with its own viewer, editor, diff viewer, VFS layers, and a large surface of “small” behaviors. A workable plan needs (1) a feature inventory grounded in upstream docs/config, (2) a compatibility strategy for keymaps and config files, and (3) an architecture that keeps the UI responsive while long operations run. I’m assuming the target is a daily-driver replacement on Linux first, then other Unix-like systems, and Windows as a follow-on.

## 1. Scope, goals, and hard constraints

### Primary goals

* **Feature parity with current GNU Midnight Commander** at the user-facing level: two-panel file manager, subshell command line, internal viewer/editor/diff viewer, VFS, search, user menu, extension-based “Open/View/Edit”. ([Midnight Commander][1])
* **Default keybindings match MC**, by shipping and parsing the same keymap format and default mappings. ([GitHub][2])
* **Modernize** the internals: strong separation of concerns, structured concurrency, testability, crash safety, and better responsiveness during I/O.

### Compatibility constraints

* Parse and honor:

  * `mc.default.keymap` format and section model (filemanager/panel/dialog/menu/input/editor/viewer/diffviewer…). ([GitHub][2])
  * `mc.ext.ini` INI format, including command macros and “Include=” composition. ([GitHub][3])
  * `mc.menu` user menu format and macros (`%f`, `%t`, `%view{...}`, `%{prompt}` …). ([GitHub][4])
  * Syntax mapping for the editor (`Syntax` file locations and matching rules). ([GitHub][5])

### “Modernize” definition for this project

* Non-blocking UI during directory reads, size calculation, remote listing, search, copy/move.
* Unicode-first rendering and input; stable behavior on modern terminals; truecolor themes.
* Safer defaults for destructive actions (trash where supported, clearer previews, better progress and cancelation).
* Extensibility that does not require patching core: plugin hooks for VFS backends and previewers.

## 2. Feature inventory and target screens

This list is derived from MC’s own description, manuals, and default config files. ([Midnight Commander][1])

### 2.1 Main file manager screen

Visual layout goals (match MC):

* Two directory panels (left/right) with selectable listing formats.
* Menu bar and bottom function key bar (F1..F10 semantics).
* Status lines with selection totals, free space, etc.
* Embedded command line and prompt area tied to a shell/subshell. ([Midnight Commander][6])

Core interactions:

* Navigation: cursor movement, paging, home/end, enter to open, panel swap, panel focus.
* Marking/selection: insert toggles, select/unselect/invert.
* Core ops: copy/move/mkdir/delete, rename, chmod/chown, symlinks/links. ([GitHub][2])
* Find file, quick cd, directory hotlist, directory tree, external panelize. ([Midnight Commander][6])
* Screen list / screen selector (MC supports multiple “screens” like viewer/editor/diff/help). ([GitHub][2])
* Background jobs list for long ops. ([GitHub][2])

### 2.2 Dialogs and secondary screens

You should plan these as composable “routes” with their own input context and keymap section.

Must-have dialogs/screens:

* Copy/move progress dialog with speed, ETA, overwrite policy, skip, retry, background, cancel.
* Delete confirm dialog (single, tagged, recursive).
* Mkdir, rename/move prompt, chmod/chown dialog.
* Link/symlink dialog, relative symlink option.
* Find file dialog and results panel.
* Directory hotlist manager.
* Directory tree navigation. ([GitHub][2])
* Compare directories and compare files UI entry points. ([GitHub][2])
* Options/config screens: layout, appearance, panel options, confirmation settings, VFS settings, learn keys. ([GitHub][2])
* User menu (F2) screen, driven by `mc.menu`. ([GitHub][4])

### 2.3 Internal viewer

Plan for:

* Text viewing with wrap toggle.
* Search forward/backward, continue search.
* Goto offset/line.
* Hex mode and “hex edit mode” behavior mapped to the viewer:hex keymap section. ([GitHub][2])

### 2.4 Internal editor (mcedit-compatible experience)

From the editor man page and keymap:

* Multiple files / file switching.
* Undo/redo, block selection, column selection.
* Search/replace, goto line, bracket match, paragraph format, bookmarks, syntax highlighting on/off. ([GitHub][2])
* Editor user menu (F11) driven by `mcedit.menu`. ([GitHub][7])
* Syntax mapping rules and system/user locations for syntax files. ([GitHub][5])

### 2.5 Diff viewer

From the keymap and manuals:

* Side-by-side diff with split controls (full/equal/more/less).
* Hunk navigation, search, save, edit/merge actions. ([GitHub][2])

### 2.6 VFS and “extension file” behaviors

MC’s `mc.ext.ini` is effectively a rule engine for what happens on Enter/F3/F4 and for virtual “open into archive” commands, including schemes like `utar://`, `u7z://`, `urar://`, `patchfs://`, `rpm://`, etc. ([GitHub][3])
Your Rust version should treat this as a first-class compatibility target, not a legacy edge-case.

Also needed:

* User menu macros and quoting toggles (`%0f`, `%1f`), prompt macro `%{…}`, and piping to viewer via `%view{ascii,hex,nroff,unform}`. ([GitHub][4])

## 3. Compatibility strategy for keybindings

### 3.1 Ship MC keymaps and parse them

* Bundle `mc.default.keymap` content and parser.
* Implement a keymap engine with:

  * Context sections: `[filemanager]`, `[panel]`, `[dialog]`, `[menu]`, `[input]`, `[editor]`, `[viewer]`, `[viewer:hex]`, `[diffviewer]`, etc. ([GitHub][2])
  * Multiple bindings per action (`Search = ctrl-s; alt-s` style).
  * Named keys with modifiers (`alt-question`, `ctrl-backslash`, function keys, keypad keys).
  * “xmap” sub-context (`[filemanager:xmap]`) for command menu accelerators. ([GitHub][2])

### 3.2 Key event normalization layer

Terminals differ in how they encode:

* Alt combinations (ESC prefix).
* Function keys beyond F12.
* Keypad keys.
* Shifted arrows/home/end in some terminals.

Design:

* Raw input (from terminal backend) goes to a normalization layer producing a canonical `KeyChord`:

  * `KeyCode`: char, named key, function key.
  * `Modifiers`: ctrl/alt/shift.
  * Optional “sequence id” for special escape sequences not expressible otherwise.
* Keymap matching runs on canonical chords, and also supports raw escape sequence matching for “learn keys” and edge cases.

### 3.3 “Learn keys” and diagnostics

A learn-keys screen is worth doing early because it de-risks portability and user support. MC has an explicit LearnKeys command in the options list. ([GitHub][2])
Plan: a screen that displays what the terminal sent (raw bytes and decoded guess), then writes overrides into a user keymap.

## 4. Rust stack and maintained libraries (2025/2026)

### 4.1 Terminal UI and input

* **ratatui** for layout/widgets/rendering. ([GitHub][8])
* **crossterm** for cross-platform terminal control and input events. ([GitHub][9])
* A thin app framework on top (your own), instead of adopting a heavy external framework, because MC has many custom behaviors and input contexts.

### 4.2 Concurrency and background work

* **tokio** for async runtime, timers, async I/O, cancellation plumbing. ([GitHub][10])
  Use it for:

  * VFS network I/O
  * Background directory reads and stat batches
  * File copy pipelines with progress
  * Search jobs and size calculation jobs

### 4.3 PTY and subshell

* **portable-pty** as the cross-platform abstraction for spawning and talking to PTYs. ([Crates][11])
  Plan to isolate this behind a `ShellBackend` trait so you can swap strategies per OS.

### 4.4 Filesystem traversal, ignore rules, watching

* **walkdir** for recursive traversal (used for copy/search/tree ops). ([Docs.rs][12])
* **ignore** for `.gitignore`-style ignore evaluation and fast walking with filters. ([Crates][13])
* **notify** for filesystem change events to refresh panels and caches. ([GitHub][14])

### 4.5 Paths and Unicode handling

* **camino** (`Utf8PathBuf`) for UI-facing paths where you need guaranteed UTF-8 display. Keep `std::path::PathBuf` for OS-facing calls where non-UTF-8 paths matter. ([Docs.rs][15])

### 4.6 Remote access libraries (VFS backends)

You have two viable approaches for SSH-like backends:

Option A (native library bindings):

* **ssh2** (libssh2 bindings) for SSH/SFTP. ([Docs.rs][16])
  Pros: mature, feature-complete for client use, common in distros.
  Cons: C dependency, OpenSSL considerations.

Option B (pure Rust SSH stack):

* **russh** for SSH client plumbing. ([Docs.rs][17])
  Pros: Rust-only codebase.
  Cons: you will spend more project time on protocol surface and edge cases.

FTP/FTPS:

* **suppaftp** as the primary FTP/FTPS client library. ([Crates][18])

“Fish” (shell over SSH) backend:

* If you want MC parity, implement fish as “run remote shell commands” on top of either `ssh2` exec channels or OpenSSH wrapper. MC supports fish-style VFS in its docs. ([Midnight Commander][6])
* **openssh** crate is usable for “wrap the system ssh binary” on Unix. ([Docs.rs][19])

Recommendation:

* Start with `ssh2` for SFTP (fastest path to a usable daily tool), keep an internal trait boundary so a future pure-Rust backend can land without rewriting the UI and file ops layer.

### 4.7 Archives

For native archive browsing (treated like a directory):

* **tar** for TAR read/write (compression handled separately). ([Crates][20])
* **zip** for ZIP read/write. ([Crates][21])

MC also relies on many external helpers and schemes defined through `mc.ext.ini` and helper scripts for a broad set of archive/package formats (7z, rar, rpm, deb, iso…). ([GitHub][3])
Plan: native TAR/ZIP early, then keep “extfs helper” compatibility for the long tail.

### 4.8 Viewer/editor/diff internals

* **memmap2** for memory-mapped I/O in viewer and large-file operations. ([Crates][22])
* **ropey** as the editor’s text buffer (rope structure for large files and edits). ([Crates][23])
* **syntect** for syntax highlighting (TextMate/Sublime style definitions). ([Crates][24])
* **similar** for diff computation and presentation. ([Docs.rs][25])

### 4.9 Config, CLI, logging

* **serde** as the data model serializer/deserializer foundation. ([serde.rs][26])
* **clap** for CLI argument parsing. ([Docs.rs][27])
* **tracing** for structured diagnostics and internal event spans. ([Crates][28])

## 5. Rust 2025/2026 coding standards for this codebase

### 5.1 Language edition and baseline

* Target **Rust 2024 edition** (the edition guide shows Rust 2024 and its release version). ([Rust Documentation][29])
  Set an explicit MSRV policy (example: MSRV = edition release toolchain, then bump on a fixed cadence).

### 5.2 API and module design rules

* Follow the Rust API Guidelines for public crates and internal trait boundaries. ([Rust Language][30])
* Keep the workspace split so most crates have small public surfaces:

  * `core` (domain types and commands)
  * `ui` (ratatui widgets and render pipeline)
  * `vfs` (backends and path parsing)
  * `ops` (copy/move/delete engines)
  * `viewer`, `editor`, `diff`
  * `config` (mc.keymap, mc.ext.ini, menus, theme formats)
  * `shell` (subshell/pty)

### 5.3 Formatting and linting

* Enforce `rustfmt` in CI. ([Rust Language][31])
* Enforce `clippy` with a project lint baseline (deny warnings for CI builds).
* Prefer `thiserror` for library error enums and `anyhow` for app-level “bubble up” errors (no citation here; this is ecosystem convention).

### 5.4 Unsafe code policy

* Default stance: `#![forbid(unsafe_code)]` in most crates.
* Allow tightly scoped unsafe only where required by OS interfaces:

  * memory mapping (`memmap2` mapping calls are unsafe at the boundary). ([Docs.rs][32])
  * PTY / terminal raw mode where OS calls require it (behind a single module boundary).

## 6. High-level architecture

### 6.1 Event-driven core with message passing

Design the app as:

* **Single UI thread**: draws frames, handles keyboard/mouse, updates state.
* **Background worker tasks**: directory reads, stat batches, copy jobs, search jobs, remote I/O, archive scans.
* Communication:

  * UI sends `AppCommand` to a command dispatcher.
  * Dispatcher updates state synchronously for quick actions.
  * Long actions spawn tasks that emit `AppEvent` messages back to the UI (progress, completion, errors, new directory entries).

Tokio fits here: a multi-thread runtime for background tasks, with UI running on the main thread while polling an event channel. ([GitHub][10])

### 6.2 State model

Core types:

* `AppState`

  * `panels: [PanelState; 2]`
  * `active_panel: Left|Right`
  * `command_line: InputState`
  * `screens: Vec<Screen>` where `Screen` is one of FileManager, Viewer, Editor, Diff, Help, Tree, FindResults, Jobs, Dialog
  * `key_context: KeyContext` derived from top screen and focused widget
  * `config: ConfigState` (keymap, skin, ext rules, menus, options)
  * `jobs: JobManagerState`
* `PanelState`

  * `cwd: VfsPath`
  * `listing: Vec<FileEntry>`
  * `sort_mode`, `filter`, `quick_search_state`
  * `selection: SelectionState` (tagged set, current index)
  * `cached_stats` and `dir_size_cache` for UI display

### 6.3 UI composition

Use ratatui for layout primitives (chunks, blocks, tables) and custom widgets for:

* Panel table with column layout rules (name, size, mtime, perms, owner, group).
* Mini status, selection totals, free space line.
* Function key bar and menu bar.
* Dialog system with consistent focus and shortcuts.

Ratatui is the rendering layer, but your widgets should be “dumb” and render-only, with behavior defined by commands and state transitions. ([GitHub][8])

## 7. VFS design that can match MC behaviors

### 7.1 VFS path model

Implement `VfsPath` as:

* `Vec<VfsSegment>`
* A `VfsSegment` is `{ scheme, authority, path }`, allowing nested mounts like:

  * local path
  * inside archive
  * inside remote path
* Parse both:

  * MC-style `/#ftp:host/path` patterns (for compatibility)
  * URI style `ftp://host/path` as a modern convenience

### 7.2 Backend trait

Define:

* `trait VfsBackend { async fn list_dir(...); async fn stat(...); async fn open_read(...); async fn open_write(...); async fn rename(...); async fn unlink(...); async fn mkdir(...); async fn rmdir(...); async fn readlink(...); async fn symlink(...); ... }`

Backends:

* `LocalFsBackend`
* `SftpBackend` (ssh2 or russh)
* `FtpBackend` (suppaftp)
* `ArchiveBackend`:

  * tar
  * zip
  * extfs helper backed
* `ExtfsBackend` for `utar://` style schemes defined by extension rules (see next section)

### 7.3 Extfs and `mc.ext.ini` compatibility

MC uses `mc.ext.ini` to map file patterns to `Open=...`, `View=...`, `Edit=...`, often changing directory into a virtual scheme like `utar://` or `patchfs://`. ([GitHub][3])

Plan:

* Implement an **extension rule engine**:

  * Parse INI sections, evaluate rules in order, support `Include=` sections. ([GitHub][3])
  * Match by:

    * directory regex (`Directory=...`)
    * extension (`Shell=.tar`)
    * regex (`Regex=...`)
    * file-type regex (“Type” rule based on `file` output) if you want parity. ([GitHub][3])
* Execute rule actions:

  * For `Open=`, `View=`, `Edit=`, interpret MC macros (`%f`, `%p`, `%view{...}`, `%cd`, etc) and run commands.
  * For schemes like `utar://`, you can route them either:

    * Native: treat `utar://` as tar backend
    * External: call a helper script, match MC’s ext helper style

This keeps your Rust core small while preserving MC’s “open archive like a folder” behaviors. ([GitHub][3])

## 8. User menu and macro engine

MC’s user menu is explicitly configurable and macro-driven. ([Midnight Commander][6])
Plan:

* Parse `mc.menu` file:

  * Condition lines (file patterns, flags)
  * Command blocks
  * Title lines
* Implement macro expansion:

  * `%f`, `%p`, `%d`, `%t`, `%s`, `%D` (other panel directory), uppercase variants refer to other panel. ([GitHub][4])
  * `%{prompt}` interactive prompt.
  * `%view{ascii,hex,nroff,unform}` pipes command output into internal viewer. ([GitHub][4])
  * `%0`/`%1` quoting toggles. ([GitHub][4])
* Execution:

  * Run via subshell backend when available, else `sh -c` with explicit cwd.
  * Stream stdout/stderr to an “Output viewer” screen; store exit status.

## 9. Subshell and command line plan

MC runs commands in a subshell and supports toggling the shell view. ([Midnight Commander][1])
In a Rust TUI, this is one of the highest-risk parts. Plan it in stages:

### Stage 1: Non-interactive command runner (early milestone)

* Bottom input runs `shell -c <cmd>` in panel cwd.
* Capture output to viewer; allow background execution.
* Track jobs and show results.

This already supports many workflows and makes other features testable.

### Stage 2: Persistent subshell with PTY

* Use portable-pty to spawn the user’s preferred shell and keep it running. ([Crates][11])
* When user hits Enter on command line:

  * Write command into PTY
  * Read until prompt marker detected (needs a prompt protocol)
* Make cwd sync:

  * A prompt protocol similar to MC: inject a shell hook that prints cwd and a unique marker each prompt.
  * Update active panel cwd when shell `cd` is detected, and optionally push cwd to both panels depending on settings.

### Stage 3: Ctrl-O “panels off”

* Temporarily detach the UI and attach the controlling terminal to the shell session.
* On return, restore alternate screen and redraw.

This stage needs careful OS-specific handling; isolate it per platform.

## 10. File operations engine

### 10.1 Copy/move/delete semantics

Implement as job pipelines that:

* Enumerate sources (supports tagged files and directories)
* For each entry:

  * Decide overwrite policy (ask, skip, overwrite, append, resume)
  * Stream copy with progress
  * Preserve metadata (mtime, perms, owner where permitted)
* Moves:

  * If same filesystem and local: attempt rename, fallback to copy+delete.
* Deletes:

  * Support “trash” mode (platform dependent) and “unlink” mode; default policy configurable.

### 10.2 Performance and responsiveness targets

* Listing and stat should be incremental: show initial entries fast, then fill in metadata.
* Size calculation (DirSize key binding exists) must run as a background job. ([GitHub][2])
* “Reread” refresh should cancel in-flight listing tasks for the same panel and restart. ([GitHub][2])

### 10.3 Safety and auditability

* Every destructive op records a structured event (path, op, result, timing) through tracing. ([Crates][28])
* Optional “dry run” mode for copy/move jobs to show what would happen.

## 11. Viewer, editor, diff: detailed build plan

### 11.1 Viewer

* Backing storage:

  * Memory map for local files where stable; fallback to buffered read streams for remote and mutating files. ([Docs.rs][32])
* Rendering:

  * Text mode: line cache, wrap mode toggle.
  * Hex mode: byte grid, address column, ASCII sidebar.
* Search:

  * Plain and regex search (optional).
  * Continue search and direction controls mapped to viewer keymap. ([GitHub][2])

### 11.2 Editor

* Text buffer: ropey. ([Crates][23])
* Syntax highlighting: syntect (load TextMate grammars). ([Crates][24])
  Also plan a compatibility layer:

  * Parse MC’s Syntax mapping file, but translate to syntect grammar selection where possible. ([GitHub][5])
* Editing features to schedule in order:

  1. Basic navigation, insert/overwrite, save, save-as.
  2. Undo/redo.
  3. Search/replace.
  4. Block selection and operations (copy/move/remove), then column selection (MC has dedicated column mark movement keys). ([GitHub][2])
  5. Bookmarks and bookmark navigation keys.
  6. External command piping and editor user menu (F11). ([GitHub][7])

### 11.3 Diff viewer

* Diff engine: similar crate for text diffs and hunk structure. ([Docs.rs][25])
* UI:

  * Two panes with synchronized scrolling.
  * Hunk list and navigation.
  * Split controls and tab width controls per diffviewer keymap. ([GitHub][2])
* Merge/edit operations:

  * Start with “open left/right in editor”.
  * Later: inline apply hunk to target file.

## 12. Configuration model and file formats

### 12.1 File locations

Adopt XDG on Unix and provide import from MC locations:

* Keymap, menus, extension file, syntax files.
  MC’s syntax mapping file describes system and user locations that you can mirror. ([GitHub][5])

### 12.2 Config layering

* System defaults (shipped with the binary or installed data dir)
* User overrides
* Session overrides (not persisted unless user saves)

### 12.3 Formats

* Keep reading MC formats for compatibility:

  * keymap (`*.keymap`) ([GitHub][2])
  * `mc.ext.ini` ([GitHub][3])
  * `mc.menu` and `mcedit.menu` ([GitHub][4])
  * Syntax mapping file `Syntax` ([GitHub][5])
* For new settings, store a Rust-owned config file (TOML is a common choice with serde), while still allowing export to MC-ish formats.

## 13. Licensing plan

MC is GPL-licensed; if you copy MC’s shipped config/data files (keymaps, menu templates, ext rules), you inherit their licensing requirements. A clean-room Rust codebase can choose its own license, but bundling MC data files pushes you toward GPL compatibility.

Two workable approaches:

1. **GPL-3.0-or-later for the whole project**: simplest if you want to ship MC’s default configs verbatim.
2. **Dual distribution**:

   * Core Rust binary under permissive license
   * A separate “compat pack” containing MC-derived config files under GPL, installed optionally

Pick early; it impacts packaging and contributor expectations.

## 14. Test strategy and CI

### 14.1 Non-UI core tests

* Unit tests for:

  * keymap parser and matching resolution
  * `mc.ext.ini` rule evaluation and macro substitution
  * `mc.menu` parsing and macro substitution
  * VFS path parsing and normalization
* Property tests for selection logic, path join/split, quoting rules.

### 14.2 Golden tests for TUI

* A headless renderer mode that renders to an in-memory buffer (ratatui supports alternate backends) and snapshot-test key screens.
* Record/replay input sequences for regressions: “press keys, assert state”.

### 14.3 Integration tests

* Spawn a temp directory tree and run scripted ops:

  * copy/move/delete
  * find file
  * archive browsing (tar/zip)
* VFS tests with a local container:

  * Use a dockerized SFTP/FTP server in CI when possible.

### 14.4 Static checks

* fmt, clippy, deny warnings
* dependency checks (security/license tooling)

## 15. Roadmap with milestones (ordered to reach a usable tool early)

### Milestone 0: Project skeleton

* Workspace layout, CI, ratatui+crossterm “hello frame”, event loop, logging (tracing), CLI (clap). ([GitHub][8])

### Milestone 1: Panels and navigation

* Local filesystem listing, sorting, basic selection/tagging.
* Keymap parser loaded from bundled `mc.default.keymap` and context switching.
* Core dialogs scaffold (Ok/Cancel focus, listbox, input widgets). ([GitHub][2])

### Milestone 2: Basic file operations

* Copy/move/mkdir/delete with progress and cancel.
* Job manager and background jobs screen.
* Refresh/reread behavior.

### Milestone 3: Viewer

* Text viewer, search, wrap toggle, goto.
* Hex mode read-only first, then hex edit mode.

### Milestone 4: Find file, tree, hotlist, panelize

* Find UI and results panel.
* Directory tree screen.
* Hotlist CRUD.
* External panelize.

### Milestone 5: Extension rules and user menu

* `mc.ext.ini` engine: Open/View/Edit behaviors and scheme dispatch. ([GitHub][3])
* `mc.menu` parser and user menu screen, macro engine, `%view{...}` piping to internal viewer. ([GitHub][4])

### Milestone 6: Editor

* Rope-based buffer, syntax highlight via syntect, core edit keys from keymap.
* Search/replace, undo/redo.
* Editor user menu (`mcedit.menu`). ([GitHub][7])

### Milestone 7: Diff viewer

* Similar-based diff model, hunk navigation, split controls, open-in-editor.

### Milestone 8: Remote VFS

* FTP (suppaftp) and SFTP (ssh2 or russh) directory listing and file transfer. ([Crates][18])
* Credential handling, host key checks, bookmarks.

### Milestone 9: Subshell

* Stage 1 command runner then PTY-backed persistent subshell.
* Ctrl-O panels off behavior.

### Milestone 10: Cross-platform hardening

* Windows terminal support under crossterm, path and permission model differences.
* Optional: Windows-specific shell integration.

## 16. Key modernization extras (kept off by default or behind new bindings)

These are “modern” features that can coexist with MC defaults:

* Fuzzy file search in panel (typeahead that matches substrings and fuzzy scoring).
* Persistent tabs per panel (multiple directories per side).
* Inline git status decorations (optional) using either `git2` bindings or `gix` family crates if you decide to adopt them later. ([Docs.rs][33])
* Preview plugins (syntax-highlighted preview, image metadata, archive content summary) attached to QuickView.

## 17. Known hard parts and risk controls

High risk areas:

* Subshell + Ctrl-O + terminal mode transitions across terminals.
* Full MC VFS compatibility, especially nested VFS and extfs scheme semantics.
* Editor parity (MC’s editor has years of accumulated behaviors).

Risk controls:

* Reach a usable file manager early (Milestone 2) and then add tools.
* Keep strict boundaries: UI never blocks on I/O; everything expensive is a job.
* Preserve compatibility by reading the same keymap/menu/ext formats first, then add new formats later.

If you want, I can turn this plan into a repository-level blueprint: workspace crate graph, public trait surfaces, and a per-milestone backlog broken into tickets with acceptance criteria keyed to MC behaviors and the exact keymap contexts.

[1]: https://midnight-commander.org/ "https://midnight-commander.org/"
[2]: https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.default.keymap "https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.default.keymap"
[3]: https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.ext.ini.in "https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.ext.ini.in"
[4]: https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.menu.in "https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mc.menu.in"
[5]: https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/syntax/Syntax.in "https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/syntax/Syntax.in"
[6]: https://source.midnight-commander.org/man/mc.html "https://source.midnight-commander.org/man/mc.html"
[7]: https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mcedit.menu.in "https://raw.githubusercontent.com/MidnightCommander/mc/master/misc/mcedit.menu.in"
[8]: https://github.com/ratatui/ratatui "https://github.com/ratatui/ratatui"
[9]: https://github.com/crossterm-rs/crossterm "https://github.com/crossterm-rs/crossterm"
[10]: https://github.com/tokio-rs/tokio "https://github.com/tokio-rs/tokio"
[11]: https://crates.io/crates/portable-pty "https://crates.io/crates/portable-pty"
[12]: https://docs.rs/walkdir/ "https://docs.rs/walkdir/"
[13]: https://crates.io/crates/ignore "https://crates.io/crates/ignore"
[14]: https://github.com/notify-rs/notify "https://github.com/notify-rs/notify"
[15]: https://docs.rs/camino "https://docs.rs/camino"
[16]: https://docs.rs/ssh2 "https://docs.rs/ssh2"
[17]: https://docs.rs/russh "https://docs.rs/russh"
[18]: https://crates.io/crates/suppaftp "https://crates.io/crates/suppaftp"
[19]: https://docs.rs/openssh "https://docs.rs/openssh"
[20]: https://crates.io/crates/tar "https://crates.io/crates/tar"
[21]: https://crates.io/crates/zip "https://crates.io/crates/zip"
[22]: https://crates.io/crates/memmap2 "https://crates.io/crates/memmap2"
[23]: https://crates.io/crates/ropey "https://crates.io/crates/ropey"
[24]: https://crates.io/crates/syntect "https://crates.io/crates/syntect"
[25]: https://docs.rs/similar "https://docs.rs/similar"
[26]: https://serde.rs/ "https://serde.rs/"
[27]: https://docs.rs/clap "https://docs.rs/clap"
[28]: https://crates.io/crates/tracing "https://crates.io/crates/tracing"
[29]: https://doc.rust-lang.org/edition-guide/rust-2024/index.html "https://doc.rust-lang.org/edition-guide/rust-2024/index.html"
[30]: https://rust-lang.github.io/api-guidelines/about.html "https://rust-lang.github.io/api-guidelines/about.html"
[31]: https://rust-lang.github.io/rustfmt/ "https://rust-lang.github.io/rustfmt/"
[32]: https://docs.rs/memmap2 "https://docs.rs/memmap2"
[33]: https://docs.rs/git2 "https://docs.rs/git2"

