#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# One-time setup on the Shadeform GPU box: sync, then run remote-gpu-setup.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
"${ROOT}/scripts/dev/gpu-sync.sh"
chmod +x "${ROOT}/scripts/dev/remote-gpu-setup.sh"
"${ROOT}/scripts/dev/gpu-run.sh" bash scripts/dev/remote-gpu-setup.sh
