// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//! Telemetry surface completeness for the GPM field set (P1).
//!
//! Builds a fully populated [`GpuInfo`], exports Prometheus text, parses it
//! back, and asserts every GPM field survives the hop losslessly. Also
//! checks snapshot JSON round-trip for the same object.

#![cfg(feature = "cli")]

use all_smi::api::metrics::MetricExporter;
use all_smi::api::metrics::hardware::HardwareMetricExporter;
use all_smi::device::types::{
    GpmMetrics, GpuInfo, NvLinkErrorCount, RemappedRowsInfo, TelemetrySource, ThrottleReasons,
    UtilizationSample,
};
use all_smi::network::metrics_parser::MetricsParser;
use regex::Regex;
use std::collections::HashMap;

fn metric_re() -> Regex {
    Regex::new(r"^all_smi_([^\{]+)\{([^}]+)\} ([\d\.eE+-]+)$").unwrap()
}

fn populated_gpm() -> GpmMetrics {
    GpmMetrics {
        graphics_active: Some(0.91),
        sm_active: Some(0.12),
        sm_occupancy: Some(0.08),
        tensor_active: Some(0.04),
        tensor_hmma_active: Some(0.03),
        tensor_imma_active: Some(0.01),
        tensor_dfma_active: Some(0.0),
        fp64_active: Some(0.0),
        fp32_active: Some(0.10),
        fp16_active: Some(0.02),
        integer_active: Some(0.0),
        memory_bandwidth_utilization: Some(0.33),
        pcie_tx_bytes_per_sec: Some(1.25e9),
        pcie_rx_bytes_per_sec: Some(4.5e8),
        nvlink_tx_bytes_per_sec: Some(5.0e9),
        nvlink_rx_bytes_per_sec: Some(5.1e9),
        nvdec_active: Some(0.0),
        nvjpg_active: Some(0.0),
        nvofa_active: Some(0.0),
        source: Some(TelemetrySource::Gpm),
    }
}

fn populated_gpu() -> GpuInfo {
    GpuInfo {
        uuid: "GPU-TEST-P1".to_string(),
        time: String::new(),
        name: "NVIDIA H100".to_string(),
        device_type: "GPU".to_string(),
        host_id: "node-p1".to_string(),
        hostname: "node-p1".to_string(),
        instance: "node-p1".to_string(),
        utilization: 90.0,
        ane_utilization: 0.0,
        dla_utilization: None,
        tensorcore_utilization: None,
        temperature: 50,
        used_memory: 0,
        total_memory: 0,
        frequency: 0,
        power_consumption: 0.0,
        gpu_core_count: None,
        temperature_threshold_slowdown: None,
        temperature_threshold_shutdown: None,
        temperature_threshold_max_operating: None,
        temperature_threshold_acoustic: None,
        performance_state: None,
        fan_speed_rpm: None,
        numa_node_id: Some(0),
        gsp_firmware_mode: None,
        gsp_firmware_version: None,
        nvlink_remote_devices: Vec::new(),
        gpm_metrics: Some(populated_gpm()),
        throttle_reasons: Some(ThrottleReasons {
            sw_power_cap: true,
            ..ThrottleReasons::default()
        }),
        energy_hw_millijoules: Some(12_345_678),
        remapped_rows: Some(RemappedRowsInfo {
            correctable: 2,
            uncorrectable: 0,
            pending: false,
            failed: false,
        }),
        nvlink_errors: vec![NvLinkErrorCount {
            link_index: 0,
            error_type: "crc_flit".to_string(),
            count: 3,
        }],
        utilization_samples: Some(vec![
            UtilizationSample {
                timestamp_us: 1,
                value: 10.0,
            },
            UtilizationSample {
                timestamp_us: 2,
                value: 20.0,
            },
            UtilizationSample {
                timestamp_us: 3,
                value: 40.0,
            },
        ]),
        xid_event_counts: {
            let mut m = HashMap::new();
            m.insert(13u32, 1u64);
            m
        },
        detail: HashMap::new(),
    }
}

fn assert_gpm_close(a: &GpmMetrics, b: &GpmMetrics) {
    let check_f32 = |name: &str, x: Option<f32>, y: Option<f32>| match (x, y) {
        (Some(x), Some(y)) => assert!((x - y).abs() < 1e-4, "{name}: {x} vs {y}"),
        (None, None) => {}
        _ => panic!("{name}: {x:?} vs {y:?}"),
    };
    let check_f64 = |name: &str, x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => {
            assert!((x - y).abs() / x.max(1.0) < 1e-6, "{name}: {x} vs {y}")
        }
        (None, None) => {}
        _ => panic!("{name}: {x:?} vs {y:?}"),
    };
    check_f32("graphics_active", a.graphics_active, b.graphics_active);
    check_f32("sm_active", a.sm_active, b.sm_active);
    check_f32("sm_occupancy", a.sm_occupancy, b.sm_occupancy);
    check_f32("tensor_active", a.tensor_active, b.tensor_active);
    check_f32(
        "tensor_hmma_active",
        a.tensor_hmma_active,
        b.tensor_hmma_active,
    );
    check_f32(
        "tensor_imma_active",
        a.tensor_imma_active,
        b.tensor_imma_active,
    );
    check_f32(
        "tensor_dfma_active",
        a.tensor_dfma_active,
        b.tensor_dfma_active,
    );
    check_f32("fp64_active", a.fp64_active, b.fp64_active);
    check_f32("fp32_active", a.fp32_active, b.fp32_active);
    check_f32("fp16_active", a.fp16_active, b.fp16_active);
    check_f32("integer_active", a.integer_active, b.integer_active);
    check_f32(
        "memory_bandwidth_utilization",
        a.memory_bandwidth_utilization,
        b.memory_bandwidth_utilization,
    );
    check_f64(
        "pcie_tx_bytes_per_sec",
        a.pcie_tx_bytes_per_sec,
        b.pcie_tx_bytes_per_sec,
    );
    check_f64(
        "pcie_rx_bytes_per_sec",
        a.pcie_rx_bytes_per_sec,
        b.pcie_rx_bytes_per_sec,
    );
    check_f64(
        "nvlink_tx_bytes_per_sec",
        a.nvlink_tx_bytes_per_sec,
        b.nvlink_tx_bytes_per_sec,
    );
    check_f64(
        "nvlink_rx_bytes_per_sec",
        a.nvlink_rx_bytes_per_sec,
        b.nvlink_rx_bytes_per_sec,
    );
    check_f32("nvdec_active", a.nvdec_active, b.nvdec_active);
    check_f32("nvjpg_active", a.nvjpg_active, b.nvjpg_active);
    check_f32("nvofa_active", a.nvofa_active, b.nvofa_active);
    assert_eq!(a.source, b.source);
}

#[test]
fn gpm_prometheus_parser_round_trip() {
    let gpu = populated_gpu();
    let exposition = HardwareMetricExporter::new(std::slice::from_ref(&gpu)).export_metrics();
    assert!(
        exposition.contains("all_smi_gpu_sm_active_ratio{"),
        "exporter missing sm_active:\n{exposition}"
    );
    assert!(
        exposition.contains(r#"source="gpm""#),
        "exporter missing source label:\n{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_throttle_reason{"),
        "{exposition}"
    );
    assert!(
        exposition.contains(r#"reason="sw_power_cap""#),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_energy_hw_millijoules_total{"),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_remapped_rows{"),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_nvlink_errors_total{"),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_utilization_sample_p50{"),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_xid_events_total{"),
        "{exposition}"
    );
    assert!(
        exposition.contains("all_smi_gpu_hollow_utilization_ratio{"),
        "hollow util missing:\n{exposition}"
    );

    let parser = MetricsParser::new();
    let parsed = parser.parse_metrics(&exposition, "node-p1:9090", &metric_re());
    assert_eq!(parsed.gpu_info.len(), 1);
    let round_gpu = &parsed.gpu_info[0];
    let round = round_gpu
        .gpm_metrics
        .as_ref()
        .expect("gpm present after parse");
    assert_gpm_close(gpu.gpm_metrics.as_ref().unwrap(), round);
    let hollow = all_smi::metrics::gpu_readings::hollow_utilization(&gpu).expect("hollow");
    assert!((hollow - 0.79).abs() < 1e-4, "hollow={hollow}");
    let thr = round_gpu.throttle_reasons.expect("throttle");
    assert!(thr.sw_power_cap);
    assert_eq!(round_gpu.energy_hw_millijoules, Some(12_345_678));
    let remap = round_gpu.remapped_rows.expect("remap");
    assert_eq!(remap.correctable, 2);
    assert!(!round_gpu.nvlink_errors.is_empty());
    assert_eq!(round_gpu.xid_event_counts.get(&13), Some(&1));
}

#[test]
fn gpm_json_serde_round_trip() {
    let gpu = populated_gpu();
    let json = serde_json::to_string(&gpu).expect("serialize");
    let back: GpuInfo = serde_json::from_str(&json).expect("deserialize");
    assert_gpm_close(
        gpu.gpm_metrics.as_ref().unwrap(),
        back.gpm_metrics.as_ref().unwrap(),
    );
}
