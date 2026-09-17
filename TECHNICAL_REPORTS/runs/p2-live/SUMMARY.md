# P2 live scrape summary

Shadeform H100 idle scrape (power-limit change not permitted without root).

- `throttle_reason`: present
- `energy_hw`: present
- `remapped_rows`: present
- `utilization_sample`: present
- `sm_active_ratio`: present
- `pcie_tx`: present

Observed: `throttle_reason{reason="gpu_idle"}`, energy HW counter, remapped rows, util sample p50/p95/max, GPM `source="gpm"`.
