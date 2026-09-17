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
- [x] PR #2 merged into fork `main`

### Run dir

- `TECHNICAL_REPORTS/runs/p2-live/`

## P3 — derived hollow util + alert rules

### Status

Complete (merged via PR #3).

### Changes

- Derived `hollow_utilization` (`max(0, graphics_active − sm_active)`); Prometheus `all_smi_gpu_hollow_utilization_ratio`; filter DSL `hollow`/`hollow_util`; TUI red when > 0.2
- Alert rules: hollow util (sustain), no_tensor, memory_bound, remap_pending, xid, throttle_sustained — config/schema/env/example/render + webhook `reason`/`xid`
- Mock: `ALL_SMI_MOCK_HOLLOW=1` (+ hardware details) forces high board util / low SM for hollow acceptance
- Docs: `API.md`, README Filtering & Alerts, help text

### Acceptance

- [x] Unit/integration tests + clippy on H100
- [x] Mock hollow smoke (`all-smi-mock-server` with hollow env)
- [x] PR #3 merged into fork `main`

## P4 — DCGM plugin

### Status

Complete (merged via PR #4).

### Changes

- Workspace crate `crates/all-smi-dcgm-plugin` (`liball_smi_dcgm.so`); ABI + loader mirroring AMD; vendored bindgen bindings
- Plugin dlopens `libdcgm.so.4`, connects to hostengine (embedded fallback), watches PROF_*/XID/remap/throttle
- `nvidia.rs` merge: fill only `None` GPM fields from DCGM; `Source: <field>=dcgm`
- Release workflow builds/packages the DCGM `.so` on Linux glibc

### Acceptance

- [x] With `ALL_SMI_NVIDIA_DISABLE_GPM=1`, DCGM fills `sm_active`; with GPM on, `source=gpm`
- [x] Loader unavailable path does not panic
- [x] PR #4 merged into fork `main`

## Release v0.26.3-metrum.2

Tagged on fork `main` after P3+P4.

### Release smoke (Shadeform H100)

Published `all-smi-linux-x86_64.tar.gz` unpacked on the live box (`~/all-smi-release-metrum.2/`):

- sha256 OK; tree includes `all-smi`, `liball_smi_dcgm.so`, `liball_smi_amd.so`
- `doctor --only nvidia.gpm,nvidia.dcgm`: 4 PASS
- Default API scrape: `sm_active` / hollow with `source="gpm"`
- `ALL_SMI_NVIDIA_DISABLE_GPM=1` + `ALL_SMI_DCGM_PLUGIN=…/liball_smi_dcgm.so`: PROF gauges with `source="dcgm"`

Artifacts: `TECHNICAL_REPORTS/runs/metrum.2-release-smoke/`

### Nucbox smoke (gengar, no GPU)

- Purged apt package `all-smi 0.23.0-1~noble1` so PATH no longer shadowed the fork binary
- Unpacked published `v0.26.3-metrum.2` linux-x86_64 tarball into `~/all-smi-release-metrum.2/`
- `./all-smi --version` → `0.26.3`; doctor nvidia.gpm/dcgm → 4 SKIP; help/config print OK

Artifacts: `TECHNICAL_REPORTS/runs/metrum.2-nucbox-smoke/`

### Shadeform teardown

Instance `fea307ef-c176-4343-8965-493f0db1a17a` deleted via `scripts/dev/shadeform.py delete --yes-i-am-sure`; local `.shadeform-state.json` cleared.
