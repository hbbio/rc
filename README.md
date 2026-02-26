# rc

`rc` is an in-progress Rust TUI file manager inspired by GNU Midnight Commander.

The goal is MC-compatible behavior and keymaps, with a modern internal architecture
that keeps the UI responsive while long operations run.

## Current status

This repository is actively developed and already usable for core workflows.

Implemented milestones:

- Milestone 0: workspace skeleton, app loop, tracing, CLI
- Milestone 1: dual panels, navigation, sorting, tagging, dialogs
- Milestone 2: copy/move/mkdir/delete with background jobs and cancel
- Milestone 3: read-only viewer with search, goto, wrap, syntax highlighting
- Milestone 4 (partial): find dialog/results, tree, and hotlist
- Settings overhaul (partial): mc-shaped Options menu, typed settings model, Save setup persistence

Planned next major milestones include `mc.ext.ini`, user menu, editor, diff viewer,
remote VFS, and subshell integration. See [doc/roadmap.md](doc/roadmap.md).

## Quick start

Requirements:

- Rust stable toolchain
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

`rc` looks up skins in `crates/ui/assets/skins` (bundled originals) and standard
system locations like `/usr/share/mc/skins` and Homebrew paths.

## Settings and setup

- Options menu now follows MC categories:
  `Configuration`, `Layout`, `Panel options`, `Confirmation`, `Appearance`,
  `Display bits`, `Learn keys`, `Virtual FS`, and `Save setup`.
- Settings are loaded with deterministic precedence:
  built-in defaults -> persisted config -> environment overrides -> CLI flags.
- `Save setup` persists to:
  - `~/.config/rc/settings.ini` for rc-owned settings.
  - `~/.config/mc/ini` for MC-compatible skin key.
- Skin discovery uses ordered search roots:
  custom configured dirs, then bundled/system MC skin directories.

## Key controls (current defaults)

Main file manager:

- `Tab`: switch active panel
- `Enter` / `F3`: open directory or open file in viewer
- `F4`: edit file using `$EDITOR`, then `$VISUAL`, with internal fallback
- `Backspace`: go to parent directory
- `F5` copy, `F6` move, `F7` mkdir, `F8` delete, `F2` rename/move
- `Ctrl-J`: open jobs screen
- `Alt-J`: cancel latest/selected job
- `Alt-F`, `M-?`, `Ctrl-/`: open find dialog
- `Alt-T`: open tree
- `Alt-H`: open hotlist
- `q` / `Esc`: quit

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
- `doc/roadmap.md`: feature plan and milestone breakdown

## Development

Run all checks locally:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

CI runs the same checks on pushes and pull requests via:

- `.github/workflows/ci.yml`

## License

GPL-3.0-or-later.
