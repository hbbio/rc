# Bounded Contexts

This document defines stable domain boundaries for the current architecture.

## Context Map

1. `interaction` context
- Owns key decoding, command mapping, and command dispatch entry points.
- Must not perform filesystem I/O directly.
- Primary code: `crates/app/src/main.rs`, `crates/core/src/keymap.rs`.

2. `orchestration` context
- Owns `AppState`, route transitions, dialog state, and panel/viewer workflows.
- May enqueue jobs but must not execute job side effects directly.
- Primary code: `crates/core/src/lib.rs`, `crates/core/src/orchestration.rs`.

3. `runtime-jobs` context
- Owns async runtime loop, worker scheduling, cancellation, and backpressure.
- Only context that executes queued jobs and emits worker/background events.
- Primary code: `crates/app/src/runtime.rs`, `crates/core/src/jobs.rs`, `crates/core/src/background.rs`.

4. `rendering` context
- Owns ratatui view models, layout, and skin materialization.
- Must be deterministic for a given `AppState` snapshot.
- Primary code: `crates/ui/src/lib.rs`, `crates/ui/src/skin.rs`.

5. `configuration` context
- Owns settings persistence, mc keymap parsing, and compatibility formats.
- Must not mutate runtime queues or UI directly.
- Primary code: `crates/core/src/settings.rs`, `crates/core/src/settings_io.rs`, `crates/core/src/keymap.rs`.

6. `shell-process` context
- Owns process invocation and cancellation-safe shell execution primitives.
- Must expose narrow APIs consumed by `runtime-jobs` and orchestration adapters.
- Primary code: `crates/shell/src/lib.rs`.

## Boundary Rules

1. `interaction -> orchestration` only through `AppCommand` and `ApplyResult`.
2. `orchestration -> runtime-jobs` only through `WorkerCommand` and `JobRequest`.
3. `runtime-jobs -> orchestration` only through `JobEvent` and `BackgroundEvent`.
4. `rendering` receives state and returns terminal draw output; no direct mutation of app state.
5. `configuration` and `shell-process` are service providers; they do not own routing state.

## Refactor Rule For Phase 2

When splitting `crates/core/src/lib.rs`, new modules must map back to one of the six contexts above.

