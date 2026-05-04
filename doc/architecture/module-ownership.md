# Module Ownership

This map defines ownership boundaries for refactors and reviews.
Ownership is functional (domain stewardship), not a personnel roster.

## Ownership Groups

1. `app-runtime`
- Files: `crates/app/src/runtime.rs`
- Scope: worker scheduling, queue policy, cancellation, shutdown behavior.

2. `app-bootstrap`
- Files: `crates/app/src/main.rs`
- Scope: startup, terminal integration, event loop glue, CLI wiring.

3. `core-orchestration`
- Files: `crates/core/src/lib.rs`, `crates/core/src/orchestration.rs`, `crates/core/src/dialog.rs`
- Scope: `AppState`, command handlers, route/dialog workflows.

4. `core-jobs`
- Files: `crates/core/src/jobs.rs`, `crates/core/src/background.rs`, `crates/core/src/slo.rs`
- Scope: job execution semantics, background event contracts, SLO constants.

5. `core-config`
- Files: `crates/core/src/keymap.rs`, `crates/core/src/settings.rs`, `crates/core/src/settings_io.rs`
- Scope: configuration model, keymap parsing, persistence.

6. `ui-rendering`
- Files: `crates/ui/src/lib.rs`, `crates/ui/src/skin.rs`
- Scope: render pipeline, skin/material style behavior, view caches.

7. `shell-process`
- Files: `crates/shell/src/lib.rs`
- Scope: process backend primitives and OS-specific subprocess behavior.

## Review Rule

Changes that cross ownership groups must include at least one reviewer from each touched group.

## Phase 2 Requirement

As `crates/core/src/lib.rs` is decomposed, every new module must be assigned to exactly one ownership group.

