#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Run each hollow_util mode for 30s while capturing dcgmi dmon and all-smi snapshot.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TS="$(date -u +%Y-%m-%dT%H%M%SZ)"
OUT="${ROOT}/TECHNICAL_REPORTS/runs/${TS}/parity"
mkdir -p "${OUT}"

# shellcheck source=/dev/null
source "${ROOT}/scripts/dev/gpu-run.sh" --source-only

MODES=(hollow tensor dram pcie)
DCGM_FIELDS="1001,1002,1003,1004,1005,1009,1010,1011,1012"

for mode in "${MODES[@]}"; do
  echo "=== parity mode=${mode} ==="
  mkdir -p "${OUT}/${mode}"

  # Start workload on the GPU box (avoid nested bash -lc; gpu_run already wraps).
  gpu_run pkill -f hollow_util.py || true
  gpu_run bash -c "nohup \$HOME/venv/bin/python \$HOME/all-smi/scripts/dev/hollow_util.py ${mode} > /tmp/hollow_${mode}.log 2>&1 & echo \$!"
  sleep 3

  # Capture DCGM then all-smi (sequential; each ~30s window).
  gpu_run bash -c "dcgmi dmon -e ${DCGM_FIELDS} -d 1000 -c 30 > /tmp/dcgm_${mode}.txt 2>&1" \
    || echo "WARN: dcgmi dmon failed for ${mode}"
  gpu_run bash -c "cd \$HOME/all-smi && ./target/release/all-smi snapshot --format json --include gpu --samples 30 --interval 1 > /tmp/allsmi_${mode}.json 2>&1" \
    || echo "WARN: all-smi snapshot failed for ${mode}"

  gpu_run pkill -f hollow_util.py || true

  # Pull captures locally
  gpu_run cat "/tmp/dcgm_${mode}.txt" > "${OUT}/${mode}/dcgm_dmon.txt" || true
  gpu_run cat "/tmp/allsmi_${mode}.json" > "${OUT}/${mode}/allsmi_snapshot.json" || true
  gpu_run cat "/tmp/hollow_${mode}.log" > "${OUT}/${mode}/hollow_util.log" || true
done

echo "parity captures in ${OUT}"
