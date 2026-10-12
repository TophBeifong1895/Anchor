#!/usr/bin/env bash
# Install a reviewed complete platform; credentials are parsed by native code.
set -euo pipefail

usage() {
  cat <<'HELP'
Usage: scripts/install-platform.sh --archive PLATFORM.tar.gz --sha256 TRUSTED_SHA256
       --install-root NEW_DIRECTORY --model-config PRIVATE_MODEL.env
       [--installer /reviewed/anchor-platform]

Create a private installation, never overwrite an existing root. Only model
URL/API key/model name and optional model settings are required; Python/Rust
and package sources are prepared later by Pilot. The model file is KEY=value
data, not shell code, and is kept at <install-root>/config/model.env (0600).
After installation run <install-root>/release/anchor-runtime/bin/anchor-platform start.
Open http://127.0.0.1:8077. No startup or service registration is done here.

Supported machine: Linux x86_64, Bubblewrap, slirp4netns, Git, shell, a C linker,
CA certificates and the runtime shared libraries recorded in the manifest.
HELP
}

installer=
archive=
digest=
arguments=()
while (($#)); do
  case "$1" in
    --installer) installer=${2:?--installer needs a binary}; shift 2 ;;
    --archive) archive=${2:?--archive needs an archive}; shift 2 ;;
    --sha256|--install-root|--model-config)
      [[ "$1" != --sha256 ]] || digest=${2:?--sha256 needs a trusted digest}
      arguments+=("$1" "${2:?option needs a value}"); shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
  printf 'Platform installation supports Linux x86_64 only.\n' >&2; exit 1;
}
missing=()
for tool in bwrap slirp4netns git sh cc tar sha256sum mktemp chmod cp rm; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
if ((${#missing[@]})); then
  printf 'Missing system prerequisites: %s\n' "${missing[*]}" >&2
  printf 'On Debian/Ubuntu install: sudo apt-get install bubblewrap slirp4netns git build-essential ca-certificates\n' >&2
  exit 1
fi
if [[ -z "$installer" ]]; then
  [[ -n "$archive" && "$digest" =~ ^[[:xdigit:]]{64}$ ]] || { usage >&2; exit 2; }
  staging=$(mktemp -d)
  trap 'rm -rf -- "$staging"' EXIT
  # Hash and extract the same private bytes, even if the supplied archive path
  # changes concurrently. --to-stdout writes only the expected native installer.
  cp -- "$archive" "$staging/platform.tar.gz"
  archive="$staging/platform.tar.gz"
  printf '%s  %s\n' "$digest" "$archive" | sha256sum --check --status
  installer="$staging/anchor-platform"
  tar -xOf "$archive" anchor-runtime/bin/anchor-platform > "$installer"
  chmod 0700 "$installer"
fi
arguments+=(--archive "$archive")
[[ -x "$installer" ]] || { printf 'Supply the reviewed native launcher via --installer.\n' >&2; exit 1; }
"$installer" install "${arguments[@]}"
