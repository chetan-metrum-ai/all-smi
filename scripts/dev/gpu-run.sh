#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# SSH wrapper: cd ~/all-smi && source cargo env && run command on the GPU box.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STATE="${ROOT}/.shadeform-state.json"
KEY="${ROOT}/.shadeform_ed25519"
KNOWN_HOSTS="${ROOT}/.shadeform_known_hosts"

gpu_run() {
  if [[ ! -f "${STATE}" ]]; then
    echo "missing ${STATE}" >&2
    return 1
  fi
  local IP USER PORT
  IP="$(python3 -c "import json; print(json.load(open('${STATE}'))['ip'])")"
  USER="$(python3 -c "import json; print(json.load(open('${STATE}')).get('ssh_user') or 'shadeform')")"
  PORT="$(python3 -c "import json; print(json.load(open('${STATE}')).get('ssh_port') or 22)")"
  # Pass the command as a single remote argv via bash -lc and printf %q.
  local remote_cmd
  remote_cmd=$(printf '%q ' "$@")
  ssh -i "${KEY}" -p "${PORT}" \
    -o StrictHostKeyChecking=accept-new \
    -o UserKnownHostsFile="${KNOWN_HOSTS}" \
    "${USER}@${IP}" \
    "bash -lc $(printf '%q' "cd ~/all-smi && source \"\$HOME/.cargo/env\" 2>/dev/null || true; ${remote_cmd}")"
}

if [[ "${1:-}" == "--source-only" ]]; then
  return 0 2>/dev/null || exit 0
fi

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <cmd> [args...]" >&2
  exit 1
fi

gpu_run "$@"
