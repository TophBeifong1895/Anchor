#!/usr/bin/env bash
# cargo test with the tests' temporary files on tmpfs.
#
# The Host's stores fsync every durable write. On a disk filesystem each fsync
# costs milliseconds and the suite spends most of its time there; tmpfs keeps
# the same code paths and makes them cheap. The scratch directory lives under
# XDG_RUNTIME_DIR, not /dev/shm, because sandboxed tests reserve /dev, and its
# path stays short so Unix socket paths built inside it fit.
#
# Usage: scripts/test.sh [cargo test arguments...]
set -euo pipefail

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
runtime=${XDG_RUNTIME_DIR:-}
if [[ -n $runtime && $(stat -f -c %T -- "$runtime" 2>/dev/null) == tmpfs ]]; then
  scratch=$(mktemp -d -- "$runtime/at.XXXXXX")
  # Some tests leave read-only release trees; restore write access to remove them.
  trap 'chmod -R u+w -- "$scratch" 2>/dev/null; rm -rf -- "$scratch"' EXIT
  export TMPDIR=$scratch
else
  echo "scripts/test.sh: XDG_RUNTIME_DIR is not tmpfs; temporary files stay in ${TMPDIR:-/tmp}" >&2
fi
cargo +stable test --manifest-path "$repo/rust/Cargo.toml" "$@"
