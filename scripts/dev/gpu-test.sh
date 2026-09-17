#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Sync, build, test, doctor on the GPU box; copy artifacts into TECHNICAL_REPORTS/runs/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TS="$(date -u +%Y-%m-%dT%H%M%SZ)"
RUN_DIR="${ROOT}/TECHNICAL_REPORTS/runs/${TS}"
mkdir -p "${RUN_DIR}"

"${ROOT}/scripts/dev/gpu-sync.sh"

# shellcheck source=/dev/null
source "${ROOT}/scripts/dev/gpu-run.sh" --source-only

echo "=== cargo build --release ===" | tee "${RUN_DIR}/build.log"
gpu_run cargo build --release 2>&1 | tee -a "${RUN_DIR}/build.log"

echo "=== cargo test --release ===" | tee "${RUN_DIR}/test.log"
gpu_run cargo test --release 2>&1 | tee -a "${RUN_DIR}/test.log"

echo "=== gpm_parity (ALL_SMI_LIVE_GPU=1) ===" | tee "${RUN_DIR}/gpm_parity.log"
gpu_run env ALL_SMI_LIVE_GPU=1 cargo test --release --test gpm_parity -- --nocapture \
  2>&1 | tee -a "${RUN_DIR}/gpm_parity.log" || true

echo "=== doctor --json ===" | tee "${RUN_DIR}/doctor.log"
# Doctor may exit non-zero on WARN/FAIL; still capture JSON.
gpu_run ./target/release/all-smi doctor --json > "${RUN_DIR}/doctor.json" || true
# Also archive human-readable summary of the new checks.
gpu_run ./target/release/all-smi doctor --only nvidia.gpm,nvidia.dcgm \
  > "${RUN_DIR}/doctor-nvidia-deep.txt" || true

echo "=== versions ===" | tee "${RUN_DIR}/versions.txt"
gpu_run nvidia-smi --query-gpu=name,driver_version --format=csv > "${RUN_DIR}/versions.txt" || true
{
  echo "---"
  gpu_run dcgmi --version || true
  echo "---"
  gpu_run rustc --version || true
} >> "${RUN_DIR}/versions.txt"

echo "run artifacts in ${RUN_DIR}"
