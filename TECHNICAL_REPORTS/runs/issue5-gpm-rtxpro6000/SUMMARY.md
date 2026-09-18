# Issue #5 live verification — RTX PRO 6000 Blackwell Server Edition

Shadeform `RTXPro6000` (massedcompute), driver 580.126.09.

## Findings

Contrary to the original report's hypothesis that SM120 RTX parts lack GPM entirely, a real two-sample probe **does** populate GPM activity fields on this SKU (`source=gpm`). Idle readings are `0.0` (valid present values). The earlier `snapshot` nulls with `source=nvml` were from the collector returning `None` on the first poll (PCIe fallback only).

## After fix

| Check | Result |
|-------|--------|
| `doctor --only nvidia.gpm` | **PASS** — activity metrics sampled (not flag-only) |
| `snapshot --samples 1` | `source=gpm`, `sm_active=0.0`, `graphics_active=0.0` (self-primed) |

Doctor still **WARN**s when NVML claims support but the two-sample probe yields no activity fields (regression covered by unit tests).
