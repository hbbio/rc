# Milestone 4 completion report

Status: **complete** as of 2026-08-04.

Milestone 4 now satisfies the acceptance backlog derived from the
[project roadmap](roadmap.md#milestone-4-find-file-tree-hotlist-panelize) and the
[Midnight Commander manual](https://source.midnight-commander.org/man/mc.html).

## 1. Find file

- [x] Full search form with editable starting directory and tree picker, filename
  pattern, glob/regex mode, case sensitivity, optional content and whole-word search,
  ignored-directory list, and match-all empty filename patterns.
- [x] Compiled matchers and bounded iterative traversal with streamed result chunks.
- [x] Pause/continue, Again, exact-job cancellation, close-time cancellation, and a
  defined replacement policy for paused searches.
- [x] Separate running, paused, canceling, completed, partial, canceled, and failed
  states, including result-limit truncation and bounded read-error summaries.
- [x] Stable selection while results stream and when a selected result is located in a
  panel.
- [x] Coverage for all matcher modes, content search, ignored directories, limits,
  permissions, stale/closed searches, concurrent jobs, rendering, and the real runtime
  bridge.

## 2. Directory tree

- [x] Sorted preorder construction with contiguous subtrees and an index for parent,
  child, sibling, and subtree-boundary lookups.
- [x] Cancellation checks during traversal, structured scan issues, request/job
  correlation, stale-result rejection, close-time cancellation, and explicit depth or
  entry truncation.
- [x] Static/dynamic navigation, left/right parent-child movement, rescan, forget,
  incremental search, and copy/move/mkdir/delete through existing job flows.
- [x] Focused builder, index, cancellation, stale-result, truncation, mutation, route,
  rendering, and runtime tests.

Tree-as-a-panel and MC persistent-tree-cache compatibility remain intentionally
deferred; this milestone covers the standalone tree screen.

## 3. Directory hotlist

- [x] Labeled `HotlistEntry { label, path }` storage with backward migration from
  legacy path-only settings.
- [x] Add, edit, confirmed or immediate remove, and open workflows with duplicate,
  missing, non-directory, and inaccessible-path diagnostics.
- [x] MC-compatible `Ctrl-X H` quick-add and a persisted hotlist-deletion confirmation
  setting.
- [x] Persistence, migration, validation, stale-editor, cursor, rendering, keymap, and
  application input coverage.

## 4. External and find panelize

- [x] Named `PanelizePreset { label, command }` settings with legacy command-only
  migration.
- [x] Preset add/edit/remove UX, duplicate validation, deletion confirmation, stale
  editor protection, and visible `Tab`/`F2`/`F4`/`F8` hints.
- [x] Independent latest-result history for each panel, restored without rerunning the
  command and preserving cursor and tags.
- [x] Exact panel-refresh cancellation and stale-refresh rejection.
- [x] Bounded adaptive streaming into the visible virtual panel, with authoritative
  final sorting, stable selection, and complete rollback on failure or cancellation.
- [x] Coverage for migration, history, stale and failed refreshes, cancellation,
  streaming, restored-result file operations, rendering, and runtime execution.

## 5. Quick CD

- [x] File-menu and bundled `Alt-C`/`CdQuick` entry points.
- [x] Shell-style quoting for a single path, relative and absolute paths, `.`/`..`
  lexical normalization, current/named-user home expansion on Unix, and per-panel `-`
  history.
- [x] Invalid input reopens intact with a specific diagnostic; successful navigation
  uses the request-correlated asynchronous panel refresh path.
- [x] Resolver, menu, keymap, dialog, history, and application dispatch tests.

## 6. Cross-cutting acceptance

- [x] Mouse click selection and double-click activation for find results, tree,
  hotlist, and panelize preset lists.
- [x] One shared overlay/viewport model for rendering and hit-testing, including
  scrolling and small-terminal clamping.
- [x] Render tests for the find form/results, tree states, labeled hotlist, and named
  panelize presets.
- [x] Application/runtime coverage for streamed find and panelize work, cancellation,
  input routing, Quick CD dispatch, and double-click activation.
- [x] In-app help and README updated; the README no longer marks Milestone 4 partial.
- [x] Complete Rust 1.97 formatting, Clippy, test, MSRV, dependency-policy, unused
  dependency, and coverage-trend gates required before merge.

## Architectural outcome

Milestone 4 uses bounded data flows and explicit state machines rather than blocking
the UI or inferring ownership from “latest job” state. Search and tree traversal are
iterative and cancellation-aware; tree navigation is index-backed; panelize output is
bounded and adaptively streamed; every asynchronous result is correlated to the request
that owns it. Rendering and mouse hit-testing share the same geometry and visible-window
algorithm, eliminating a common class of TUI interaction drift.
