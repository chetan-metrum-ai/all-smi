#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Regenerate vendored DCGM bindgen bindings (run on a glibc Linux host with
# datacenter-gpu-manager-4-dev installed).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INCLUDE="${DCGM_INCLUDE_DIR:-/usr/include}"
OUT="${ROOT}/src/bindings/dcgm.rs"

command -v bindgen >/dev/null || {
  echo "bindgen not found; cargo install bindgen-cli" >&2
  exit 1
}

bindgen "${ROOT}/wrapper.h" \
  -o "${OUT}" \
  --allowlist-function 'dcgm.*' \
  --allowlist-type 'dcgm.*' \
  --allowlist-type 'DCGM_.*' \
  --allowlist-var 'DCGM_.*' \
  -- \
  "-I${INCLUDE}"

echo "wrote ${OUT}"
