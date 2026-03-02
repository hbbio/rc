#!/usr/bin/env bash
set -euo pipefail

COVERAGE_JSON_PATH="${1:-target/coverage/llvm-cov.json}"
BASELINE_PATH="${2:-.github/coverage-baseline.json}"
MAX_DROP="${COVERAGE_MAX_DROP:-2.0}"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for coverage trend parsing" >&2
  exit 1
fi

if [[ ! -f "$COVERAGE_JSON_PATH" ]]; then
  echo "coverage json not found: $COVERAGE_JSON_PATH" >&2
  exit 1
fi

CURRENT_LINE_COVERAGE="$(jq -r '.data[0].totals.lines.percent // empty' "$COVERAGE_JSON_PATH")"
if [[ -z "$CURRENT_LINE_COVERAGE" || "$CURRENT_LINE_COVERAGE" == "null" ]]; then
  echo "unable to parse line coverage from $COVERAGE_JSON_PATH" >&2
  exit 1
fi

BASELINE_LINE_COVERAGE="0"
if [[ -f "$BASELINE_PATH" ]]; then
  BASELINE_LINE_COVERAGE="$(jq -r '.line_coverage_percent // 0' "$BASELINE_PATH")"
fi

LINE_COVERAGE_DELTA="$(
  awk -v current="$CURRENT_LINE_COVERAGE" -v baseline="$BASELINE_LINE_COVERAGE" 'BEGIN { printf "%.2f", current - baseline }'
)"

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "line_coverage_current=$CURRENT_LINE_COVERAGE"
    echo "line_coverage_baseline=$BASELINE_LINE_COVERAGE"
    echo "line_coverage_delta=$LINE_COVERAGE_DELTA"
  } >>"$GITHUB_OUTPUT"
fi

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "## Coverage Trend"
    echo ""
    echo "| Metric | Value |"
    echo "| --- | ---: |"
    echo "| Current line coverage | ${CURRENT_LINE_COVERAGE}% |"
    echo "| Baseline line coverage | ${BASELINE_LINE_COVERAGE}% |"
    echo "| Delta | ${LINE_COVERAGE_DELTA} pp |"
    echo "| Allowed drop | ${MAX_DROP} pp |"
  } >>"$GITHUB_STEP_SUMMARY"
fi

if awk -v current="$CURRENT_LINE_COVERAGE" -v baseline="$BASELINE_LINE_COVERAGE" -v max_drop="$MAX_DROP" 'BEGIN { exit !((baseline - current) > max_drop) }'; then
  echo "coverage regression exceeded allowed drop (${MAX_DROP}pp): baseline=${BASELINE_LINE_COVERAGE}% current=${CURRENT_LINE_COVERAGE}%" >&2
  exit 1
fi

