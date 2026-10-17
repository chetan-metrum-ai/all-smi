#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Exercise DCGM session invalidation, retry backoff, and recovery on a GPU box.
set -euo pipefail

BIN=./target/release/all-smi
DCGM_SO="${PWD}/target/release/liball_smi_dcgm.so"
SERVICE=nvidia-dcgm.service
PORT=9193
API_LOG=/tmp/all-smi-dcgm-backoff-api.log
POLL_LOG=/tmp/all-smi-dcgm-backoff-polls.log
METRICS=/tmp/all-smi-dcgm-backoff-metrics.prom
API_PID=""

cleanup() {
  local status=$?
  trap - EXIT
  if [[ -n "${API_PID}" ]]; then
    kill -TERM "${API_PID}" 2>/dev/null || true
    wait "${API_PID}" 2>/dev/null || true
  fi
  sudo systemctl start "${SERVICE}" >/dev/null 2>&1 || true
  exit "${status}"
}
trap cleanup EXIT

sudo systemctl start "${SERVICE}"
if ! systemctl is-active --quiet "${SERVICE}"; then
  echo "ERROR: ${SERVICE} did not start" >&2
  exit 1
fi

ALL_SMI_DCGM_PLUGIN="${DCGM_SO}" \
ALL_SMI_NVIDIA_DISABLE_GPM=1 \
RUST_LOG=all_smi::device::readers::dcgm=debug \
  "${BIN}" api --port "${PORT}" --interval 1 >"${API_LOG}" 2>&1 &
API_PID=$!
: >"${POLL_LOG}"

# Wait for an initial live DCGM sample before restarting the service.
ready_poll=0
for poll in $(seq 1 60); do
  if curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/metrics" -o "${METRICS}" \
      && grep -q 'source="dcgm"' "${METRICS}"; then
    ready_poll="${poll}"
    break
  fi
  sleep 1
done
if (( ready_poll == 0 )); then
  echo "ERROR: API did not produce initial source=dcgm metrics" >&2
  exit 1
fi
printf 'hostengine-initial source=dcgm ready_poll=%s\n' "${ready_poll}"

# Keep the hostengine down for about 30 seconds while collecting each tick.
sudo systemctl stop "${SERVICE}"
if systemctl is-active --quiet "${SERVICE}"; then
  echo "ERROR: ${SERVICE} is still active after stop" >&2
  exit 1
fi
for poll in $(seq 1 30); do
  curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/metrics" -o "${METRICS}"
  dcgm_series=$(grep -c 'source="dcgm"' "${METRICS}" || true)
  printf '%s poll=%s dcgm_series=%s\n' "$(date -u +%FT%TZ)" "${poll}" "${dcgm_series}" >>"${POLL_LOG}"
  sleep 1
done

attempts=$(grep -c 'attempting DCGM hostengine session' "${API_LOG}" || true)
skips=$(grep -c 'skipping DCGM session create during backoff' "${API_LOG}" || true)
discarded=$(grep -c 'DCGM sample failed; discarded session and will reconnect' "${API_LOG}" || true)
if (( discarded < 1 || attempts < 2 || attempts >= 20 || skips < 10 )); then
  echo "ERROR: unexpected reconnect/backoff counts: discarded=${discarded} attempts=${attempts} skips=${skips}" >&2
  exit 1
fi
if grep -q 'source="dcgm"' "${METRICS}"; then
  echo "ERROR: DCGM metrics unexpectedly present while hostengine was stopped" >&2
  exit 1
fi
printf 'hostengine-down polls=30 discarded_sessions=%s create_attempts=%s backoff_skips=%s\n' \
  "${discarded}" "${attempts}" "${skips}"

sudo systemctl start "${SERVICE}"
found_after=0
for poll in $(seq 1 60); do
  curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/metrics" -o "${METRICS}"
  dcgm_series=$(grep -c 'source="dcgm"' "${METRICS}" || true)
  printf '%s recovery_poll=%s dcgm_series=%s\n' "$(date -u +%FT%TZ)" "${poll}" "${dcgm_series}" >>"${POLL_LOG}"
  if (( dcgm_series > 0 )); then
    found_after=${poll}
    break
  fi
  sleep 1
done
if (( found_after == 0 )); then
  echo "ERROR: DCGM source did not recover within 60 seconds" >&2
  exit 1
fi

reset=$(grep -c 'DCGM sample succeeded; retry backoff reset' "${API_LOG}" || true)
if (( reset == 0 )); then
  echo "ERROR: successful recovery sample did not reset retry backoff" >&2
  exit 1
fi
printf 'hostengine-recovered source=dcgm recovery_poll=%s create_attempts=%s backoff_skips=%s backoff_resets=%s\n' \
  "${found_after}" "$(grep -c 'attempting DCGM hostengine session' "${API_LOG}" || true)" \
  "$(grep -c 'skipping DCGM session create during backoff' "${API_LOG}" || true)" "${reset}"

kill -TERM "${API_PID}" 2>/dev/null || true
wait "${API_PID}" 2>/dev/null || true
API_PID=""
