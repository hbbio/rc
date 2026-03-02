# Crate Contracts

This file defines crate-level contracts and allowed dependency direction.

## Dependency Direction

Allowed:

- `app -> core, ui`
- `ui -> core`
- `core -> shell`
- `shell -> (std + platform crates)`

Disallowed:

- `core -> app`
- `ui -> app`
- `shell -> core|ui|app`

## `rc` (`crates/app`)

Responsibilities:

- Program entrypoint and terminal lifecycle.
- Runtime bridge, async worker loop, shutdown sequence.

Must not own:

- Business/domain state model details that belong in `rc-core`.
- Rendering logic that belongs in `rc-ui`.

## `rc-core` (`crates/core`)

Responsibilities:

- Canonical domain state (`AppState`) and command semantics.
- Job request/response models and non-UI orchestration logic.
- Compatibility parsers (keymaps/settings) and workflow policies.

Must not own:

- Terminal drawing primitives.
- Tokio runtime orchestration internals.

## `rc-ui` (`crates/ui`)

Responsibilities:

- Stateless rendering of `AppState` into frame output.
- Skin parsing/materialization and rendering caches with bounded behavior.

Must not own:

- Filesystem or process side effects.
- Command queueing and runtime scheduling.

## `rc-shell` (`crates/shell`)

Responsibilities:

- Process backend abstraction used by core/runtime workflows.
- Platform-specific process control and cancellation handling.

Must not own:

- Routing state.
- Terminal UI concerns.

