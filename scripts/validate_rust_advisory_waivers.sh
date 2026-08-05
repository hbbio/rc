#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

exec python3 "$SCRIPT_DIR/validate_rust_advisory_waivers.py" \
  "$REPOSITORY_ROOT/deny.toml"
