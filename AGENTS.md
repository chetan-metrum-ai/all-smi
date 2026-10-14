<!-- Copyright (c) 2026 Metrum AI, Inc. All rights reserved. -->

# Agent instructions for the Metrum all-smi fork

This repo is the Metrum AI fork of [lablup/all-smi](https://github.com/lablup/all-smi)
(`origin` = `chetan-metrum-ai/all-smi`, `upstream` = `lablup/all-smi`). It adds
NVIDIA GPM, NVML extras, and the DCGM plugin on top of upstream. These rules apply
to every coding agent (pi, Claude Code, Codex, Cursor) and to humans.

## Rule: validate on a real NVIDIA GPU via Shadeform before calling work done

No change, upstream sync, or release tag is done until it has been built and
tested on a Shadeform NVIDIA GPU instance. A green build on a dev host without a
GPU (for example gengar) is not enough. Budget for this is approved, so do not
skip the step to save cost. Do tear the instance down afterwards.

Workflow (scripts live in `scripts/dev/`, written by Metrum AI):

```sh
python3 scripts/dev/shadeform.py status        # refuse to double-rent; reuse a live box
python3 scripts/dev/shadeform.py create        # RTXPro6000 -> H200 -> H100 -> L40S (B200 banned)
scripts/dev/gpu-setup.sh                       # once per box: toolchain, libdrm, DCGM, torch
scripts/dev/gpu-test.sh                        # sync, release build, tests, GPM parity, doctor
scripts/dev/gpu-run.sh cargo build --release --workspace                     # plugins too
scripts/dev/gpu-run.sh bash scripts/dev/remote-release-smoke.sh              # doctor + live scrapes
python3 scripts/dev/shadeform.py delete --yes-i-am-sure                      # always
```

Minimum evidence to record in the PR "Test plan" and in
`TECHNICAL_REPORTS/deep-telemetry-progress.md`:

- GPU model and driver, and `rustc --version` on the box
- `cargo build --release --workspace` passes, including `liball_smi_amd.so` and
  `liball_smi_dcgm.so`
- `cargo test --release` result line
- `all-smi doctor --only nvidia.gpm,nvidia.dcgm` passes
- a live `/metrics` scrape shows `source="gpm"` by default and `source="dcgm"` on
  the DCGM fallback

`gpu-test.sh` writes run artifacts to `TECHNICAL_REPORTS/runs/<timestamp>/`.

### Known hazard: parallel NVML tests crash the GPU box

`tests/library_api_test.rs` creates and drops `AllSmi` instances on parallel test
threads. On a real NVIDIA GPU this segfaults inside `libnvidia-ml.so` (seen on
H100 with driver 570 and on RTX PRO 6000 with driver 580, on `main` as of
v0.27.0-metrum.1). On the RTX PRO 6000 it then wedged the kernel driver
(`NVRM: nvAssertFailedNoLog`, RCU stalls, `nvidia-smi` hangs), and the box had
to be deleted. Until the race is fixed, run the GPU-box test suite with
`cargo test --release -- --test-threads=1`, and run any parallel repro only as
the last step before deleting the instance. If `nvidia-smi` hangs, don't
debug the box. Capture `dmesg`, delete it, and rent a fresh one.

## Secrets

- `SHADEFORM_API_KEY` is in the git-ignored `env.json` at the repo root. On gengar
  it is also in `~/src/genai-smart-router/ops.env.json`. `shadeform.py` reads it
  in-process and sends it only as a header.
- Never print, echo, commit, or pass the key on a command line. Inspect env files
  by key name only.
- `env.json`, `.shadeform-state.json` and the `.shadeform_ed25519*` keys are
  git-ignored and excluded by `gpu-sync.sh`. Keep it that way.

## Upstream syncs

Follow the pattern of PR #8 (v0.27.0-metrum.1):

1. `git fetch --tags upstream`, then `git switch -c sync/upstream-vX.Y.Z main` and
   `git merge upstream/main`.
2. Bump `crates/all-smi-dcgm-plugin/Cargo.toml` to the main crate version.
   A clean textual merge is not a clean build. New upstream `GpuInfo` and
   `ProcessInfo` literals, usually in tests, lack the fork's deep-telemetry
   fields (`throttle_reasons`, `energy_hw_millijoules`, `remapped_rows`,
   `nvlink_errors`, `utilization_samples`, `xid_event_counts`; `enc_util`,
   `dec_util`, `gpu_mem_util`). Run `cargo test` (not only `cargo build`) and
   add them.
3. Add a `vX.Y.Z-metrum.N` entry to the Metrum section of `CHANGELOG.md` and to
   `TECHNICAL_REPORTS/deep-telemetry-progress.md`, and update the tag example in
   `DEVELOPERS.md`.
4. Validate on Shadeform as above. Open the PR against `origin` (this fork), never
   against lablup.
5. After merging, tag `vX.Y.Z-metrum.N` and confirm the Metrum release workflow
   uploads the linux x86_64 archive.

## Host notes

- `all-smi-amd-plugin` links against `libdrm` and `libdrm_amdgpu`. On a host
  without `libdrm-dev` (gengar today), build with
  `cargo build --release --workspace --exclude all-smi-amd-plugin` and rely on the
  Shadeform box, where `gpu-setup.sh` installs libdrm, for the full workspace build.
- `.gitignore` ignores `*.md` and dotfiles by default. Add a `!path` exception, or
  use `git add -f`, for new docs.

---
Maintained by Metrum AI.
