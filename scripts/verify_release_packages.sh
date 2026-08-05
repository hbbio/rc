#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

PUBLISH_ARGS=(publish --workspace --all-features --locked --dry-run)
PACKAGE_ARGS=(package --workspace --all-features --locked --no-verify)
VALIDATOR_ARGS=(
  --repository "$REPOSITORY_ROOT"
  --archives "$REPOSITORY_ROOT/target/package"
)
ALLOW_DIRTY=false

case "${1:-}" in
  "") ;;
  --allow-dirty)
    ALLOW_DIRTY=true
    PUBLISH_ARGS+=(--allow-dirty)
    PACKAGE_ARGS+=(--allow-dirty)
    VALIDATOR_ARGS+=(--allow-dirty)
    ;;
  *)
    echo "usage: $0 [--allow-dirty]" >&2
    exit 2
    ;;
esac

if (( $# > 1 )); then
  echo "usage: $0 [--allow-dirty]" >&2
  exit 2
fi

cd "$REPOSITORY_ROOT"
if [[ "$ALLOW_DIRTY" == false ]] && [[ -n "$(git status --porcelain=v1)" ]]; then
  echo "release verification requires a clean Git worktree" >&2
  exit 1
fi

VALIDATOR_ARGS+=(--expected-revision "$(git rev-parse HEAD)")
cargo "${PUBLISH_ARGS[@]}"
cargo "${PACKAGE_ARGS[@]}"
python3 "$SCRIPT_DIR/verify_release_packages.py" "${VALIDATOR_ARGS[@]}"
