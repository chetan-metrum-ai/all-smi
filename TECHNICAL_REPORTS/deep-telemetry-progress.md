<!-- Copyright (c) 2026 Metrum AI, Inc. All rights reserved. -->

# Deep-telemetry progress

Running log for the Metrum `metrum/deep-telemetry` work. Updated at the end of every phase.

## P0 — bootstrap

### Status

Complete (acceptance on Shadeform H100). Instance left running until human says to delete.

### Shadeform instance

| Field | Value |
|-------|-------|
| name | metrum-allsmi-dev |
| gpu_type | H100 |
| shade_instance_type | H100 |
| cloud / region | massedcompute / desmoines-usa-1 |
| os | ubuntu22.04_cuda13.0_shade_os |
| hourly_price | 273 cents/hour ($2.73/hr) |
| created_at | 2026-09-17T02:08:11Z |
| id / ip | see `.shadeform-state.json` (gitignored) |

### Software versions (GPU box)

From `TECHNICAL_REPORTS/runs/2026-09-17T022543Z/`:

- NVIDIA driver: 580.126.09 (CUDA Version reported by nvidia-smi: 13.0)
- DCGM / dcgmi: 4.7.0 (`datacenter-gpu-manager-4-cuda13` + `nvidia-dcgm.service` active)
- rustc: 1.98.1 (48a229cea 2026-09-01)
- torch: 2.14.0+cu130 (venv `~/venv`)

### Hollow-mode manual check

Workload: `scripts/dev/hollow_util.py hollow` (batched 32×32 fp32 `torch.mm`).

| Metric | Value |
|--------|-------|
| nvidia-smi GPU-Util | ~27% (not near 100% on this H100 with tiny kernels; still ≫ SM_ACTIVE) |
| DCGM SM_ACTIVE (1002) mean | ~0.001 (N/A on first sample, then 0.001 × 9) |
| notes | Qualitative hollow signal holds: board util ≫ SM_ACTIVE. Raising nvidia-smi util toward 100% without raising SM_ACTIVE may need a 1-SM spin kernel in a later tweak; not blocking P0. |

### Doctor checks added

- `nvidia.gpm.supported` — **pass** (GPM on H100)
- `nvidia.dcgm.library` — **pass**
- `nvidia.dcgm.hostengine` — **pass**
- `nvidia.dcgm.prof_module` — **pass** (`dcgmi dmon -e 1002`)

Policy: SKIP when no NVIDIA GPU; DCGM absence is WARN (never FAIL). Filter with `--only nvidia.dcgm` / `nvidia.gpm`.

### Release machinery

- Workflow: `.github/workflows/metrum-release.yml` (GitHub-hosted only).
- First public tag: after P4 only (`v0.26.3-metrum.N`).
- Nucbox is not a compile host; after P4, download the linux-x86_64 tarball there for a no-GPU smoke.

### Run directories

- `TECHNICAL_REPORTS/runs/2026-09-17T022543Z/` — doctor.json, doctor-nvidia-deep.txt, hollow-check.txt, versions.txt
- Full `cargo test --release` green on the H100 box (including `tests/gpm_parity.rs` under `ALL_SMI_LIVE_GPU=1`, which skips numeric asserts until P1).
- `cargo clippy --all-targets --all-features -- -D warnings` clean on the H100 box.

### Open questions

- Hollow harness: how to push nvidia-smi GPU-Util near 100% on H100 while keeping SM_ACTIVE ≤ 0.02 (possible follow-up before P1 parity tightening).

## P1 — real GPM (two-sample) + full field set

### Status

Complete on Shadeform H100 (merged via PR #1).

### Changes

- `src/device/readers/nvidia_gpm.rs`: two-sample collector with per-UUID handle cache; `ALL_SMI_NVIDIA_DISABLE_GPM`; PCIe/NVLink rates scaled MB/s → bytes/s
- Extended `GpmMetrics` + `TelemetrySource`; Prometheus family with `source=` label
- Surfaces: exporter, metrics parser, mock, TUI GPM row, filter DSL, `telemetry_surface_completeness`, `API.md`
- First poll returns `None`; subsequent polls populate ratios/rates (never invent zeros)

### Live hollow check (2026-09-17T025156Z)

| Metric | all-smi GPM | DCGM |
|--------|-------------|------|
| sm_active / SMACT (1002) | mean 0.00104 | mean 0.001 |
| graphics_active / GRACT | mean 0.272 | ~0.31 |
| nvidia-smi util | ~31% | — |

`gpm_parity` live test: **pass** (±0.05).

### Acceptance

- [x] `gpm_parity` ±0.05 vs DCGM (hollow)
- [x] `telemetry_surface_completeness` green
- [x] first poll `None`, later polls populated on H100
- [x] PR #1 merged into fork `main`

## P2 — NVML extras

### Status

Complete on Shadeform H100 (this PR).

### Changes

- `nvidia_extras.rs`: throttle reasons, PCIe fallback (`source=nvml`), energy HW counter, remapped rows, NVLink errors, utilization sample summaries, process util
- `nvidia_xid.rs`: best-effort XID/ECC watcher thread → `xid_event_counts`
- Surfaces: Prometheus exporter, metrics parser, filter DSL (`throttle`/`throttled`), TUI throttle tags, Users-tab SM-share power weighting (`POWER*sm`), `API.md`, completeness test

### Acceptance

- [x] Unit tests (`nvidia_extras`) + `telemetry_surface_completeness`
- [x] Live H100 scrape exposes energy/GPM/PCIe/remapped/util samples; `throttle_reason{gpu_idle}` (power-cap change requires root; not available on Shadeform user)
- [ ] PR merged

### Run dir

- `TECHNICAL_REPORTS/runs/p2-live/`
