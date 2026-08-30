#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"

PUBLISH_ARGS=(publish --workspace --all-features --locked --dry-run)
PACKAGE_ARGS=(package --workspace --all-features --locked --no-verify)
VALIDATOR_ARGS=(
  --repository "$REPOSITORY_ROOT"
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

# `cargo publish --workspace` stages the selected packages in a temporary registry so
# interdependent workspace crates can be verified as one coordinated release. Keep its build
# artifacts isolated: a shared target directory can otherwise reuse metadata for an already
# published crate with the same name and version instead of compiling the locally staged archive.
readonly RELEASE_STAGING_PARENT="${TMPDIR:-/tmp}"
readonly RELEASE_TARGET_DIR="$(
  mktemp -d "${RELEASE_STAGING_PARENT%/}/rc-release-packages.XXXXXX"
)"
cleanup_release_target() {
  rm -rf -- "$RELEASE_TARGET_DIR"
}
trap cleanup_release_target EXIT

PUBLISH_ARGS+=(--target-dir "$RELEASE_TARGET_DIR")
PACKAGE_ARGS+=(--target-dir "$RELEASE_TARGET_DIR")
VALIDATOR_ARGS+=(--archives "$RELEASE_TARGET_DIR/package")
VALIDATOR_ARGS+=(--expected-revision "$(git rev-parse HEAD)")
cargo "${PUBLISH_ARGS[@]}"
cargo "${PACKAGE_ARGS[@]}"
python3 "$SCRIPT_DIR/verify_release_packages.py" "${VALIDATOR_ARGS[@]}"
