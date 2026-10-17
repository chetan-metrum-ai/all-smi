#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Run the manual GitHub Actions Shadeform validation; keep raw logs private.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RAW_DIR="${1:?usage: $0 <private-raw-log-dir>}"
MAX_HOURLY_PRICE_USD="3.00"
ALLOW_CLEANUP=0
mkdir -p "${RAW_DIR}"
chmod 700 "${RAW_DIR}"

run_logged() {
  local name="$1" rc=0
  shift
  printf 'running %s\n' "${name}" >>"${RAW_DIR}/runner.raw"
  if "$@" >"${RAW_DIR}/${name}.raw" 2>&1; then
    printf '0\n' >"${RAW_DIR}/${name}.exit"
  else
    rc=$?
    printf '%s\n' "${rc}" >"${RAW_DIR}/${name}.exit"
    printf 'failed %s (exit %s)\n' "${name}" "${rc}" >>"${RAW_DIR}/runner.raw"
    return "${rc}"
  fi
}

cleanup() {
  local rc=$? cleanup_rc=0
  trap - EXIT INT TERM
  if [[ ${ALLOW_CLEANUP} -eq 1 && -f "${ROOT}/.shadeform-state.json" ]]; then
    if python3 "${ROOT}/scripts/dev/shadeform.py" delete --yes-i-am-sure \
      >"${RAW_DIR}/delete.raw" 2>&1; then
      :
    else
      cleanup_rc=$?
    fi
  else
    printf 'no Shadeform instance state remained to delete\n' >"${RAW_DIR}/delete.raw"
  fi
  if [[ -f "${ROOT}/.shadeform-state.json" ]]; then
    printf 'Shadeform state remained after deletion attempt\n' >>"${RAW_DIR}/delete.raw"
    cleanup_rc=1
  else
    if ! python3 "${ROOT}/scripts/dev/shadeform.py" status \
      >"${RAW_DIR}/delete-status.raw" 2>&1; then
      cleanup_rc=1
    fi
  fi
  if [[ ${cleanup_rc} -ne 0 ]]; then
    printf 'Shadeform cleanup failed\n' >>"${RAW_DIR}/runner.raw"
    rc=1
  fi
  exit "${rc}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -z "${SHADEFORM_API_KEY:-}" ]]; then
  printf 'missing required GitHub Actions secret SHADEFORM_API_KEY\n' >"${RAW_DIR}/preflight.raw"
  exit 1
fi
if [[ ! -s "${ROOT}/.shadeform_ed25519" || ! -s "${ROOT}/.shadeform_ed25519.pub" ]]; then
  printf 'Shadeform SSH key files are missing\n' >"${RAW_DIR}/preflight.raw"
  exit 1
fi
if [[ -e "${ROOT}/.shadeform-state.json" ]]; then
  printf 'refusing to use a pre-existing Shadeform instance state\n' >"${RAW_DIR}/preflight.raw"
  exit 1
fi

ALLOW_CLEANUP=1
run_logged shadeform-create python3 "${ROOT}/scripts/dev/shadeform.py" create \
  --max-hourly-price-usd "${MAX_HOURLY_PRICE_USD}"

run_logged gpu-setup bash "${ROOT}/scripts/dev/gpu-setup.sh"

RUN_MARKER="${RAW_DIR}/gpu-test-start.marker"
touch "${RUN_MARKER}"
gpu_test_rc=0
if run_logged gpu-test bash "${ROOT}/scripts/dev/gpu-test.sh"; then
  gpu_test_rc=0
else
  gpu_test_rc=$?
fi
# gpu-test.sh writes its detailed run artifacts under TECHNICAL_REPORTS/runs.
# Copy only the newly generated timestamped directory into the private log tree;
# the later redaction step decides what is safe to upload.
while IFS= read -r run_dir; do
  [[ -d "${run_dir}" ]] || continue
  relative="${run_dir#${ROOT}/}"
  mkdir -p "${RAW_DIR}/gpu-test-artifacts/$(dirname "${relative}")"
  cp -a "${run_dir}" "${RAW_DIR}/gpu-test-artifacts/${relative}"
done < <(find "${ROOT}/TECHNICAL_REPORTS/runs" -mindepth 1 -maxdepth 1 -type d \
  -newer "${RUN_MARKER}" -print 2>/dev/null)
if [[ ${gpu_test_rc} -ne 0 ]]; then
  exit "${gpu_test_rc}"
fi

run_logged workspace-build bash "${ROOT}/scripts/dev/gpu-run.sh" \
  cargo build --release --workspace
run_logged library-api-10x bash "${ROOT}/scripts/dev/gpu-run.sh" bash -lc \
  'set -e; for i in $(seq 1 10); do echo "=== library_api_test repeat ${i}/10 ==="; cargo test --release --test library_api_test; done'
run_logged remote-release-smoke bash "${ROOT}/scripts/dev/gpu-run.sh" \
  bash scripts/dev/remote-release-smoke.sh
run_logged dmesg-safeguard bash "${ROOT}/scripts/dev/gpu-run.sh" bash -lc \
  'set -euo pipefail; timeout --signal=TERM --kill-after=5s 60s nvidia-smi -L; sudo -n dmesg --ctime > /tmp/all-smi-shadeform-dmesg.log; grep -Ei "segfault|NVRM|Xid|RCU stall|BUG:" /tmp/all-smi-shadeform-dmesg.log | tail -n 200 || true; if grep -Eiq "segfault|NVRM.*(assert|BUG)|RCU.*stall|BUG:" /tmp/all-smi-shadeform-dmesg.log; then exit 1; fi; echo "dmesg safeguard passed"'

printf 'Shadeform GPU validation passed\n' >>"${RAW_DIR}/runner.raw"
