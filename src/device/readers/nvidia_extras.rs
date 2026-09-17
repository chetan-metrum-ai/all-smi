// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! NVML extras for P2: throttle reasons, process util, PCIe fallback,
//! energy counter, remapped rows, NVLink errors, utilization samples.

use crate::device::types::{
    GpmMetrics, NvLinkErrorCount, RemappedRowsInfo, TelemetrySource, ThrottleReasons,
    UtilizationSample,
};
use nvml_wrapper::Device;
use nvml_wrapper::bitmasks::device::ThrottleReasons as NvmlThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{PcieUtilCounter, Sampling};
use nvml_wrapper::enum_wrappers::nv_link::ErrorCounter;
use nvml_wrapper::enums::device::SampleValue;
use nvml_wrapper::structs::device::FieldId;
use nvml_wrapper::sys_exports::field_id;
use std::collections::HashMap;

const MAX_UTIL_SAMPLES: usize = 64;

/// Map NVML throttle bitmask into our serialisable struct.
pub fn collect_throttle_reasons(device: &Device<'_>) -> Option<ThrottleReasons> {
    let reasons = device.current_throttle_reasons().ok()?;
    Some(ThrottleReasons {
        gpu_idle: reasons.contains(NvmlThrottleReasons::GPU_IDLE),
        app_clocks: reasons.contains(NvmlThrottleReasons::APPLICATIONS_CLOCKS_SETTING),
        sw_power_cap: reasons.contains(NvmlThrottleReasons::SW_POWER_CAP),
        hw_slowdown: reasons.contains(NvmlThrottleReasons::HW_SLOWDOWN),
        sync_boost: reasons.contains(NvmlThrottleReasons::SYNC_BOOST),
        sw_thermal: reasons.contains(NvmlThrottleReasons::SW_THERMAL_SLOWDOWN),
        hw_thermal: reasons.contains(NvmlThrottleReasons::HW_THERMAL_SLOWDOWN),
        hw_power_brake: reasons.contains(NvmlThrottleReasons::HW_POWER_BRAKE_SLOWDOWN),
        display_clocks: reasons.contains(NvmlThrottleReasons::DISPLAY_CLOCK_SETTING),
    })
}

/// Fill GPM PCIe fields from `pcie_throughput` when GPM left them `None`.
pub fn apply_pcie_throughput_fallback(device: &Device<'_>, gpm: &mut Option<GpmMetrics>) {
    let tx = device
        .pcie_throughput(PcieUtilCounter::Send)
        .ok()
        .map(|v| v as f64 * 1024.0); // NVML returns KiB/s
    let rx = device
        .pcie_throughput(PcieUtilCounter::Receive)
        .ok()
        .map(|v| v as f64 * 1024.0);
    if tx.is_none() && rx.is_none() {
        return;
    }
    let metrics = gpm.get_or_insert_with(GpmMetrics::default);
    let mut used_fallback = false;
    if metrics.pcie_tx_bytes_per_sec.is_none()
        && let Some(v) = tx
    {
        metrics.pcie_tx_bytes_per_sec = Some(v);
        used_fallback = true;
    }
    if metrics.pcie_rx_bytes_per_sec.is_none()
        && let Some(v) = rx
    {
        metrics.pcie_rx_bytes_per_sec = Some(v);
        used_fallback = true;
    }
    if used_fallback && metrics.source.is_none() {
        metrics.source = Some(TelemetrySource::Nvml);
    }
}

pub fn collect_energy_hw_millijoules(device: &Device<'_>) -> Option<u64> {
    device.total_energy_consumption().ok()
}

pub fn collect_remapped_rows(device: &Device<'_>) -> Option<RemappedRowsInfo> {
    let ids = [
        FieldId(field_id::NVML_FI_DEV_REMAPPED_COR),
        FieldId(field_id::NVML_FI_DEV_REMAPPED_UNC),
        FieldId(field_id::NVML_FI_DEV_REMAPPED_PENDING),
        FieldId(field_id::NVML_FI_DEV_REMAPPED_FAILURE),
    ];
    let values = device.field_values_for(&ids).ok()?;
    let mut info = RemappedRowsInfo::default();
    let mut any = false;
    for (i, sample) in values.into_iter().enumerate() {
        let Ok(sample) = sample else {
            continue;
        };
        let Ok(value) = sample.value else {
            continue;
        };
        any = true;
        let v = sample_value_as_u64(&value).unwrap_or(0);
        match i {
            0 => info.correctable = v as u32,
            1 => info.uncorrectable = v as u32,
            2 => info.pending = v != 0,
            3 => info.failed = v != 0,
            _ => {}
        }
    }
    any.then_some(info)
}

fn sample_value_as_u64(value: &SampleValue) -> Option<u64> {
    match value {
        SampleValue::U32(v) => Some(*v as u64),
        SampleValue::U64(v) => Some(*v),
        SampleValue::I64(v) if *v >= 0 => Some(*v as u64),
        SampleValue::F64(v) if *v >= 0.0 && v.is_finite() => Some(*v as u64),
        SampleValue::I64(_) | SampleValue::F64(_) => None,
    }
}

/// Collect NVLink error counters for active links (0..18 probe).
pub fn collect_nvlink_errors(device: &Device<'_>) -> Vec<NvLinkErrorCount> {
    let mut out = Vec::new();
    for link_index in 0..18u32 {
        let link = device.link_wrapper_for(link_index);
        if !link.is_active().unwrap_or(false) {
            continue;
        }
        for (counter, label) in [
            (ErrorCounter::DlCrcFlit, "crc_flit"),
            (ErrorCounter::DlCrcData, "crc_data"),
            (ErrorCounter::DlReplay, "replay"),
            (ErrorCounter::DlRecovery, "recovery"),
        ] {
            if let Ok(count) = link.error_counter(counter) {
                out.push(NvLinkErrorCount {
                    link_index,
                    error_type: label.to_string(),
                    count,
                });
            }
        }
    }
    out
}

/// Recent GPU utilization samples, newest last, capped at [`MAX_UTIL_SAMPLES`].
pub fn collect_utilization_samples(
    device: &Device<'_>,
    last_ts: Option<u64>,
) -> Option<(Vec<UtilizationSample>, u64)> {
    let samples = device.samples(Sampling::GpuUtilization, last_ts).ok()?;
    if samples.is_empty() {
        return None;
    }
    let mut newest_ts = last_ts.unwrap_or(0);
    let mut out = Vec::with_capacity(samples.len().min(MAX_UTIL_SAMPLES));
    for s in samples {
        newest_ts = newest_ts.max(s.timestamp);
        let value = match s.value {
            SampleValue::F64(v) => v as f32,
            SampleValue::U32(v) => v as f32,
            SampleValue::U64(v) => v as f32,
            SampleValue::I64(v) => v as f32,
        };
        out.push(UtilizationSample {
            timestamp_us: s.timestamp,
            value,
        });
    }
    if out.len() > MAX_UTIL_SAMPLES {
        let skip = out.len() - MAX_UTIL_SAMPLES;
        out = out.split_off(skip);
    }
    Some((out, newest_ts))
}

/// Apply process utilization samples onto a PID→util map (SM percent 0-100).
pub fn collect_process_util_by_pid(
    device: &Device<'_>,
    last_seen: Option<u64>,
) -> (HashMap<u32, ProcessUtil>, u64) {
    let mut newest = last_seen.unwrap_or(0);
    let mut map = HashMap::new();
    let Ok(samples) = device.process_utilization_stats(last_seen) else {
        return (map, newest);
    };
    for s in samples {
        newest = newest.max(s.timestamp);
        map.insert(
            s.pid,
            ProcessUtil {
                sm: s.sm_util as f64,
                mem: Some(s.mem_util as f32),
                enc: Some(s.enc_util as f32),
                dec: Some(s.dec_util as f32),
            },
        );
    }
    (map, newest)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessUtil {
    pub sm: f64,
    pub mem: Option<f32>,
    pub enc: Option<f32>,
    pub dec: Option<f32>,
}

/// Percentile helpers for utilization sample summaries.
pub fn sample_percentile(samples: &[UtilizationSample], pct: f32) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let mut vals: Vec<f32> = samples.iter().map(|s| s.value).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((pct / 100.0) * (vals.len() as f32 - 1.0)).round() as usize;
    vals.get(idx.clamp(0, vals.len() - 1)).copied()
}

pub fn sample_max(samples: &[UtilizationSample]) -> Option<f32> {
    samples
        .iter()
        .map(|s| s.value)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_active_labels_and_throttled() {
        let mut t = ThrottleReasons::default();
        assert!(!t.is_throttled());
        t.sw_power_cap = true;
        assert!(t.is_throttled());
        assert_eq!(t.active_labels(), vec!["sw_power_cap"]);
    }

    #[test]
    fn sample_percentile_p50() {
        let samples: Vec<_> = (0..5)
            .map(|i| UtilizationSample {
                timestamp_us: i,
                value: i as f32 * 10.0,
            })
            .collect();
        assert_eq!(sample_percentile(&samples, 50.0), Some(20.0));
        assert_eq!(sample_max(&samples), Some(40.0));
    }
}
