#!/usr/bin/env sh
# Kit skeleton: install the opencode CLI inside a fresh workspace VM (M6).
# Kits run at VM boot via MicroVm::exec, never baked into the image
# (ADR-0004). This skeleton only probes; real install lands with M6.
set -eu

if command -v opencode >/dev/null 2>&1; then
  echo "opencode already installed: $(opencode --version 2>/dev/null || echo unknown)"
  exit 0
fi

echo "opencode not found in guest; M6 will install it here (network-gated)."
exit 0
