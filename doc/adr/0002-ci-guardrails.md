# ADR 0002: Add CI Guardrails for Policy, Speed, and Compatibility

- Status: Accepted
- Date: 2026-03-02

## Context

The project now requires production-grade guardrails:
- faster and more reliable test execution
- dependency/security policy enforcement
- unused dependency detection
- explicit MSRV compatibility checks
- coverage trend visibility

## Decision

Add CI jobs for:

1. `nextest` test gate (`cargo-nextest`)
2. `cargo-deny` dependency/security policy gate
3. `cargo-udeps` unused dependency gate
4. MSRV check gate on pinned toolchain version
5. Coverage trend tracking job with published summaries/artifacts

Also:

- Keep existing `fmt`, `clippy -D warnings`, and `test` checks.
- Use locked dependency resolution in CI commands.

## Consequences

Positive:

- Higher confidence in dependency hygiene and supply-chain posture.
- Faster test feedback loop with `nextest`.
- Explicit backward compatibility policy via MSRV CI.

Tradeoffs:

- Longer CI runtime and more maintenance of tool configs.
- Occasional toolchain-specific breakages require coordinated updates.

