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
- Plugin dlopens `libdcgm.so.4`, connects to the external hostengine, watches PROF_*/XID/remap/throttle. Embedded-hostengine startup was removed after GPU stress exposed unsafe concurrent in-process NVML workers; when no hostengine is reachable, DCGM enrichment degrades cleanly.
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

## CI fix + new Shadeform (RTXPro6000)

- Root cause: P4 added no-op cargo feature `dcgm` but `enabled_features()` in `src/doctor/bundle.rs` had no arm → `bundle_covers_every_declared_feature` failed on GHA.
- Fix: record `dcgm` in `enabled_features()` + build-config test; `cargo fmt` for `fmt --check`.
- `scripts/dev/shadeform.py` preference chain: RTXPro6000 → H200 → H100 → L40S (still refuse bare B200).
- New instance: RTX PRO 6000 Blackwell (`RTXPro6000`, massedcompute).

### Blackwell release smoke (RTXPro6000)

Published `v0.26.3-metrum.2` linux-x86_64 tarball on the live box:

- sha256 OK; doctor nvidia.gpm/dcgm → 4 PASS
- Default API: `source="gpm"`; with GPM disabled + DCGM plugin: `source="dcgm"`
- `library_api_test` 25/25 serial and parallel (earlier SIGSEGV not reproduced)

Artifacts: `TECHNICAL_REPORTS/runs/metrum.2-blackwell-smoke/`

### Nucbox PATH (gengar)

- Wrapper at `~/.local/bin/all-smi` → `~/all-smi-release-metrum.2/` (sets `LD_LIBRARY_PATH` + default `ALL_SMI_DCGM_PLUGIN`); not installed to `/usr/bin`
- `all-smi --version` → `0.26.3`; doctor nvidia.* → 4 SKIP on no-GPU

### Shadeform teardown (RTXPro6000)

Instance `49b9fabb-50d1-48dd-ae6e-a25cb017e2da` deleted via `scripts/dev/shadeform.py delete --yes-i-am-sure`; local `.shadeform-state.json` cleared.

Upstream lablup PR deferred until further testing.

## Release v0.26.3-metrum.3

Tagged on fork `main` after PR #6 (issue #5).

### Changes since metrum.2

- GPM doctor two-sample probe: no longer trusts NVML `gpm_support()` alone; WARN when the flag is a false positive
- First GPM collector poll self-primes so `snapshot --samples 1` can populate `source=gpm`
- `enabled_features()` records the no-op `dcgm` cargo feature (support-bundle / CI coverage)

Crate version remains `0.26.3`; tag is `v0.26.3-metrum.3`.

## Release v0.26.3-metrum.4

Tagged on fork `main` after merging `lablup/main` through `57091b7` (PR #445 closed; sync was fork-only).

### Changes since metrum.3

- Sync upstream: AWS Neuron device support (#417), Rebellions load/power fixes (#416, #423), Tenstorrent vendor metrics re-key (#439)
- Sync upstream: volatile detail keys / dedicated gauges (#435, #441, #442), Linux process CPU sampler (#443), macOS `proc_pidinfo` sampling and collection-cost work (#426, #432)
- Sync upstream: drop dead generic `all_smi_npu_*` metrics (#440), Prometheus parse hardening (#438), macOS CI clippy gate (#444)
- Merge fix: initialize fork deep-telemetry `GpuInfo` fields on new upstream Neuron/NPU/test constructors

Crate version remains `0.26.3`; tag is `v0.26.3-metrum.4`.

## Release v0.27.0-metrum.1

Tagged on fork `main` after merging `lablup/main` through `9271e22` (upstream tag `v0.27.0` is `71087e8`; four later commits are included).

### Changes since metrum.4

- Sync upstream v0.27.0: AWS Neuron device support, macOS and Linux per-tick collection cost, Apple Silicon and Rebellions power fixes, Prometheus label stability, Tenstorrent vendor gauges
- Sync upstream v0.27.0: AMD GPU power reported in watts (MI300X hwmon and gpu_metrics table scaling), `debug-pid-mapping` kept out of a default `cargo install`
- Sync commits after the upstream tag: cargo package bumps, Launchpad source rules build the AMD companion, Homebrew install drops the tap step, README split into task guides with `CHANGELOG.md`
- Fork: DCGM plugin crate version moves from 0.26.3 to 0.27.0 with the main binary; Metrum install notes stay in the shortened README and `docs/installation.md`

Crate version is `0.27.0`; tag is `v0.27.0-metrum.1`.

### Pre-release smoke (Shadeform H100)

RTX PRO 6000 was not offered, so the box is a Lambda Labs H100 PCIe (`209.20.159.188`, driver 570.148.08, DCGM 4.7.0). Instance `d387a6fe-6ec8-423d-b5ca-be7f467b0aa4`.

- Local `cargo test -p all-smi`: lib 1870 passed (2 sudo tests skipped; this host has no `sudo` binary) and the integration/doc targets passed
- Release build on the H100: `all-smi 0.27.0`, `liball_smi_amd.so`, `liball_smi_dcgm.so`
- `doctor --only nvidia.gpm,nvidia.dcgm`: 4 pass
- `doctor --only amd`: plugin 0.27.0 loaded; 3 pass, 5 skip (no AMD GPU / ROCm)
- `cargo test -p all-smi-amd-plugin --release`: 12 passed, including the watts scaling tests
- Default API scrape after `all_smi_up 1`: `source="gpm"` on the profile ratios; power read 51.364 W idle
- `ALL_SMI_NVIDIA_DISABLE_GPM=1` plus the DCGM plugin: `source="dcgm"` on the same ratios

Published `v0.27.0-metrum.1` `all-smi-linux-x86_64.tar.gz` (sha256 OK) on the same box: `all-smi 0.27.0`, archive contains `liball_smi_amd.so` and `liball_smi_dcgm.so`, doctor GPM/DCGM 4 pass. Instance deleted after that check.

### AMD power check (DigitalOcean MI325X)

Single MI325X in `tor1` (`gpu-mi325x1-256gb`, image `gpu-amd-base`, droplet `607320438`). The published glibc tarball was copied to `/opt/all-smi-rel` because the loader refuses a plugin under world-writable `/tmp`.

Same-moment readings: hwmon `power1_input` 133000000 µW, `rocm-smi` 134.0 W, `all_smi_gpu_power_consumption_watts` 133. Device id `0x74b9`, ASIC `GFX940/MI300`, SKU `M3250101`. `doctor --only amd` loaded plugin 0.27.0. `amd.rocm.version` still skipped. Droplet deleted after the check.

## Proposed sync v0.27.1-metrum.2 (not tagged)

Sync upstream main at `4319b4a` (`fix: ignore frozen Xe GT idle counters in utilization fallback (#458)`), after v0.27.1. The change ignores frozen Intel Xe GT idle counters in the utilization fallback; no issue #11-#16 overlap was identified. Plugin crate version already matches main (`0.27.1`). Release tag is intentionally deferred for the human's decision.

### Shadeform validation

RTX PRO 6000 Blackwell Server Edition (Shadeform `RTXPro6000`), driver 580.126.09, CUDA 13.0, DCGM 4.7.0, rustc 1.99.1. Instance `046b41b0-e8f4-4e13-bfcd-f5a1caff46ad` at $2.19/hour was created at 03:24:20 UTC and deleted at 03:40 UTC; estimated cost about $0.58. Shadeform status confirmed no local instance after deletion. Redacted logs are in `TECHNICAL_REPORTS/runs/2026-10-17T032846Z/` and `TECHNICAL_REPORTS/runs/sync-upstream-2026-10-17/`; those run directories contain no IP literals.

- `cargo build --release --workspace`: passed, including `liball_smi_amd.so` and `liball_smi_dcgm.so`.
- `cargo test --release -- --test-threads=1`: all workspace/default-member suites passed; main crate integration tests include 2,090 passed, 0 failed, 2 ignored.
- Parallel `cargo test --release --test library_api_test`: 10/10 runs passed, each 26 passed / 0 failed.
- `all-smi doctor --only nvidia.gpm,nvidia.dcgm`: 4 pass, 0 warnings/failures/skips.
- `remote-release-smoke.sh`: passed; AMD plugin tests 12 passed; live `/metrics` showed `source="gpm"` by default and `source="dcgm"` with GPM disabled.
- dmesg check: no NVRM assert/Xid or RCU stall. It did report an `nvidia-nvswitch` kernel/user version mismatch (kernel 580.126.09 vs user 590.48.01); no related crash or stall occurred.

The Shadeform instance was deleted after validation, before waiting on PR CI; status confirmed the state file is clear.

## Release v0.27.1-metrum.1

Tagged on fork `main` after merging `lablup/main` through `ddfe94c` (upstream tag `v0.27.1`). Merged by Metrum AI.

### Changes since v0.27.0-metrum.1

- Sync upstream v0.27.1: display-only `--hide-storage` for local and view modes (#455), remote host tab identity and chassis energy/cost fixes, bounded remote HTTP buffering, cancelled timed-out remote fetches (#454)
- Merge fix: initialize fork deep-telemetry `GpuInfo` fields and per-process `enc_util`/`dec_util`/`gpu_mem_util` on the new upstream test fixtures in `src/view/event_handler.rs` and `src/view/frame_renderer.rs`
- Fork: DCGM plugin crate version moves from 0.27.0 to 0.27.1 with the main binary
- Fork: `AGENTS.md` (Shadeform NVIDIA validation required before work is done) and `scripts/dev/remote-release-smoke.sh`

Crate version is `0.27.1`; tag is `v0.27.1-metrum.1`.

### Pre-release validation (Shadeform H100)

Shadeform H100 PCIe (Hyperstack, driver 570.195.03, CUDA 12.8, DCGM 4.7.0, rustc 1.99.0). Instance `fe1018e3-ef7e-483c-a886-c92a735e4ed6`, deleted after the run. Log: `TECHNICAL_REPORTS/runs/v0.27.1-metrum.1-h100-smoke/`.

- gengar (no GPU): `cargo build --release --workspace --exclude all-smi-amd-plugin` (no `libdrm-dev` on gengar); `cargo test -p all-smi` all targets pass with the 2 sudo tests skipped (no `sudo` binary)
- Release build on the H100: `all-smi 0.27.1`, `liball_smi_amd.so`, `liball_smi_dcgm.so`
- `cargo test --release -- --test-threads=1`: all targets pass (lib 1885, bin 2082, `library_api_test` 25)
- `ALL_SMI_LIVE_GPU=1` `gpm_parity`: 2 passed
- `doctor --only nvidia.gpm,nvidia.dcgm`: 4 pass; `doctor --only amd`: plugin 0.27.1 loaded, 3 pass, 5 skip
- `cargo test -p all-smi-amd-plugin --release`: 12 passed
- Default API scrape: `source="gpm"` on 20 series; power 48.476 W idle
- `ALL_SMI_NVIDIA_DISABLE_GPM=1` plus the DCGM plugin: `source="dcgm"` on 13 series

### Concurrent NVML/GPM crash fix (follow-up to v0.27.1-metrum.1)

The parallel `library_api_test` SIGSEGV was in NVML, not Rust. Each `NvidiaGpuReader` originally owned an `nvml_wrapper::Nvml`; `Nvml::drop` calls the process-global `nvmlShutdown`, so destroying one `AllSmi` could invalidate NVML state and cached GPM sample handles used by another. The original `GpmState` mutex was reader-local, but `nvml-wrapper` documents GPM sample operations as not thread-safe, so independent instances could also overlap allocate/sample/free operations.

GPU investigation found a second in-process caller that ordinary reader locks could not cover. When the system DCGM hostengine was unavailable, the Metrum DCGM plugin started an embedded hostengine in the same process. Its `NvmlTaskRunner` worker thread used NVML outside the main crate's locks. On the H100 / driver 580.126.09, the library stress test repeatedly faulted inside `libnvidia-ml.so`; the kernel log showed the `NvmlTaskRunner` SIGSEGV as well as test-thread SIGSEGVs. This reproduced with the embedded fallback, while the targeted test passed with the plugin disabled or with an external hostengine. A shared handle and GPM-only mutex were therefore insufficient.

The fix has three parts:

- Keep one successfully initialized NVML handle in a process-lifetime singleton. Initialization failures are not cached, and no reader can call `nvmlShutdown` on the shared handle.
- Serialize complete NVML reader/diagnostic/XID operations and GPM sample allocation, use, and freeing with process-wide locks. Add a real-GPU-skipping concurrent create/drop test (12 threads × 4 `AllSmi` instances).
- Remove embedded DCGM hostengine startup. The plugin connects to the external `nvidia-dcgm` hostengine only; a failed connection is bounded to 100 ms and DCGM enrichment remains unavailable while NVML/GPM collection continues. This keeps DCGM's NVML workers in the separate hostengine process, outside the GPM process. The Shadeform setup now reliably enables the external service, and the release smoke fails fast if it is inactive.

#### Validation

- Gengar, no GPU: `cargo fmt --check`; workspace clippy with `-D warnings` (excluding the AMD plugin requiring host libdrm); `cargo test -p all-smi -- --skip test_ensure_sudo_permissions` (2,080 passed, 2 ignored, 2 filtered; all integration and doctest targets pass); `cargo build --release --workspace --exclude all-smi-amd-plugin`. `all-smi doctor --only nvidia.gpm,nvidia.dcgm` produced 4 SKIP, 0 WARN/FAIL.
- Shadeform H100 PCIe, driver 580.126.09 / CUDA 13.0, DCGM 4.7.0, `rustc 1.99.0`: `cargo build --release --workspace` passed including `liball_smi_amd.so` and `liball_smi_dcgm.so`. Full `cargo test --release -- --test-threads=1` passed (2,082 library tests, 2 ignored, all integration and doctest targets passed).
- The final default-parallel `library_api_test` passed 10/10 with DCGM hostengine inactive and the plugin present, including the new concurrent GPU create/drop regression on every run (26/26 tests each). It also passed 26/26 with the external hostengine active. The prior embedded-hostengine build crashed, confirming why embedded startup was removed.
- With the external hostengine active, `doctor --only nvidia.gpm,nvidia.dcgm` passed 4/4; AMD doctor was 3 PASS / 5 SKIP and AMD plugin tests were 12/12. The default live scrape emitted 20 GPM-sourced series; `ALL_SMI_NVIDIA_DISABLE_GPM=1` emitted 13 DCGM-sourced series. `nvidia-smi` remained responsive. These full smoke checks were repeated after the setup/smoke-script hardening on a separate H100 / driver 570.195.03: workspace release build passed, default-parallel `library_api_test` passed 26/26, and the now-strict release smoke completed successfully.
- The first driver 570.195.03 H100 recorded one `test_allsmi_con` SIGSEGV in `libnvidia-ml.so` at 04:29 UTC before deletion. The driver 580.126.09 box's final dmesg capture still contains three earlier investigative `libnvidia-ml.so` SIGSEGV lines (the incomplete embedded-hostengine builds at 04:54, 05:04, and 05:05 UTC); no new segfault appeared during final validation through 05:39 UTC, and no NVRM assert or RCU stall was logged. The final driver 570.195.03 test box showed no segfault, NVRM assert, or RCU stall through 05:51 UTC. The failed exploratory runs are retained rather than erased.

#### Shadeform lifecycle and cost

- H100 instance `2c951979-2f7a-48ac-b51f-df3112925486` (driver 570.195.03) was deleted and confirmed cleared at 04:39 UTC.
- H100 instance `c4d65db5-fc12-4365-9271-72b7004710eb` (driver 580.126.09) was created at 04:40:41 UTC, used for the main investigation/validation, and deleted at 05:39 UTC.
- H100 instance `579049a7-1d4d-4730-93bf-8e0af1b35ab8` (driver 570.195.03) was created at 05:40:24 UTC, used to validate the hardened external-hostengine setup and strict smoke script, and deleted at 05:51 UTC. All three deletion logs say `deleted`; final status checks reported no local active instance.
- Shadeform listed $2.73/hour for the first two instances and $2.50/hour for the third. Using ~20, 58, and 11 minutes respectively, total GPU compute is estimated at about **$4.02** (provider billing granularity may vary).

Redacted command logs and crash-vs-final evidence are in `TECHNICAL_REPORTS/runs/v0.27.1-metrum.1-nvml-race/`.

### DCGM reconnect backoff follow-up

`src/device/readers/dcgm.rs` now tracks a per-plugin host-side `CreateSessionBackoff`: failed `create_session` calls schedule retries after 1, 2, 4, 8, 16, 32, then 60 seconds repeatedly; successful session creation clears the failure count/deadline. The monotonic deadline logic is tested with injected `Instant` values, and a pure schedule test checks the cap without loading DCGM.

At this stage, `GPM_API_LOCK` was retained because the public `GpmState::{collect, free_all}` and `probe_two_sample` helpers accepted arbitrary NVML handles and could bypass `nvidia_nvml::API_LOCK`. Issue #13 later restricts those sample entry points to the crate: the audited reader and doctor callers run only inside `with_shared_nvml` closures. This removes the bypass instead of maintaining a second GPM lock, while keeping all GPM samples serialized against other in-crate NVML operations. No in-tree callers outside the crate were found; direct use of these low-level helpers is an intentional public API break.

#### Follow-up validation

- Gengar: `cargo fmt --check`; `cargo clippy -p all-smi --all-targets -- -D warnings`; `cargo clippy -p all-smi-dcgm-plugin --all-targets -- -D warnings`; `cargo test -p all-smi -- --skip test_ensure_sudo_permissions` (2,082 passed, 2 ignored, 2 filtered; all integration and doctests pass); `cargo build --release --workspace --exclude all-smi-amd-plugin`.
- Shadeform H100 PCIe, driver 580.126.09 / CUDA 13.0 / DCGM 4.7.0 / rustc 1.99.0. Full `cargo test --release -- --test-threads=1` passed (2,084 passed, 2 ignored; all integration and doctest targets pass). `cargo build --release --workspace` included both plugins. Default-parallel `library_api_test` passed 10/10 (26/26 tests per run).
- `remote-release-smoke.sh` passed with external `nvidia-dcgm.service`: NVIDIA doctor 4/4; AMD doctor 3 pass / 5 skip; AMD tests 12/12; default scrape 20 `source="gpm"` series; fallback scrape 13 `source="dcgm"` series.
- Backoff runtime test stopped the external hostengine while a single `all-smi api` process ran for 30 polls. Trace output shows 5 `create_session` attempts and 25 deferred retries—not one attempt per poll. After restarting the hostengine, `source="dcgm"` returned on recovery poll 2 (within about 2 seconds); the session-created trace confirms the backoff reset. The delay trace followed 1/2/4/8/16 seconds before recovery. `nvidia-smi` remained responsive; baseline and final dmesg contained no segfault, NVRM assertion, or RCU-stall matches.

#### Follow-up Shadeform instance and cost

H100 instance `06f2a92e-9d9b-4d6f-aed1-7b1ac454af60` (driver 580.126.09, $2.73/hour) was created at 15:31:00 UTC, deleted after validation at approximately 15:55 UTC, and confirmed absent by the final status check. Estimated rental cost was **about $1.10**; billing granularity may vary. Redacted follow-up logs are in `TECHNICAL_REPORTS/runs/v0.27.1-metrum.1-followups/`.

## Issue #11 — DCGM hostengine reconnect after sampling failure

`src/device/readers/dcgm.rs` now discards the session on a DCGM sampling error, records the failure in the existing per-plugin retry backoff, and retries session creation when the deadline elapses. A successful sampling call clears the backoff. The remote test stops the external hostengine while an API process is sampling, checks the DCGM source disappears and create attempts are throttled, then starts the hostengine and checks that the DCGM source returns and retry state resets.

### Shadeform validation (RTX PRO 6000 Blackwell)

Shadeform RTXPro6000 instance `dd20cf7a-71ba-4402-acec-f3f1054536b4`, $2.19/hour, created 2026-10-17 04:03:58 UTC and deleted 04:20 UTC after validation (estimated rental cost about **$0.60**, provider billing granularity may vary). The final status reported no local instance state. GPU: NVIDIA RTX PRO 6000 Blackwell Server Edition; driver 580.126.09 / CUDA 13.0; DCGM 4.7.0; `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.

- `cargo build --release --workspace`: passed, including both `liball_smi_amd.so` and `liball_smi_dcgm.so`.
- `cargo test --release -- --test-threads=1`: all test targets passed; main crate library suite 1,893 passed, binary suite 2,090 passed, all integration/plugin/doc targets passed (2 ignored in the binary suite).
- Default-parallel `cargo test --release --test library_api_test`: 10/10 runs passed, each 26 passed / 0 failed.
- Issue-specific hostengine outage/recovery test: initial DCGM source returned; during 30 outage polls the session was discarded, with 5 create attempts and 25 backoff skips; after hostengine recovery the DCGM source returned on poll 2, with backoff reset. The initial refused connection in the log is the expected hostengine stop.
- `all-smi doctor --only nvidia.gpm,nvidia.dcgm`: 4 pass, 0 warnings/failures/skips. `remote-release-smoke.sh` also passed AMD doctor (3 pass / 5 expected skips) and AMD plugin tests (12 passed).
- Live scrape showed 20 `source="gpm"` series by default and 13 `source="dcgm"` series with GPM disabled and DCGM fallback enabled.
- Captured dmesg showed no NVRM Xid/assert or RCU stall. It included an NVIDIA NVSwitch kernel/user version mismatch (kernel driver 580.126.09 vs user driver 590.48.01); no associated crash or stall occurred.

Redacted command logs, GPU-test artifacts, reconnect trace, scrape output, and dmesg are in `TECHNICAL_REPORTS/runs/issue-11-dcgm-reconnect/` and `TECHNICAL_REPORTS/runs/2026-10-17T040809Z/`; both run directories were checked for IPv4 literals.

## Issue #12 — missing sudo executable

`src/utils/system.rs` routes the interactive `sudo -v` invocation through a fallible helper. If the `sudo` process cannot be spawned (for example, the executable is absent), the application prints an explicit “install sudo or run all-smi directly as root” message followed by the existing privilege troubleshooting, then exits through the normal failure path rather than panicking. The hardware-dependent sudo tests avoid entering that path on a non-root Linux host when sudo is not installed. A regression test verifies a missing executable is reported as `io::ErrorKind::NotFound` rather than unwinding.

### Validation

- Local `cargo fmt --all -- --check`, `cargo clippy -p all-smi --all-targets -- -D warnings`, full `cargo test -p all-smi`, and the missing-sudo regression test passed.
- Shadeform RTX PRO 6000 Blackwell Server Edition, driver 580.126.09 / CUDA 13.0, DCGM 4.7.0, `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. The instance `6febe5fc-5de1-48dc-85c7-216d96187c15` ($2.19/hour) was created at 04:42:25 UTC, deleted at approximately 04:58 UTC after validation, and confirmed absent by the final status; estimated rental cost **about $0.56**, billing granularity may vary.
- `cargo build --release --workspace`: passed with both plugin libraries.
- `cargo test --release -- --test-threads=1`: passed all targets (library 1,894 passed / 1 ignored; binary 2,091 passed / 2 ignored; remaining integration, plugin, and doctest targets passed).
- Default-parallel `cargo test --release --test library_api_test`: 10/10 runs passed, each 26 passed / 0 failed.
- `gpu-test.sh` passed; live GPM parity 2/2; `doctor --only nvidia.gpm,nvidia.dcgm` 4 pass; AMD doctor 3 pass / 5 expected skips; AMD plugin 12/12. `remote-release-smoke.sh` showed 20 `source="gpm"` series by default and 13 `source="dcgm"` with fallback enabled.
- Captured dmesg had no NVRM Xid/assert or RCU stall. IPv4 literals were checked absent from run artifacts.

Redacted logs are in `TECHNICAL_REPORTS/runs/issue-12-sudo/` and `TECHNICAL_REPORTS/runs/2026-10-17T044624Z/`.

#### macOS CI cfg correction — final-tree revalidation (2026-10-17)

The first PR CI run caught that `sudo_command_available`, referenced only by Linux tests, was compiled on macOS and failed `-D dead-code`. Restricted the helper to `#[cfg(all(test, target_os = "linux"))]` and reran validation on the final tree before pushing the correction.

- Local `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and full `cargo test -p all-smi`: passed (library 2,091 passed / 2 ignored; all integration and doctests passed).
- Shadeform RTX PRO 6000 Blackwell Server Edition, driver 580.126.09 / CUDA 13.0 / DCGM 4.7.0 / `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. `gpu-test.sh`, `cargo build --release --workspace`, and `cargo test --release -- --test-threads=1` passed. Ten default-parallel `library_api_test` runs passed 26/26 each.
- `remote-release-smoke.sh`: NVIDIA doctor 4 pass / 0 fail; AMD doctor 3 pass / 0 fail / 5 expected skips; AMD plugin tests 12/12; live scrapes emitted 20 `source="gpm"` series and 13 `source="dcgm"` fallback series. `nvidia-smi` remained responsive; dmesg safeguard captured after the successful run.
- Instance `82adcfcc-8f0d-46fa-9ca9-152f5ba5b96a`, created 05:16:06 UTC at $2.19/hour, was deleted and Shadeform status confirmed no local state. Estimated runtime rental cost approximately $0.63; billing granularity may vary.
- Evidence is in `TECHNICAL_REPORTS/runs/issue-12-sudo-recheck/` and `TECHNICAL_REPORTS/runs/2026-10-17T052031Z/`; the transient instance IP is redacted from command logs.

## Issue #13 — public GPM helpers bypass the process-wide NVML lock

`GpmState` and `probe_two_sample` were public despite requiring every NVML/GPM operation to run under the process-wide `API_LOCK`. A downstream caller could pass an unrelated `Nvml::init()` handle and bypass serialization, reintroducing the GPM/NVML race and also risking process-global shutdown on drop. The repository audit found the only sampling call sites in `NvidiaGpuReader` and the NVIDIA doctor check; both are invoked inside `with_shared_nvml` closures. These sampling entry points and the state type are now `pub(crate)`, while the pure `has_activity_metrics` helper remains public. `GPM_API_LOCK` is removed because the one shared lock now protects all in-crate GPM operations against all other NVML calls. The visibility change intentionally breaks direct use of these low-level helpers; consumers should use the supported `AllSmi` APIs.

### Validation

- Local `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and full `cargo test -p all-smi`: passed (library 2,091 passed / 2 ignored; integration and doctests passed).
- `cargo doc -p all-smi --no-deps` completed. It reports pre-existing rustdoc warnings elsewhere; the GPM-specific private-type link was corrected, and the generated public GPM module docs omit `GpmState` and `probe_two_sample`.
- Shadeform RTX PRO 6000 Blackwell Server Edition, driver 580.126.09 / CUDA 13.0 / DCGM 4.7.0 / `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. `gpu-test.sh`, `cargo build --release --workspace`, and default-parallel `cargo test --release` passed. Ten default-parallel `library_api_test` runs passed 26/26 each.
- `remote-release-smoke.sh`: NVIDIA doctor 4 pass / 0 fail; AMD doctor 3 pass / 0 fail / 5 expected skips; AMD plugin tests 12/12. Live scrapes emitted 20 `source="gpm"` series and 13 `source="dcgm"` fallback series. Captured dmesg has no Xid, GPU reset, segfault, RCU stall, or `BUG:` matches.
- Shadeform instance `e661fd0e-8d2e-44f3-8d4c-a348414a4eba` was created at 05:57:16 UTC at $2.19/hour, deleted at 06:13:05 UTC, and confirmed absent by status; estimated runtime cost about $0.58 (billing granularity may vary).
- Evidence is in `TECHNICAL_REPORTS/runs/issue-13-gpm-lock/` and `TECHNICAL_REPORTS/runs/2026-10-17T060128Z/`; the transient instance IP is redacted from logs.

## Issue #14 — XID watcher lock fairness and event-storm latency

The watcher must serialize NVML event waits against GPM because GPM sample operations are explicitly not thread-safe. It now caps each wait while holding `API_LOCK` at 1 ms (previously 10 ms), then records an event outside the shared NVML critical section. After each received event it yields for 1 ms before retrying, preventing the watcher from continuously reacquiring the non-fair mutex during a queue burst. The no-event/error backoff remains 50 ms. This preserves conservative NVML/GPM serialization while bounding routine lock occupancy and allowing waiting reader threads to run. An actual XID storm was not induced on the rented production-class GPU; deliberately generating GPU fault events would be unsafe. The event-burst fairness protection is therefore assessed by code path, not a destructive hardware test.

### Collection latency profile

Used the existing ignored `tests/perf_tick_stages.rs` hardware benchmark (`PERF_TICKS=30`) on RTX PRO 6000 Blackwell Server Edition, replaying steady-state local collection at one-second cadence. For an A/B/A comparison on the same instance, watcher startup was temporarily disabled only for the middle benchmark, then restored and re-synced before final validation. No temporary benchmark/source changes remain.

| watcher | GPU `get_gpu_info` avg (min–max) | whole collection tick avg (min–max) |
|---|---:|---:|
| enabled, first run | 63.546 ms (57.936–69.523) | 72.549 ms (68.159–82.230) |
| temporarily disabled | 63.730 ms (58.932–69.642) | 72.060 ms (65.740–79.783) |
| enabled, final run | 64.659 ms (60.185–73.752) | 75.138 ms (68.534–86.187) |

The enabled/disabled differences are within the observed run-to-run spread; no measurable idle-watcher collection penalty or regression was seen. This samples the idle event queue, not a live fault storm. Full profiles: `TECHNICAL_REPORTS/runs/issue-14-xid-fairness/perf-ticks-watcher-{enabled,disabled}.log` and `perf-ticks-watcher-enabled-final.log`. Separate 100-call reader timings are also retained in the same directory.

### Validation

- Local `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test -p all-smi`: passed (2,091 library tests passed / 2 ignored; integration and doctest targets passed).
- Shadeform RTX PRO 6000 Blackwell Server Edition; driver 580.126.09 / CUDA 13.0 / DCGM 4.7.0 / `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. `gpu-test.sh` passed (`cargo test --release`: 2,091 passed / 2 ignored, plus all integration/doc targets; live GPM parity passed). `cargo build --release --workspace` built the main crate and both `liball_smi_amd.so` and `liball_smi_dcgm.so`. Ten default-parallel `library_api_test` runs passed 26/26 each.
- `remote-release-smoke.sh`: `doctor --only nvidia.gpm,nvidia.dcgm` passed all 4 checks (0 warnings/failures); AMD doctor 3 pass / 0 fail / 5 expected skips; AMD plugin tests 12/12. Live `/metrics` emitted `source="gpm"` by default (20 series) and `source="dcgm"` in the DCGM fallback (13 series). `nvidia-smi` remained responsive.
- `sudo -n dmesg` showed no Xid, reset, segfault, RCU stall, or `BUG:`. It did show a host-provided NVSwitch kernel/user version mismatch warning (kernel 580.126.09, user 590.48.01); NVIDIA doctor and live scrapes passed on this instance.
- Instance `ed16b876-2a99-490d-99ff-5b243c9dc707`, created 06:35:52 UTC at $2.19/hour, was deleted and `shadeform.py status` confirmed no local state. Approximately 27 minutes elapsed (about $0.99 at the listed hourly rate; billing granularity may vary).
- Evidence: `TECHNICAL_REPORTS/runs/issue-14-xid-fairness/` and `TECHNICAL_REPORTS/runs/2026-10-17T064642Z/`; transient IPs redacted from console logs.

## Issue #15 — manually dispatched Shadeform GPU validation workflow

Added `.github/workflows/shadeform-gpu-validation.yml`, restricted to manual dispatch on `main`; it has no pull-request, push, release, or tag trigger and does not gate releases. Concurrency serialization prevents overlapping paid instances. It requires repository Actions secrets `SHADEFORM_API_KEY` and `SHADEFORM_SSH_PRIVATE_KEY` (not configured or changed during this work). The runner derives the SSH public key locally; `shadeform.py` reads the API key from the injected environment or, for local runs, retains the existing git-ignored `env.json` fallback. Instance selection preserves RTXPro6000 → B200 → H200 → H100 → L40S order, skips unknown/over-$3.00-per-hour types, and fails closed if none meet the rate cap. A 50-minute validation-step limit and 60-minute job limit keep the nominal run charge at or below $3, subject to provider billing granularity.

The validation script captures raw logs privately, runs `gpu-setup.sh`, `gpu-test.sh`, full workspace release build, ten default-parallel `library_api_test` runs, `remote-release-smoke.sh`, and the dmesg safeguard. Its EXIT trap deletes the instance; the workflow adds an `if: always()` fallback deletion step, then copies only text/JSON evidence through a redactor before uploading a 14-day artifact. The redactor scrubs IPv4 addresses, API/SSH secret values, and private-key blocks. No release/tag policy is changed.

### Validation

- `python3 scripts/dev/test_shadeform.py`: 7 tests pass, covering environment-secret loading, price cap fallback/unknown-price rejection, cap validation, and existing GPU preference behavior; `python3 scripts/dev/test_redact_gpu_logs.py`: 1 redaction test passes. Shell syntax, Python compilation, embedded workflow-shell parsing, preflight-without-secret path, and `git diff --check` pass. The GitHub workflow itself was not dispatched because the requested repository secrets were not configured (and must not be configured by this task).
- Final source was exercised end-to-end locally using the already-provisioned host Shadeform credential in process environment only (no repository secret was created). Shadeform RTX PRO 6000 Blackwell Server Edition, driver 580.126.09 / CUDA 13.0 / DCGM 4.7.0 / `rustc 1.99.1 (58cf8f9c3 2026-10-13)`; both the catalog selection and returned instance price were checked against the $3/hour cap, selecting RTXPro6000 at $2.19/hour. `gpu-test.sh` passed: `cargo test --release` had 2,091 passed / 2 ignored and all integration/doc tests passed, live GPM parity passed. Full workspace build produced main, AMD plugin, and DCGM plugin. Ten default-parallel `library_api_test` runs passed 26/26 each.
- Remote smoke passed NVIDIA doctor 4/4 and AMD doctor 3 pass / 0 fail / 5 expected skips; live metrics exposed `source="gpm"` by default (20 series) and `source="dcgm"` on fallback (13 series). dmesg safeguard found no segfault, NVRM assertion, RCU stall, or `BUG:`; it recorded only normal driver startup/LTR notices.
- Final passing instance `b8aec863-e837-4e1a-ab99-a7c60792ab34` was created at 07:47:40 UTC for $2.19/hour and deleted at 08:02:48; about 15 minutes elapsed (~$0.55 by elapsed time). An earlier full run during implementation also passed (`90a9c64a-8616-4aca-b7cf-11cd4e7a765d`, about 14 minutes). A subsequent retest initially hit a stale local known-host entry for a recycled Shadeform IP; its EXIT trap immediately deleted that instance (`7ef8d168-a3e7-41e0-901c-b161497c9115`), after which the stale worktree-generated cache was removed and final validation passed. All three instances were deleted and local status confirmed no state; about 30 minutes total rented across runs (~$1.10 at $2.19/hour before provider billing granularity).
- Redacted final evidence: `TECHNICAL_REPORTS/runs/issue-15-shadeform-validation/` (24 files, including the timestamped `gpu-test.sh` run); IPs, secret values, and private-key blocks were scrubbed before storage. Temporary unredacted logs and generated local known-host cache were removed.
