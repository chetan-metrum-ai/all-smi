// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! NVML GPM two-sample collector for Hopper+ GPUs.
//!
//! GPM metrics require two time-separated samples passed to
//! [`nvml_wrapper::gpm::gpm_metrics_get`]. This module caches the previous
//! raw sample handle per GPU UUID across polls (via `into_handle` /
//! `from_handle`) so subsequent polls stay cheap. On the *first* poll for a
//! UUID the collector self-primes with a short second sample so one-shot
//! tools (`all-smi snapshot`) populate activity fields instead of returning
//! an empty metrics shell filled only by the NVML PCIe fallback.
//!
//! Set `ALL_SMI_NVIDIA_DISABLE_GPM=1` to force the GPM path off (used by P4
//! to exercise the DCGM fallback).

use crate::device::readers::nvidia_hardware::gpm_is_supported;
use crate::device::types::{GpmMetrics, TelemetrySource};
use nvml_wrapper::enums::gpm::GpmMetricId;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::gpm::{GpmSample, gpm_metrics_get};
use nvml_wrapper::struct_wrappers::gpm::GpmMetricResult;
use nvml_wrapper::{Device, Nvml};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Env var that disables the NVML GPM collector entirely when set to a
/// non-empty value other than `0` / `false` / `no`.
pub const DISABLE_GPM_ENV_VAR: &str = "ALL_SMI_NVIDIA_DISABLE_GPM";

/// Metric IDs requested every poll. NVDEC/NVJPG/NVOFA instance IDs are
/// averaged into the vendor-neutral `*_active` fields.
const IDS: &[GpmMetricId] = &[
    GpmMetricId::GraphicsUtil,
    GpmMetricId::SmUtil,
    GpmMetricId::SmOccupancy,
    GpmMetricId::AnyTensorUtil,
    GpmMetricId::HmmaTensorUtil,
    GpmMetricId::ImmaTensorUtil,
    GpmMetricId::DfmaTensorUtil,
    GpmMetricId::Fp64Util,
    GpmMetricId::Fp32Util,
    GpmMetricId::Fp16Util,
    GpmMetricId::IntegerUtil,
    GpmMetricId::DramBwUtil,
    GpmMetricId::PcieTxPerSec,
    GpmMetricId::PcieRxPerSec,
    GpmMetricId::NvlinkTotalTxPerSec,
    GpmMetricId::NvlinkTotalRxPerSec,
    GpmMetricId::Nvdec0Util,
    GpmMetricId::Nvdec1Util,
    GpmMetricId::Nvdec2Util,
    GpmMetricId::Nvdec3Util,
    GpmMetricId::Nvdec4Util,
    GpmMetricId::Nvdec5Util,
    GpmMetricId::Nvdec6Util,
    GpmMetricId::Nvdec7Util,
    GpmMetricId::Nvjpg0Util,
    GpmMetricId::Nvjpg1Util,
    GpmMetricId::Nvjpg2Util,
    GpmMetricId::Nvjpg3Util,
    GpmMetricId::Nvjpg4Util,
    GpmMetricId::Nvjpg5Util,
    GpmMetricId::Nvjpg6Util,
    GpmMetricId::Nvjpg7Util,
    GpmMetricId::Nvofa0Util,
    GpmMetricId::Nvofa1Util,
];

/// Per-device previous GPM sample handles, keyed by GPU UUID.
///
/// Handles are stored as `usize` bits from [`GpmSample::into_handle`] so this
/// crate never names `nvml-wrapper`'s private FFI sample type.
pub struct GpmState {
    prev: Mutex<HashMap<String, usize>>,
}

impl GpmState {
    pub fn new() -> Self {
        Self {
            prev: Mutex::new(HashMap::new()),
        }
    }

    /// `true` when the operator has forced GPM off.
    pub fn is_disabled() -> bool {
        match std::env::var(DISABLE_GPM_ENV_VAR) {
            Ok(v) => {
                let v = v.trim().to_ascii_lowercase();
                !(v.is_empty() || v == "0" || v == "false" || v == "no")
            }
            Err(_) => false,
        }
    }

    /// Take a GPM sample and, when a previous sample exists for `uuid`,
    /// compute metrics.
    ///
    /// On the first poll for a UUID there is no prior sample. Rather than
    /// returning `None` (which made one-shot tools like
    /// `all-smi snapshot --samples 1` look as if GPM were unavailable —
    /// issue #5), we immediately take a second sample after a short gap and
    /// return the computed metrics. Subsequent polls reuse the cached
    /// handle with no extra sleep.
    pub fn collect(&self, nvml: &Nvml, device: &Device<'_>) -> Option<GpmMetrics> {
        if Self::is_disabled() {
            return None;
        }
        if !gpm_is_supported(device) {
            return None;
        }

        let uuid = device.uuid().ok()?;
        let cur = device.gpm_sample().ok()?;

        let mut map = self.prev.lock().unwrap_or_else(|e| e.into_inner());
        let Some(prev_bits) = map.remove(&uuid) else {
            // First poll: prime with a second sample so one-shot collectors
            // get activity metrics instead of an empty `source=nvml` shell.
            const PRIME_GAP: Duration = Duration::from_millis(200);
            std::thread::sleep(PRIME_GAP);
            let second = match device.gpm_sample() {
                Ok(s) => s,
                Err(_) => {
                    map.insert(uuid, sample_to_bits(cur));
                    return None;
                }
            };
            let results = match gpm_metrics_get(nvml, &cur, &second, IDS) {
                Ok(r) => r,
                Err(_) => {
                    let _ = cur.free();
                    map.insert(uuid, sample_to_bits(second));
                    return None;
                }
            };
            let metrics = metrics_from_results(&results);
            let _ = cur.free();
            map.insert(uuid, sample_to_bits(second));
            return Some(metrics);
        };

        let prev = unsafe { sample_from_bits(nvml, prev_bits) };
        let results = match gpm_metrics_get(nvml, &prev, &cur, IDS) {
            Ok(r) => r,
            Err(_) => {
                drop(prev);
                map.insert(uuid, sample_to_bits(cur));
                return None;
            }
        };
        drop(prev);

        map.insert(uuid, sample_to_bits(cur));
        Some(metrics_from_results(&results))
    }

    /// Drop every cached handle without freeing (NVML library gone / replaced).
    /// Leaks the underlying NVML sample allocations; only used on rare reinit.
    pub fn abandon(&self) {
        if let Ok(mut map) = self.prev.lock() {
            map.clear();
        }
    }

    /// Free every cached sample through `nvml`. Call before the reader drops
    /// while the NVML library is still alive.
    pub fn free_all(&self, nvml: &Nvml) {
        let Ok(mut map) = self.prev.lock() else {
            return;
        };
        for (_, bits) in map.drain() {
            let sample = unsafe { sample_from_bits(nvml, bits) };
            let _ = sample.free();
        }
    }
}

impl Default for GpmState {
    fn default() -> Self {
        Self::new()
    }
}

/// Take two GPM samples separated by `gap` and return the computed metrics.
///
/// Used by `doctor` so support is validated by real sampling rather than
/// NVML's `gpm_support()` flag alone (which can be true on SM120 RTX parts
/// that never populate activity fields). Temporary sample handles are freed
/// before returning. Returns `None` when GPM is disabled, unsupported, or
/// either sample / `gpm_metrics_get` fails.
pub fn probe_two_sample(nvml: &Nvml, device: &Device<'_>, gap: Duration) -> Option<GpmMetrics> {
    if GpmState::is_disabled() {
        return None;
    }
    if !gpm_is_supported(device) {
        return None;
    }
    let first = device.gpm_sample().ok()?;
    std::thread::sleep(gap);
    let second = match device.gpm_sample() {
        Ok(s) => s,
        Err(_) => {
            let _ = first.free();
            return None;
        }
    };
    let results = match gpm_metrics_get(nvml, &first, &second, IDS) {
        Ok(r) => r,
        Err(_) => {
            let _ = first.free();
            let _ = second.free();
            return None;
        }
    };
    let metrics = metrics_from_results(&results);
    let _ = first.free();
    let _ = second.free();
    Some(metrics)
}

/// `true` when at least one genuine GPM **activity** field is present.
///
/// PCIe / NVLink byte-rate counters are excluded: the collector can fill
/// those via the non-GPM NVML `pcie_throughput` fallback (`source=nvml`),
/// which must not count as proof that GPM works (issue #5 / RTX PRO 6000).
/// A present ratio of `0.0` is valid — absence is `None`, not zero.
pub fn has_activity_metrics(m: &GpmMetrics) -> bool {
    m.graphics_active.is_some()
        || m.sm_active.is_some()
        || m.sm_occupancy.is_some()
        || m.tensor_active.is_some()
        || m.tensor_hmma_active.is_some()
        || m.tensor_imma_active.is_some()
        || m.tensor_dfma_active.is_some()
        || m.fp64_active.is_some()
        || m.fp32_active.is_some()
        || m.fp16_active.is_some()
        || m.integer_active.is_some()
        || m.memory_bandwidth_utilization.is_some()
        || m.nvdec_active.is_some()
        || m.nvjpg_active.is_some()
        || m.nvofa_active.is_some()
}

fn sample_to_bits(sample: GpmSample<'_>) -> usize {
    // `nvmlGpmSample_t` is an opaque pointer; store the address only.
    sample.into_handle() as usize
}

unsafe fn sample_from_bits<'nvml>(nvml: &'nvml Nvml, bits: usize) -> GpmSample<'nvml> {
    // SAFETY: `bits` came from `into_handle` on a sample allocated from this
    // same NVML library instance (or we are about to free/abandon).
    unsafe { GpmSample::from_handle(nvml, bits as _) }
}

fn pct_to_ratio(value: f64) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    let ratio = (value / 100.0) as f32;
    if (0.0..=1.0).contains(&ratio) {
        Some(ratio)
    } else if ratio > 1.0 && ratio <= 1.05 {
        Some(1.0)
    } else {
        None
    }
}

fn rate_bytes(value: f64) -> Option<f64> {
    // NVML GPM PCIe/NVLink `*_PER_SEC` metrics return megabytes/sec on
    // current drivers (matches DCGM 1009–1012 once scaled). Export
    // bytes/sec for Prometheus consistency with the DCGM field docs.
    if value.is_finite() && value >= 0.0 {
        Some(value * 1_000_000.0)
    } else {
        None
    }
}

fn mean_ratio(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

fn metrics_from_results(results: &[Result<GpmMetricResult, NvmlError>]) -> GpmMetrics {
    let mut out = GpmMetrics {
        source: Some(TelemetrySource::Gpm),
        ..GpmMetrics::default()
    };
    let mut nvdec = Vec::new();
    let mut nvjpg = Vec::new();
    let mut nvofa = Vec::new();

    for (i, result) in results.iter().enumerate() {
        let Ok(metric) = result else {
            continue;
        };
        let id = IDS.get(i).copied().unwrap_or(metric.metric_id);
        match id {
            GpmMetricId::GraphicsUtil => out.graphics_active = pct_to_ratio(metric.value),
            GpmMetricId::SmUtil => out.sm_active = pct_to_ratio(metric.value),
            GpmMetricId::SmOccupancy => out.sm_occupancy = pct_to_ratio(metric.value),
            GpmMetricId::AnyTensorUtil => out.tensor_active = pct_to_ratio(metric.value),
            GpmMetricId::HmmaTensorUtil => out.tensor_hmma_active = pct_to_ratio(metric.value),
            GpmMetricId::ImmaTensorUtil => out.tensor_imma_active = pct_to_ratio(metric.value),
            GpmMetricId::DfmaTensorUtil => out.tensor_dfma_active = pct_to_ratio(metric.value),
            GpmMetricId::Fp64Util => out.fp64_active = pct_to_ratio(metric.value),
            GpmMetricId::Fp32Util => out.fp32_active = pct_to_ratio(metric.value),
            GpmMetricId::Fp16Util => out.fp16_active = pct_to_ratio(metric.value),
            GpmMetricId::IntegerUtil => out.integer_active = pct_to_ratio(metric.value),
            GpmMetricId::DramBwUtil => {
                out.memory_bandwidth_utilization = pct_to_ratio(metric.value)
            }
            GpmMetricId::PcieTxPerSec => out.pcie_tx_bytes_per_sec = rate_bytes(metric.value),
            GpmMetricId::PcieRxPerSec => out.pcie_rx_bytes_per_sec = rate_bytes(metric.value),
            GpmMetricId::NvlinkTotalTxPerSec => {
                out.nvlink_tx_bytes_per_sec = rate_bytes(metric.value)
            }
            GpmMetricId::NvlinkTotalRxPerSec => {
                out.nvlink_rx_bytes_per_sec = rate_bytes(metric.value)
            }
            GpmMetricId::Nvdec0Util
            | GpmMetricId::Nvdec1Util
            | GpmMetricId::Nvdec2Util
            | GpmMetricId::Nvdec3Util
            | GpmMetricId::Nvdec4Util
            | GpmMetricId::Nvdec5Util
            | GpmMetricId::Nvdec6Util
            | GpmMetricId::Nvdec7Util => {
                if let Some(r) = pct_to_ratio(metric.value) {
                    nvdec.push(r);
                }
            }
            GpmMetricId::Nvjpg0Util
            | GpmMetricId::Nvjpg1Util
            | GpmMetricId::Nvjpg2Util
            | GpmMetricId::Nvjpg3Util
            | GpmMetricId::Nvjpg4Util
            | GpmMetricId::Nvjpg5Util
            | GpmMetricId::Nvjpg6Util
            | GpmMetricId::Nvjpg7Util => {
                if let Some(r) = pct_to_ratio(metric.value) {
                    nvjpg.push(r);
                }
            }
            GpmMetricId::Nvofa0Util | GpmMetricId::Nvofa1Util => {
                if let Some(r) = pct_to_ratio(metric.value) {
                    nvofa.push(r);
                }
            }
            _ => {}
        }
    }

    out.nvdec_active = mean_ratio(&nvdec);
    out.nvjpg_active = mean_ratio(&nvjpg);
    out.nvofa_active = mean_ratio(&nvofa);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_to_ratio_scales_and_rejects_nonsense() {
        assert!((pct_to_ratio(50.0).unwrap() - 0.5).abs() < 1e-6);
        assert!(pct_to_ratio(-1.0).is_none());
        assert!(pct_to_ratio(f64::NAN).is_none());
        assert_eq!(pct_to_ratio(0.0), Some(0.0));
        assert_eq!(pct_to_ratio(100.0), Some(1.0));
    }

    #[test]
    fn disable_env_parses_truthy() {
        for (v, expect) in [
            ("1", true),
            ("true", true),
            ("yes", true),
            ("0", false),
            ("false", false),
            ("no", false),
            ("", false),
        ] {
            let disabled = {
                let v = v.trim().to_ascii_lowercase();
                !(v.is_empty() || v == "0" || v == "false" || v == "no")
            };
            assert_eq!(disabled, expect, "v={v}");
        }
    }

    #[test]
    fn has_activity_ignores_pcie_only() {
        let mut empty = GpmMetrics::default();
        assert!(!has_activity_metrics(&empty));

        empty.pcie_rx_bytes_per_sec = Some(1.0);
        empty.pcie_tx_bytes_per_sec = Some(2.0);
        empty.source = Some(TelemetrySource::Nvml);
        assert!(
            !has_activity_metrics(&empty),
            "PCIe NVML fallback must not count as GPM activity"
        );

        empty.sm_active = Some(0.0);
        assert!(
            has_activity_metrics(&empty),
            "zero sm_active is a valid present reading"
        );

        let mut nonzero = GpmMetrics {
            graphics_active: Some(0.5),
            ..GpmMetrics::default()
        };
        assert!(has_activity_metrics(&nonzero));
        nonzero.graphics_active = None;
        nonzero.memory_bandwidth_utilization = Some(0.1);
        assert!(has_activity_metrics(&nonzero));
    }
}
