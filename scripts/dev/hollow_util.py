#!/usr/bin/env python3
# Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
"""Ground-truth GPU workload generator for GPM / DCGM parity tests.

Modes (argv[1]):
  hollow  — tiny matmul occupying one SM; nvidia-smi util high, SM_ACTIVE low
  tensor  — large fp16 matmul; tensor pipe / SM_ACTIVE high
  dram    — device-to-device copy of two 2 GiB tensors; DRAM util high
  pcie    — pinned host-to-device transfers; PCIe bytes high

Loops forever until killed. End sessions with: pkill -f hollow_util.py
"""

from __future__ import annotations

import sys
import time


def require_torch():
    try:
        import torch
    except ImportError as e:
        sys.exit(f"torch required: {e}")
    if not torch.cuda.is_available():
        sys.exit("CUDA device required")
    return torch


def mode_hollow(torch):
    # 32x32 fp32 matmul — enough work to keep the GPU "busy" for nvidia-smi
    # while leaving SM_ACTIVE / occupancy near zero (hollow utilization).
    # Queue many tiny kernels before synchronizing so nvidia-smi's sampling
    # window sees sustained work; a single sync-per-kernel is too sparse on H100.
    a = torch.randn(32, 32, device="cuda", dtype=torch.float32)
    b = torch.randn(32, 32, device="cuda", dtype=torch.float32)
    print("hollow: looping 32x32 fp32 mm (batched submissions)", flush=True)
    while True:
        for _ in range(4096):
            c = torch.mm(a, b)
            a = c
        torch.cuda.synchronize()


def mode_tensor(torch):
    a = torch.randn(8192, 8192, device="cuda", dtype=torch.float16)
    b = torch.randn(8192, 8192, device="cuda", dtype=torch.float16)
    print("tensor: looping 8192x8192 fp16 mm", flush=True)
    while True:
        c = torch.mm(a, b)
        torch.cuda.synchronize()
        a = c


def mode_dram(torch):
    # ~2 GiB each at fp32
    n = (2 * 1024 * 1024 * 1024) // 4
    x = torch.empty(n, device="cuda", dtype=torch.float32)
    y = torch.empty(n, device="cuda", dtype=torch.float32)
    print(f"dram: looping copy_ of {n} fp32 elements (~2 GiB)", flush=True)
    while True:
        x.copy_(y)
        y.copy_(x)
        torch.cuda.synchronize()


def mode_pcie(torch):
    nbytes = 256 * 1024 * 1024
    n = nbytes // 4
    print(f"pcie: looping pinned H2D of {nbytes} bytes", flush=True)
    while True:
        host = torch.randn(n, pin_memory=True, dtype=torch.float32)
        _ = host.cuda(non_blocking=False)
        torch.cuda.synchronize()
        del host


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in {"hollow", "tensor", "dram", "pcie"}:
        print(f"usage: {sys.argv[0]} hollow|tensor|dram|pcie", file=sys.stderr)
        sys.exit(2)
    mode = sys.argv[1]
    torch = require_torch()
    torch.cuda.set_device(0)
    print(f"device={torch.cuda.get_device_name(0)} mode={mode}", flush=True)
    {
        "hollow": mode_hollow,
        "tensor": mode_tensor,
        "dram": mode_dram,
        "pcie": mode_pcie,
    }[mode](torch)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("stopped", flush=True)
        time.sleep(0)
