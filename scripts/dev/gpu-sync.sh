#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Sync the local tree to the Shadeform instance (source only; no env.json).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STATE="${ROOT}/.shadeform-state.json"
KEY="${ROOT}/.shadeform_ed25519"
KNOWN_HOSTS="${ROOT}/.shadeform_known_hosts"

if [[ ! -f "${STATE}" ]]; then
  echo "missing ${STATE}; run: python3 scripts/dev/shadeform.py create" >&2
  exit 1
fi
if [[ ! -f "${KEY}" ]]; then
  echo "missing ${KEY}" >&2
  exit 1
fi

IP="$(python3 -c "import json; print(json.load(open('${STATE}'))['ip'])")"
USER="$(python3 -c "import json; print(json.load(open('${STATE}')).get('ssh_user') or 'shadeform')")"
PORT="$(python3 -c "import json; print(json.load(open('${STATE}')).get('ssh_port') or 22)")"

if [[ -z "${IP}" || "${IP}" == "None" ]]; then
  echo "state has no ip yet; run: python3 scripts/dev/shadeform.py status" >&2
  exit 1
fi

rsync -az --delete \
  -e "ssh -i ${KEY} -p ${PORT} -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=${KNOWN_HOSTS}" \
  --exclude target \
  --exclude .git \
  --exclude env.json \
  --exclude .shadeform-state.json \
  --exclude .shadeform_ed25519 \
  --exclude .shadeform_ed25519.pub \
  --exclude .shadeform_known_hosts \
  "${ROOT}/" "${USER}@${IP}:~/all-smi/"

echo "synced to ${USER}@${IP}:~/all-smi/"
