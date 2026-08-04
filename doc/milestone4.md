# Milestone 4 completion roadmap

Milestone 4 has working vertical slices, but it is not ready to lose the “partial”
label. The remaining backlog, based on the
[project roadmap](roadmap.md#milestone-4-find-file-tree-hotlist-panelize), is:

### 1. Find file

- Replace the single “Name contains” input with a proper search form:

  - Editable starting directory and tree picker
  - Filename pattern
  - Glob versus regular-expression mode
  - Case sensitivity
  - Optional content search and whole-word matching
  - Ignored-directory list
  - Permit an empty filename pattern to match everything

  These are part of MC’s documented Find File workflow.
  [MC manual](https://source.midnight-commander.org/man/mc.html)

- Complete the search lifecycle:

  - Stop/pause and continue the active search
  - “Again” to start another search
  - Cancel the specific find job rather than whichever job is newest
  - Cancel a still-running search when its results screen is closed
  - Define what happens to paused results when a new search starts

- Improve result state:

  - Report when `max_find_results` truncated the search
  - Distinguish completed, canceled, failed, and partial results
  - Surface skipped-directory/read errors without flooding the UI
  - Preserve selection while streamed chunks arrive

- Add tests for regex/glob modes, empty patterns, content search, ignored directories,
  truncation, permission failures, closing during search, and concurrent jobs.

Current implementation starts in `crates/core/src/navigation_flow.rs` and
`crates/core/src/background.rs`.

### 2. Directory tree

- Fix the current tree-builder ordering. Siblings are appended in reverse order, and
  children can appear separated from their displayed parent. See
  `crates/core/src/tree_builder.rs`.

- Harden background scanning:

  - Check cancellation during traversal, not only before and after it
  - Return errors instead of silently skipping every failure
  - Correlate results with a job/request ID
  - Ignore stale results after close/reopen
  - Cancel the build when the tree closes
  - Show when depth or entry limits truncate the tree

- Complete tree interaction:

  - Left/right parent-and-child navigation
  - Expand/collapse or MC-style static/dynamic navigation
  - Rescan selected directory
  - Forget/remove a cached subtree
  - Incremental directory-name search
  - Copy, move, mkdir, and delete selected directories through existing job flows

  MC documents Rescan, Forget, static/dynamic navigation, search, and file operations
  as tree actions. [MC manual](https://source.midnight-commander.org/man/mc.html)

- Add focused builder, cancellation, stale-result, truncation, route, and rendering
  tests. Currently tree has only a basic open-directory route test.

### 3. Directory hotlist

- Replace raw `Vec<PathBuf>` entries with something like
  `HotlistEntry { label, path }`. MC displays labels mapped to directories.
  [MC manual](https://source.midnight-commander.org/man/mc.html)

- Implement genuine CRUD:

  - Add with a label and editable path
  - Edit an existing label/path
  - Remove with confirmation
  - Open the selected entry
  - Handle duplicate, missing, and inaccessible paths clearly

- Add backward-compatible settings migration from existing `hotlist=/path` entries.

- Implement the MC-compatible `Ctrl-X H` quick-add flow. The bundled keymap parses
  `HotListAdd`, but `crates/core/src/command_map.rs` does not dispatch it from the
  extended file-manager context.

- Add a hotlist-deletion confirmation setting and tests for persistence, migration,
  cursor clamping, duplicates, invalid paths, and rendering.

Current storage is in `crates/core/src/settings.rs`, with behavior in
`crates/core/src/navigation_flow.rs`.

### 4. External and find panelize

- Change presets from bare commands to named presets, for example
  `PanelizePreset { label, command }`, with settings migration. MC’s workflow saves
  commands under descriptive names.
  [MC manual](https://source.midnight-commander.org/man/mc.html)

- Finish preset management UX:

  - Edit both label and command
  - Reject duplicate names/commands consistently
  - Confirm deletion
  - Show `Tab`, `F2`, `F4`, and `F8` hints in the dialog itself

- Remember the latest external and find panelized results per panel. The side-panel
  “Panelize” entry should restore the latest result set; currently it just opens the
  external-command dialog and results are lost after leaving panelize mode.

- Tie cancellation to the exact panel refresh job instead of generic “latest job”
  cancellation.

- Stream entries into the visible panel while the command runs; process output is
  bounded and read incrementally now, but the panel is populated only after
  completion.

- Add tests for named-preset migration, restore-after-exit, stale refreshes, targeted
  cancellation, failed commands, and file operations from restored results.

Current flow is in `crates/core/src/panelize_flow.rs`.

### 5. Cross-cutting completion work

- Add mouse selection and double-click handling for find results, tree, hotlist, and
  panelize dialogs. Mouse hit-testing currently supports only menus.

- Add UI snapshot/render tests for all four routes.

- Add application-level tests using the real runtime bridge, not only the synchronous
  core test helper.

- Update help and README once the acceptance criteria are satisfied, then remove
  “partial” from Milestone 4.

- Run the complete Rust 1.97 CI suite before merging.

Scope decisions:

- Quick CD is part of Milestone 4 because it is in the core feature inventory and is
  not assigned to another milestone.
- Tree-as-a-panel and compatibility with MC's persistent tree cache are deferred.
  Milestone 4 covers the standalone tree screen; panel modes and cache compatibility
  need a separate design alongside the broader panel/VFS work.

Recommended order: tree correctness/lifecycle → find lifecycle/form → hotlist
model/CRUD → named panelize/history → Quick CD → integration tests and documentation.
