# rc

`rc` is an in-progress Rust TUI file manager inspired by GNU Midnight Commander.
Already using it as a daily driver.

The goal is MC-compatible behavior and keymaps, with a modern internal architecture that
keeps the UI responsive while long operations run, without requiring a strict 1:1
reimplementation of every MC subsystem.

## Current status

This repository is actively developed with AI assistance but human oversight and already
usable for core workflows.

Implemented milestones:

- Milestone 0: workspace skeleton, app loop, tracing, CLI
- Milestone 1: dual panels, navigation, sorting, tagging, dialogs
- Milestone 2: copy/move/mkdir/delete with background jobs, progress, overwrite
  policies, and cancellation
- Milestone 3: read-only viewer with search, goto, wrap, syntax highlighting
- Milestone 4: complete find workflow, directory tree, labeled hotlist, external/find
  panelize, Quick CD, mouse interaction, and core Left/Right panel controls
- Settings overhaul (partial): mc-shaped Options menu, typed settings model, Save setup
  persistence
- External editor workflow: deterministic resolution, terminal suspend/resume, command
  templates
- Product direction update: external-editor-first workflow, command-based diff output,
  optional FTP/SFTP support

Recent Milestone 4 and reliability progress:

- Find compiles glob or regular-expression matchers once per search, supports optional
  whole-word content matching and ignored directories, streams stable selections, and reports
  truncation and bounded read errors distinctly from cancellation or failure.
- Tree scanning is iterative, cancellation-aware, request-correlated, sorted in preorder, and
  indexed for parent/child/subtree operations. Static/dynamic navigation, incremental search,
  rescan/forget, and file operations are complete.
- Hotlist entries persist editable labels and paths with legacy migration, duplicate/path
  validation, optional deletion confirmation, and `Ctrl-X H` quick-add.
- External panelize uses named presets, bounded adaptive streaming, exact-job cancellation, and
  per-panel result history; find results use the same virtual-panel layer.
- Quick CD supports quoted relative/absolute paths, `~`, Unix `~user`, and per-panel
  `cd -` history.
- Find results, tree, hotlist, and panelize preset lists support click selection and double-click
  activation from a renderer-shared hit-test layout.
- Left/Right menus provide targeted File listing, Quick view, Info, Tree, Panelize, and Rescan
  actions; persisted Full/Brief/Long formats, complete sort fields, and glob/regex filters are
  independent for each panel.
- Quick view uses cancelable request-correlated background reads. Listing filters preserve hidden
  tags and selection where possible, and reuse cached panelized results instead of rerunning a
  command.
- Tagged selection totals use cancelable background traversal and include complete directory
  contents, while overlapping trees are counted once and unreadable entries are reported as a
  partial total.

The remaining inactive Left/Right entries are explicitly later work: user-defined listing
formats in Milestone 5, FTP/SFTP in Milestone 8, Shell links in Milestone 9, and lossless legacy
filename transcoding in Milestone 10.

Planned next major milestones include `mc.ext.ini`, user menu, command-based diff
integration (`difftastic`/`diff`), optional remote VFS, and subshell integration.
See [doc/roadmap.md](doc/roadmap.md).

## Quick start

Requirements:

- Rust 1.88.0 or newer
- A terminal with ANSI support

Install from a local checkout:

```bash
cargo install --path crates/app --locked
```

Note: recent Cargo versions require `--path` for local installs.

Run:

```bash
cargo run -p rc
```

Optional arguments:

```bash
cargo run -p rc -- --path /some/start/dir --tick-rate-ms 200
```

Select an `mc` skin:

```bash
cargo run -p rc -- --skin modarin256
cargo run -p rc -- --skin julia256 --skin-dir /path/to/mc/skins
```

`rc` looks up skins in `crates/ui/assets/skins` (bundled originals) and standard system
locations like `/usr/share/mc/skins` and Homebrew paths.

## Settings and setup

- Options menu now follows MC categories: `Configuration`, `Layout`, `Panel options`,
  `Confirmation`, `Appearance`, `Display bits`, `Learn keys`, `Virtual FS`, and
  `Save setup`.

- Settings are loaded with deterministic precedence: built-in defaults -> persisted
  config -> environment overrides -> CLI flags.

- `Save setup` persists to:
  - `~/.config/rc/settings.ini` for rc-owned settings.
  - `~/.config/mc/ini` for MC-compatible skin key.

- Skin discovery uses ordered search roots: custom configured dirs, then bundled/system
  MC skin directories.

## Key controls (current defaults)

Main file manager:

- `Tab`: switch active panel
- `Enter` / `F3`: open directory or open file in viewer
- `F4`: edit file using `editor_command`, `$EDITOR`, `$VISUAL`, or PATH probes (`hx`,
  `nvim`, `vim`, `vi`, `emacs`)
- `Space` / `Insert` / `Ctrl-T`: toggle selected item
- `Backspace`: go to parent directory
- `Alt-C`: Quick CD (`~`, Unix `~user`, relative/absolute paths, or `-` for previous)
- `F5` copy, `F6` move, `F7` mkdir, `F8` delete, `F2` rename/move
- `Ctrl-J`: open jobs screen
- `Alt-J`: cancel latest/selected job
- `Alt-F`, `M-?`, `Ctrl-/`: open find dialog
- `Alt-T`: open tree
- `Alt-H`: open hotlist
- `Alt-P` / `Ctrl-P` or `Ctrl-X` then `!`: open external panelize
- `F9`: open menus; Left/Right configure either panel's view, format, sort, and filter
- `Ctrl-X i` / `Ctrl-X q`: show Info / Quick view in the passive panel
- `Alt-Shift-T`: cycle Full, Brief, and Long formats on the active panel
- `Shift-F6` / `Shift-F8`: cycle sort field / toggle reverse order
- `q` / `Esc`: quit

Milestone 4 screens:

- Find results: `F4` search again, `F5` panelize, `F6` pause/continue, `Alt-J` cancel
  the exact search.
- Tree: arrows navigate, `F2` rescan, `F3` forget subtree, `F4` static/dynamic mode,
  `F5`/`F6`/`F7`/`F8` copy/move/mkdir/delete.
- Hotlist: `a` add, `e`/`F4` edit, `d`/`Delete` remove, `Enter` open.
- Panelize presets: `Tab` custom command, `F2` add, `F4` edit, `F8` remove,
  `Enter` run. The side-panel `Panelize` menu entry restores that panel's latest results.
- Mouse: click a result/list entry to select it; double-click to open or run it.

Viewer:

- `F7` / `Ctrl-S`: search
- `Shift-F7`: search backward
- `n` / `Shift-n`: continue search forward/backward
- `g` / `Alt-L`: goto
- `w`: toggle wrap
- `h`: toggle hex/text mode
- `Esc` / `q` / `F10`: close viewer

Notes:

- Default bindings are loaded from `crates/core/assets/mc.default.keymap`.
- Common macOS Option-symbol variants are normalized for keymap matching.

## Project layout

- `crates/app`: terminal app entrypoint, event loop, input normalization
- `crates/core`: domain model, commands, routes, file operations, jobs, keymap parser
- `crates/ui`: ratatui rendering layer
- `crates/shell`: process backend primitives used by core/runtime
- `doc/roadmap.md`: feature plan and milestone breakdown
- `doc/architecture/`: bounded contexts, crate contracts, ownership map
- `doc/adr/`: architecture decision records

## Development

Runtime tracing is written to `~/.config/rc/rc.log` instead of the terminal so
diagnostics cannot corrupt the alternate-screen UI. Set `RC_LOG_FILE` to use a
different path and `RUST_LOG` to change the default `warn` filter. Logs at or
above 8 MiB are reset on the next startup.

Run baseline checks locally:

```bash
cargo --locked fmt --all --check
cargo --locked clippy --all-targets --all-features -- -D warnings
cargo --locked test --all-targets --all-features
```

Additional local equivalents of CI policy/perf checks (requires extra cargo tools):

```bash
cargo +1.88.0 check --workspace --all-targets --locked
cargo --locked nextest run --workspace --all-targets --all-features
cargo deny check bans licenses advisories sources
cargo +nightly udeps --workspace --all-targets --all-features --locked
mkdir -p target/coverage
cargo --locked llvm-cov --workspace --all-targets --all-features --json --output-path target/coverage/llvm-cov.json
./scripts/coverage_trend.sh target/coverage/llvm-cov.json .github/coverage-baseline.json
```

CI runs all required gates on pushes and pull requests via:

- `.github/workflows/ci.yml`

## License

GPL-3.0-or-later, as this project is derived from the original
[midnight commander](https://github.com/MidnightCommander/mc).
