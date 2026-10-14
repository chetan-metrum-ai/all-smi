#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Runs ON the Shadeform GPU box (via gpu-run.sh) after a release build.
# Metrum AI release smoke: doctor GPM/DCGM/AMD, AMD plugin tests, and live
# /metrics scrapes for the default GPM source and the DCGM fallback.
set -uo pipefail

BIN=./target/release/all-smi
DCGM_SO="${PWD}/target/release/liball_smi_dcgm.so"

echo "=== version ==="
"${BIN}" --version
nvidia-smi --query-gpu=name,driver_version --format=csv,noheader
dcgmi --version 2>/dev/null | head -3 || true

echo "=== doctor --only nvidia.gpm,nvidia.dcgm ==="
ALL_SMI_DCGM_PLUGIN="${DCGM_SO}" "${BIN}" doctor --only nvidia.gpm,nvidia.dcgm

echo "=== doctor --only amd ==="
"${BIN}" doctor --only amd || true

echo "=== cargo test -p all-smi-amd-plugin --release ==="
cargo test -p all-smi-amd-plugin --release 2>&1 | grep -E "^test result|FAILED"

scrape() {
  local port="$1" label="$2"; shift 2
  env "$@" "${BIN}" api --port "${port}" --interval 1 >"/tmp/allsmi-${label}.log" 2>&1 &
  local pid=$!
  for _ in $(seq 1 60); do
    curl -fs --max-time 5 "http://127.0.0.1:${port}/metrics" 2>/dev/null | grep -qE '^all_smi_up(\{[^}]*\})? 1' && break
    sleep 1
  done
  sleep 3
  curl -fs --max-time 5 "http://127.0.0.1:${port}/metrics" >"/tmp/allsmi-${label}.prom"
  kill "${pid}" 2>/dev/null; wait "${pid}" 2>/dev/null
  echo "--- ${label}: all_smi_up / source label counts / power"
  grep -E '^all_smi_up[{ ]' "/tmp/allsmi-${label}.prom"
  grep -oE 'source="[a-z]+"' "/tmp/allsmi-${label}.prom" | sort | uniq -c
  grep -E '^all_smi_gpu_power_consumption_watts' "/tmp/allsmi-${label}.prom" | head -2
}

echo "=== live scrapes ==="
scrape 9101 default ALL_SMI_DCGM_PLUGIN="${DCGM_SO}"
scrape 9102 dcgm-fallback ALL_SMI_NVIDIA_DISABLE_GPM=1 ALL_SMI_DCGM_PLUGIN="${DCGM_SO}"

echo "SMOKE_DONE"
