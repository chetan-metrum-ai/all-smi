#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Archive doctor + versions + hollow check into TECHNICAL_REPORTS/runs/<ts>/.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TS="$(date -u +%Y-%m-%dT%H%M%SZ)"
RUN_DIR="${ROOT}/TECHNICAL_REPORTS/runs/${TS}"
mkdir -p "${RUN_DIR}"

# shellcheck source=/dev/null
source "${ROOT}/scripts/dev/gpu-run.sh" --source-only

gpu_run ./target/release/all-smi doctor --json > "${RUN_DIR}/doctor.json" || true
gpu_run ./target/release/all-smi doctor --only nvidia.gpm,nvidia.dcgm \
  > "${RUN_DIR}/doctor-nvidia-deep.txt" || true
gpu_run nvidia-smi --query-gpu=name,driver_version --format=csv > "${RUN_DIR}/versions.txt" || true
{
  echo "---"
  gpu_run dcgmi --version || true
  echo "---"
  gpu_run rustc --version || true
} >> "${RUN_DIR}/versions.txt"
gpu_run bash scripts/dev/remote-hollow-check.sh > "${RUN_DIR}/hollow-check.txt" || true

echo "RUN_DIR=${RUN_DIR}"
