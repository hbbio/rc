#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

if ! command -v cargo-deny >/dev/null 2>&1; then
  echo "cargo-deny is required; install the version pinned in rust-security.yml" >&2
  exit 127
fi

cd "$REPOSITORY_ROOT"
exec cargo deny "$@"
