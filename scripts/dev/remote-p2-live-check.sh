#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Live P2 metrics smoke on the GPU box.
set -euo pipefail
cd ~/all-smi
source "$HOME/.cargo/env" 2>/dev/null || true

if [[ ! -x target/release/all-smi ]]; then
  cargo build --features cli --release -q
fi

PL="$(nvidia-smi --query-gpu=power.limit --format=csv,noheader,nounits | head -1 | tr -d ' ')"
echo "baseline_pl=${PL}"

pkill -f 'target/release/all-smi api' 2>/dev/null || true
sleep 1
./target/release/all-smi api --port 9101 --interval 1 >/tmp/allsmi-p2-api.log 2>&1 &
API_PID=$!
trap 'kill '"$API_PID"' 2>/dev/null || true; nvidia-smi -i 0 -pl '"$PL"' 2>/dev/null || true' EXIT

for _ in $(seq 1 15); do
  if curl -sf localhost:9101/metrics >/dev/null; then
    break
  fi
  sleep 1
done

nvidia-smi -i 0 -pl 100 || true
sleep 3
curl -sf localhost:9101/metrics >/tmp/p2-metrics-throttle.txt
nvidia-smi -i 0 -pl "${PL}" || nvidia-smi -i 0 -pl 350 || true

echo "=== P2 metric hits ==="
grep -E c 'all_smi_gpu_throttle_reason|all_smi_gpu_energy_hw|all_smi_gpu_remapped|all_smi_gpu_nvlink_errors|all_smi_gpu_utilization_sample|all_smi_gpu_xid_events|all_smi_gpu_sm_active|all_smi_gpu_pcie' /tmp/p2-metrics-throttle.txt || true
echo "=== sample lines ==="
grep -E 'throttle_reason|energy_hw_millijoules|remapped_rows|utilization_sample|sm_active_ratio|pcie_tx|source=' /tmp/p2-metrics-throttle.txt | head -40
echo "=== restored pl ==="
nvidia-smi --query-gpu=power.limit --format=csv,noheader

# Persist a run artifact locally on the box; agent will scp later if needed
mkdir -p TECHNICAL_REPORTS/runs/p2-live
cp /tmp/p2-metrics-throttle.txt TECHNICAL_REPORTS/runs/p2-live/metrics-throttle.txt
echo ok > TECHNICAL_REPORTS/runs/p2-live/STATUS
