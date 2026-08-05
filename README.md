# rust commander (rc)

*rust commander* is an in-progress Rust TUI file manager inspired by GNU Midnight
Commander. I'm already using it as a daily driver and, even after 20 years of using `mc`,
I find it improves over the original in multiple ways:

- faster startup
- async operations
- better keybindings due to the removal of immediate shell
- Quick CD is really quick (keybinding: `/`)

The goal is to provide mc-inspired behavior and keymaps, with a modern internal
architecture that keeps the UI responsive while long operations run, without requiring a
strict 1:1 reimplementation of every mc subsystem.

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
- Deliberate shell-key policy: there is no always-live shell input, so file-manager
  keys remain available; `/` opens Quick CD and `>` is reserved for a future explicit
  shell-command prompt

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
  `cd -` history. Arbitrary case-insensitive substrings search directories from the
  current directory, home, and filesystem root in a bounded, cancelable background scan;
  ranked results stream into an arrow-selectable list.
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

## Installation

Requirements:

- Rust 1.88.0 or newer
- A terminal with ANSI support

Install Rust Commander from [`crates.io`](https://crates.io/crates/rust-commander):

```bash
cargo install rust-commander --locked
```

Then launch it with:

```bash
rc
```

The package is named `rust-commander`; the installed executable is intentionally named
`rc`.

To build and run from a local checkout instead:

```bash
cargo run -p rust-commander --locked
```

Optional arguments:

```bash
rc --path /some/start/dir --tick-rate-ms 200
```

Select an `mc` skin:

```bash
rc --skin modarin256
rc --skin julia256 --skin-dir /path/to/mc/skins
```

`rc` embeds its bundled original skins in the binary and also discovers custom and system
skins in locations such as `/usr/share/mc/skins` and Homebrew paths.

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
- `/` or `Alt-C`: Quick CD; enter an exact path (`~`, Unix `~user`, relative,
  absolute, or `-` for previous) or any substring, then choose ranked matches with
  `Up`/`Down`
- `>`: reserved for a future explicit shell-command prompt; rc has no always-live shell
  input
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
- `Left` / `Right`: move across responsive columns in Brief format
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

- `crates/app` (`rust-commander`): terminal app entrypoint, event loop, input normalization
- `crates/core` (`rust-commander-core`): domain model, commands, routes, operations, jobs
- `crates/ui` (`rust-commander-ui`): ratatui rendering and bundled skin support
- `crates/shell` (`rust-commander-shell`): cancelable process backend primitives
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
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
```

Additional local equivalents of CI policy/perf checks (requires extra cargo tools):

```bash
cargo +1.88.0 check --workspace --all-targets --locked
cargo nextest run --workspace --all-targets --all-features --locked
./scripts/validate_rust_advisory_waivers.sh
cargo deny check bans licenses sources
./scripts/run_cargo_deny.sh \
  --manifest-path Cargo.toml --all-features --locked \
  check --config deny.toml advisories
./scripts/verify_release_packages.sh
cargo +nightly udeps --workspace --all-targets --all-features --locked
mkdir -p target/coverage
cargo llvm-cov --workspace --all-targets --all-features --locked --json --output-path target/coverage/llvm-cov.json
./scripts/coverage_trend.sh target/coverage/llvm-cov.json .github/coverage-baseline.json
```

CI runs all required gates on pushes and pull requests via:

- `.github/workflows/ci.yml`
- `.github/workflows/rust-security.yml`, which also runs weekly against the latest RustSec
  advisory database

## License

GPL-3.0-or-later, as this project is derived from the original
[midnight commander](https://github.com/MidnightCommander/mc).
See [LICENSE](LICENSE) for the complete terms.
