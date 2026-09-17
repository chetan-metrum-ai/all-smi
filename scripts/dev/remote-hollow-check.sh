#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Capture hollow-mode nvidia-smi util vs DCGM SM_ACTIVE on the GPU box.
set -euo pipefail
pkill -f hollow_util.py || true
nohup ~/venv/bin/python ~/all-smi/scripts/dev/hollow_util.py hollow > /tmp/hollow.log 2>&1 &
sleep 8
echo "=== nvidia-smi util ==="
nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader
echo "=== dcgmi dmon SM_ACTIVE (1002) x10 ==="
dcgmi dmon -e 1002 -d 1000 -c 10
pkill -f hollow_util.py || true
echo "=== hollow log ==="
head -5 /tmp/hollow.log || true
