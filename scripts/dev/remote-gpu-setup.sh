#!/usr/bin/env bash
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
# Runs ON the GPU box (invoked via gpu-run.sh after gpu-sync.sh).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config libssl-dev protobuf-compiler \
  libdrm-dev libdrm-amdgpu1 clang libclang-dev \
  curl ca-certificates git python3 python3-venv python3-pip \
  rsync jq wget

if ! command -v rustc >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck source=/dev/null
source "${HOME}/.cargo/env"
rustup default stable
rustup update stable

# NVIDIA CUDA keyring + DCGM (best-effort; image may already have CUDA).
if ! dpkg -l | grep -qE 'datacenter-gpu-manager|nv-hostengine'; then
  if [[ ! -f /usr/share/keyrings/cuda-archive-keyring.gpg ]]; then
    wget -q https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2204/x86_64/cuda-keyring_1.1-1_all.deb -O /tmp/cuda-keyring.deb
    sudo dpkg -i /tmp/cuda-keyring.deb || true
    sudo apt-get update || true
  fi
fi
# Prefer CUDA-major matching the image (this Shadeform OS is CUDA 13); fall back.
sudo apt-get install -y datacenter-gpu-manager-4-cuda13 \
  || sudo apt-get install -y datacenter-gpu-manager-4-cuda12 \
  || sudo apt-get install -y datacenter-gpu-manager \
  || echo "WARN: could not install datacenter-gpu-manager via apt"

if systemctl list-unit-files | grep -q nvidia-dcgm; then
  sudo systemctl enable --now nvidia-dcgm
elif systemctl list-unit-files | grep -q '^dcgm'; then
  sudo systemctl enable --now dcgm || true
fi

if [[ ! -d "${HOME}/venv" ]]; then
  python3 -m venv "${HOME}/venv"
fi
# shellcheck source=/dev/null
source "${HOME}/venv/bin/activate"
pip install --upgrade pip
pip install torch --index-url https://download.pytorch.org/whl/cu124

echo "=== verification ==="
nvidia-smi || { echo "nvidia-smi failed"; exit 1; }
command -v dcgmi && dcgmi discovery -l || echo "WARN: dcgmi discovery failed"
command -v dcgmi && dcgmi dmon -e 1002 -c 1 || echo "WARN: dcgmi dmon failed"
rustc --version
python3 -c "import torch; print('torch', torch.__version__, 'cuda', torch.cuda.is_available())"

echo "SETUP_OK"
