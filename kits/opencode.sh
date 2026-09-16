#!/usr/bin/env sh
# Kit: install the opencode CLI inside a fresh workspace container.
# Kits run at container boot via PodmanProvider::exec, never baked into
# the image (ADR-0006: pin version, rollback = pin version).
#
#   ADE_OPENCODE_VERSION  exact version to install ("1.18.30"), or unset/
#                         "latest" for the newest release.
#
# Strategy: keep a matching install, else prefer npm (pinnable), else the
# official install script (latest only). Every path ends in `verify`.
set -eu

want="${ADE_OPENCODE_VERSION:-latest}"

installed_version() {
  if command -v opencode >/dev/null 2>&1; then
    # `opencode --version` prints e.g. "1.18.30".
    opencode --version 2>/dev/null | head -n 1 | tr -d '[:space:]'
  fi
}

verify() {
  if ! command -v opencode >/dev/null 2>&1; then
    echo "kit(opencode): verify failed — opencode not on PATH" >&2
    return 1
  fi
  echo "kit(opencode): ready: $(opencode --version 2>/dev/null || echo unknown)"
}

have_version() {
  [ -n "$1" ] && [ "$1" != "unknown" ]
}

current="$(installed_version || true)"
if have_version "$current"; then
  if [ "$want" = "latest" ] || [ "$want" = "$current" ]; then
    echo "kit(opencode): already installed: $current"
    verify
    exit 0
  fi
  echo "kit(opencode): have $current, want $want — reinstalling"
fi

if command -v npm >/dev/null 2>&1; then
  if [ "$want" = "latest" ]; then
    spec="opencode-ai@latest"
  else
    spec="opencode-ai@$want"
  fi
  echo "kit(opencode): npm install -g $spec"
  npm install -g "$spec"
  verify
  exit 0
fi

if [ "$want" != "latest" ]; then
  echo "kit(opencode): cannot pin $want without npm in the guest" >&2
  exit 1
fi

echo "kit(opencode): official install script (latest)"
curl -fsSL https://opencode.ai/install | bash
verify
