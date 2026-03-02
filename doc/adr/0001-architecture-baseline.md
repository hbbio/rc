# ADR 0001: Freeze Bounded Contexts and Crate Contracts

- Status: Accepted
- Date: 2026-03-02

## Context

`rc` currently evolves quickly across core orchestration, runtime, and rendering.
Large refactors are planned in upcoming phases. Without explicit boundaries, refactors risk
regressions, circular dependencies, and ownership ambiguity.

## Decision

1. Adopt six bounded contexts:
- interaction
- orchestration
- runtime-jobs
- rendering
- configuration
- shell-process

2. Freeze crate contracts and dependency direction:
- `app -> core, ui`
- `ui -> core`
- `core -> shell`

3. Enforce module ownership map for cross-context reviews.

4. Treat this baseline as the canonical map for Phase 2 decomposition.

## Consequences

Positive:

- Reduces monolithic refactor risk.
- Makes module split work auditable against a stable context model.
- Clarifies change ownership and review surfaces.

Tradeoffs:

- Adds up-front documentation overhead.
- Boundary changes now require ADR updates, increasing process rigor.

