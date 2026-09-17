// Copyright 2025 Lablup Inc. and Jeongkyu Shin
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! Prometheus exporter for NVIDIA hardware-detail metrics (issue #132).
//!
//! Metrics produced by this exporter:
//!
//! * `all_smi_gpu_numa_node_id` (gauge): NUMA node the GPU is attached to.
//!   Emitted only when the reader returned a concrete value — absence
//!   signals "unavailable" (no NUMA topology, driver too old, non-Linux
//!   platform). Dashboards MUST treat `absent` as "unknown" rather than
//!   defaulting to 0.
//! * `all_smi_gpu_gsp_firmware_mode` (gauge): `0=disabled`, `1=enabled`,
//!   `2=default`. Absent when unsupported.
//! * `all_smi_gpu_gsp_firmware_version_info` (gauge, label-only): info-style
//!   metric carrying the firmware version string in a `version` label with
//!   a constant value of 1. Absent when unsupported.
//! * `all_smi_nvlink_remote_device_type` (gauge, label-only): one row per
//!   active NvLink carrying `link_index` and `remote_type` labels with a
//!   constant value of 1. Empty when no NvLinks are active.
//! * GPM family (Hopper+ / DCGM fallback), each with a `source` label
//!   (`gpm` | `dcgm` | `dcgmi` | `amd`):
//!   - `all_smi_gpu_graphics_active_ratio`
//!   - `all_smi_gpu_sm_active_ratio`
//!   - `all_smi_gpu_sm_occupancy`
//!   - `all_smi_gpu_tensor_active_ratio` (+ hmma / imma / dfma)
//!   - `all_smi_gpu_fp64_active_ratio` / `fp32` / `fp16`
//!   - `all_smi_gpu_integer_active_ratio`
//!   - `all_smi_gpu_memory_bandwidth_utilization`
//!   - `all_smi_gpu_pcie_{tx,rx}_bytes_per_second`
//!   - `all_smi_gpu_nvlink_{tx,rx}_bytes_per_second`
//!   - `all_smi_gpu_nvdec_active_ratio` / `nvjpg` / `nvofa`
//!
//! All metrics carry the standard GPU label set (`gpu`, `instance`, `gpu_uuid`,
//! `gpu_index`), matching the core `all_smi_gpu_temperature_celsius` series so
//! dashboards can correlate by label.
//!
//! The exporter emits nothing when no GPUs populated any of the new fields
//! — non-NVIDIA paths, older NVIDIA drivers without the relevant APIs, and
//! hosts with zero NvLinks stay silent in the `/metrics` output.

use super::{MetricBuilder, MetricExporter};
use crate::device::GpuInfo;
use crate::device::types::{GpmMetrics, TelemetrySource};

/// Hardware-detail exporter.
///
/// Takes a borrowed slice of [`GpuInfo`] and emits the issue-#132 metric
/// families. Row caching (stringifying the device index once per GPU) is
/// done in [`HardwareMetricExporter::collect_rows`] so the four metric
/// families iterate without re-allocating labels.
pub struct HardwareMetricExporter<'a> {
    pub gpu_info: &'a [GpuInfo],
}

impl<'a> HardwareMetricExporter<'a> {
    pub fn new(gpu_info: &'a [GpuInfo]) -> Self {
        Self { gpu_info }
    }

    /// Pre-compute the stringified `index` label and only keep GPUs that
    /// contribute at least one hardware-detail row. Allocates once per
    /// scrape regardless of how many metric families are later emitted.
    fn collect_rows(&self) -> Vec<Row<'a>> {
        self.gpu_info
            .iter()
            .enumerate()
            .filter(|(_, gpu)| has_any_hw_detail(gpu))
            .map(|(idx, gpu)| Row {
                gpu,
                index_str: idx.to_string(),
            })
            .collect()
    }

    fn base_labels<'b>(row: &'b Row<'a>) -> [(&'b str, &'b str); 4] {
        [
            ("gpu", row.gpu.name.as_str()),
            ("instance", row.gpu.instance.as_str()),
            ("gpu_uuid", row.gpu.uuid.as_str()),
            ("gpu_index", row.index_str.as_str()),
        ]
    }

    fn export_numa_node_id(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        // Emit HELP/TYPE unconditionally so the exposition format is well-
        // formed even when no GPU has NUMA data. This matches the
        // per-metric-family pattern used by the vGPU / MIG exporters.
        let mut emitted_any = false;
        for row in rows {
            if let Some(node_id) = row.gpu.numa_node_id {
                if !emitted_any {
                    builder
                        .help(
                            "all_smi_gpu_numa_node_id",
                            "NUMA node the GPU is attached to (metric is omitted when the host \
                             has no NUMA topology or the driver does not report one)",
                        )
                        .type_("all_smi_gpu_numa_node_id", "gauge");
                    emitted_any = true;
                }
                let labels = Self::base_labels(row);
                builder.metric("all_smi_gpu_numa_node_id", &labels, node_id);
            }
        }
    }

    fn export_gsp_firmware_mode(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted_any = false;
        for row in rows {
            if let Some(mode) = row.gpu.gsp_firmware_mode {
                if !emitted_any {
                    builder
                        .help(
                            "all_smi_gpu_gsp_firmware_mode",
                            "GSP firmware mode (0=disabled, 1=enabled, 2=default); omitted when \
                             the driver does not expose the GSP firmware API",
                        )
                        .type_("all_smi_gpu_gsp_firmware_mode", "gauge");
                    emitted_any = true;
                }
                let labels = Self::base_labels(row);
                builder.metric("all_smi_gpu_gsp_firmware_mode", &labels, mode);
            }
        }
    }

    fn export_gsp_firmware_version(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted_any = false;
        for row in rows {
            if let Some(ref version) = row.gpu.gsp_firmware_version {
                if !emitted_any {
                    builder
                        .help(
                            "all_smi_gpu_gsp_firmware_version_info",
                            "GSP firmware version, encoded as a constant 1 with the version in a \
                             `version` label; omitted when unsupported",
                        )
                        .type_("all_smi_gpu_gsp_firmware_version_info", "gauge");
                    emitted_any = true;
                }
                let base = Self::base_labels(row);
                let labels = [
                    base[0],
                    base[1],
                    base[2],
                    base[3],
                    ("version", version.as_str()),
                ];
                builder.metric("all_smi_gpu_gsp_firmware_version_info", &labels, 1);
            }
        }
    }

    fn export_nvlink_remote_device_type(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted_any = false;
        // Buffer for the optional `bandwidth_mb_s` label (issue #190). The
        // string lives on the stack frame as an `Option<String>` per row/link
        // because `labels` borrows a `&str`. Old exporters omit the label
        // entirely so remote scrapers parsing pre-#190 output continue to
        // deserialise cleanly (handled by `.unwrap_or(None)` in the parser).
        for row in rows {
            for link in &row.gpu.nvlink_remote_devices {
                if !emitted_any {
                    builder
                        .help(
                            "all_smi_nvlink_remote_device_type",
                            "NvLink remote endpoint classification per active link. Value is \
                             always 1; classification is carried in the `remote_type` label \
                             (gpu / switch / ibmnpu / unknown). Optional `bandwidth_mb_s` label \
                             carries the per-link bandwidth hint used by the topology tab's \
                             NVn classifier (omitted when the driver does not report it).",
                        )
                        .type_("all_smi_nvlink_remote_device_type", "gauge");
                    emitted_any = true;
                }
                let link_idx_str = link.link_index.to_string();
                let bandwidth_str = link.bandwidth_mb_s.map(|v| v.to_string());
                let base = Self::base_labels(row);
                match bandwidth_str {
                    Some(ref bw) => {
                        let labels = [
                            base[0],
                            base[1],
                            base[2],
                            base[3],
                            ("link_index", link_idx_str.as_str()),
                            ("remote_type", link.remote_type.as_label()),
                            ("bandwidth_mb_s", bw.as_str()),
                        ];
                        builder.metric("all_smi_nvlink_remote_device_type", &labels, 1);
                    }
                    None => {
                        let labels = [
                            base[0],
                            base[1],
                            base[2],
                            base[3],
                            ("link_index", link_idx_str.as_str()),
                            ("remote_type", link.remote_type.as_label()),
                        ];
                        builder.metric("all_smi_nvlink_remote_device_type", &labels, 1);
                    }
                }
            }
        }
    }

    fn export_gpm_metrics(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        // Only emit gauges when the underlying reader surfaced a numeric
        // value. Presence of a `GpmMetrics` struct without any populated
        // field (first poll / handshake incomplete) must produce no output
        // so dashboards cannot confuse it with a real zero reading.
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_graphics_active_ratio",
            "Graphics engine active fraction (0.0-1.0); aligns with nvidia-smi GPU-Util",
            |m| m.graphics_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_sm_active_ratio",
            "SM active fraction (0.0-1.0); DCGM field 1002",
            |m| m.sm_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_sm_occupancy",
            "GPM-reported SM occupancy fraction (0.0-1.0); omitted on devices \
             that do not support GPM (pre-Hopper)",
            |m| m.sm_occupancy,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_tensor_active_ratio",
            "Any tensor pipe active fraction (0.0-1.0); DCGM field 1004",
            |m| m.tensor_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_tensor_hmma_active_ratio",
            "HMMA tensor pipe active fraction (0.0-1.0)",
            |m| m.tensor_hmma_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_tensor_imma_active_ratio",
            "IMMA tensor pipe active fraction (0.0-1.0)",
            |m| m.tensor_imma_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_tensor_dfma_active_ratio",
            "DFMA tensor pipe active fraction (0.0-1.0)",
            |m| m.tensor_dfma_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_fp64_active_ratio",
            "FP64 pipe active fraction (0.0-1.0); DCGM field 1006",
            |m| m.fp64_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_fp32_active_ratio",
            "FP32 pipe active fraction (0.0-1.0); DCGM field 1007",
            |m| m.fp32_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_fp16_active_ratio",
            "FP16 pipe active fraction (0.0-1.0); DCGM field 1008",
            |m| m.fp16_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_integer_active_ratio",
            "Integer pipe active fraction (0.0-1.0)",
            |m| m.integer_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_memory_bandwidth_utilization",
            "GPM-reported memory bandwidth utilization fraction (0.0-1.0); \
             omitted on devices that do not support GPM (pre-Hopper)",
            |m| m.memory_bandwidth_utilization,
        );
        emit_gpm_rate(
            builder,
            rows,
            "all_smi_gpu_pcie_tx_bytes_per_second",
            "PCIe transmit throughput in bytes/sec; DCGM field 1009",
            |m| m.pcie_tx_bytes_per_sec,
        );
        emit_gpm_rate(
            builder,
            rows,
            "all_smi_gpu_pcie_rx_bytes_per_second",
            "PCIe receive throughput in bytes/sec; DCGM field 1010",
            |m| m.pcie_rx_bytes_per_sec,
        );
        emit_gpm_rate(
            builder,
            rows,
            "all_smi_gpu_nvlink_tx_bytes_per_second",
            "NVLink transmit throughput in bytes/sec (total); DCGM field 1011",
            |m| m.nvlink_tx_bytes_per_sec,
        );
        emit_gpm_rate(
            builder,
            rows,
            "all_smi_gpu_nvlink_rx_bytes_per_second",
            "NVLink receive throughput in bytes/sec (total); DCGM field 1012",
            |m| m.nvlink_rx_bytes_per_sec,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_nvdec_active_ratio",
            "Mean NVDEC instance utilization (0.0-1.0)",
            |m| m.nvdec_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_nvjpg_active_ratio",
            "Mean NVJPG instance utilization (0.0-1.0)",
            |m| m.nvjpg_active,
        );
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_nvofa_active_ratio",
            "Mean NVOFA instance utilization (0.0-1.0)",
            |m| m.nvofa_active,
        );
        // Derived hollow util (P3): graphics_active - sm_active, clamped ≥ 0.
        emit_gpm_ratio(
            builder,
            rows,
            "all_smi_gpu_hollow_utilization_ratio",
            "Hollow utilization: graphics_active - sm_active (clamped ≥ 0); \
             omitted when either input is unavailable",
            |m| match (m.graphics_active, m.sm_active) {
                (Some(g), Some(s)) => Some((g - s).max(0.0)),
                _ => None,
            },
        );
    }

    fn export_throttle_reasons(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted = false;
        for row in rows {
            let Some(reasons) = row.gpu.throttle_reasons else {
                continue;
            };
            for reason in reasons.active_labels() {
                if !emitted {
                    builder
                        .help(
                            "all_smi_gpu_throttle_reason",
                            "Active NVML clock-throttle reason (1 when set)",
                        )
                        .type_("all_smi_gpu_throttle_reason", "gauge");
                    emitted = true;
                }
                let base = Self::base_labels(row);
                let labels = [base[0], base[1], base[2], base[3], ("reason", reason)];
                builder.metric("all_smi_gpu_throttle_reason", &labels, 1);
            }
        }
    }

    fn export_energy_hw(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted = false;
        for row in rows {
            let Some(mj) = row.gpu.energy_hw_millijoules else {
                continue;
            };
            if !emitted {
                builder
                    .help(
                        "all_smi_gpu_energy_hw_millijoules_total",
                        "NVML hardware energy counter (millijoules since driver load); \
                         distinct from software-integrated all_smi_energy_consumed_joules_total",
                    )
                    .type_("all_smi_gpu_energy_hw_millijoules_total", "counter");
                emitted = true;
            }
            let labels = Self::base_labels(row);
            builder.metric("all_smi_gpu_energy_hw_millijoules_total", &labels, mj);
        }
    }

    fn export_remapped_rows(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted_rows = false;
        let mut emitted_pending = false;
        let mut emitted_failed = false;
        for row in rows {
            let Some(remap) = row.gpu.remapped_rows else {
                continue;
            };
            let labels = Self::base_labels(row);
            if !emitted_rows {
                builder
                    .help(
                        "all_smi_gpu_remapped_rows",
                        "HBM remapped row counts by cause",
                    )
                    .type_("all_smi_gpu_remapped_rows", "gauge");
                emitted_rows = true;
            }
            let corr = [
                labels[0],
                labels[1],
                labels[2],
                labels[3],
                ("cause", "correctable"),
            ];
            builder.metric("all_smi_gpu_remapped_rows", &corr, remap.correctable);
            let uncorr = [
                labels[0],
                labels[1],
                labels[2],
                labels[3],
                ("cause", "uncorrectable"),
            ];
            builder.metric("all_smi_gpu_remapped_rows", &uncorr, remap.uncorrectable);

            if !emitted_pending {
                builder
                    .help(
                        "all_smi_gpu_remapping_pending",
                        "1 when row remapping is pending a reset",
                    )
                    .type_("all_smi_gpu_remapping_pending", "gauge");
                emitted_pending = true;
            }
            builder.metric(
                "all_smi_gpu_remapping_pending",
                &labels,
                if remap.pending { 1 } else { 0 },
            );

            if !emitted_failed {
                builder
                    .help(
                        "all_smi_gpu_remapping_failed",
                        "1 when row remapping failed",
                    )
                    .type_("all_smi_gpu_remapping_failed", "gauge");
                emitted_failed = true;
            }
            builder.metric(
                "all_smi_gpu_remapping_failed",
                &labels,
                if remap.failed { 1 } else { 0 },
            );
        }
    }

    fn export_nvlink_errors(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted = false;
        for row in rows {
            for err in &row.gpu.nvlink_errors {
                if !emitted {
                    builder
                        .help(
                            "all_smi_gpu_nvlink_errors_total",
                            "NVLink error counters by link and type",
                        )
                        .type_("all_smi_gpu_nvlink_errors_total", "counter");
                    emitted = true;
                }
                let link_str = err.link_index.to_string();
                let base = Self::base_labels(row);
                let labels = [
                    base[0],
                    base[1],
                    base[2],
                    base[3],
                    ("link", link_str.as_str()),
                    ("type", err.error_type.as_str()),
                ];
                builder.metric("all_smi_gpu_nvlink_errors_total", &labels, err.count);
            }
        }
    }

    fn export_utilization_sample_summaries(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        use crate::device::readers::nvidia_extras::{sample_max, sample_percentile};

        let mut emitted_p50 = false;
        let mut emitted_p95 = false;
        let mut emitted_max = false;
        for row in rows {
            let Some(ref samples) = row.gpu.utilization_samples else {
                continue;
            };
            if samples.is_empty() {
                continue;
            }
            let labels = Self::base_labels(row);
            if let Some(v) = sample_percentile(samples, 50.0) {
                if !emitted_p50 {
                    builder
                        .help(
                            "all_smi_gpu_utilization_sample_p50",
                            "p50 of recent NVML GpuUtilization samples (percent)",
                        )
                        .type_("all_smi_gpu_utilization_sample_p50", "gauge");
                    emitted_p50 = true;
                }
                builder.metric("all_smi_gpu_utilization_sample_p50", &labels, v);
            }
            if let Some(v) = sample_percentile(samples, 95.0) {
                if !emitted_p95 {
                    builder
                        .help(
                            "all_smi_gpu_utilization_sample_p95",
                            "p95 of recent NVML GpuUtilization samples (percent)",
                        )
                        .type_("all_smi_gpu_utilization_sample_p95", "gauge");
                    emitted_p95 = true;
                }
                builder.metric("all_smi_gpu_utilization_sample_p95", &labels, v);
            }
            if let Some(v) = sample_max(samples) {
                if !emitted_max {
                    builder
                        .help(
                            "all_smi_gpu_utilization_sample_max",
                            "max of recent NVML GpuUtilization samples (percent)",
                        )
                        .type_("all_smi_gpu_utilization_sample_max", "gauge");
                    emitted_max = true;
                }
                builder.metric("all_smi_gpu_utilization_sample_max", &labels, v);
            }
        }
    }

    fn export_xid_events(&self, builder: &mut MetricBuilder, rows: &[Row<'a>]) {
        let mut emitted = false;
        for row in rows {
            for (xid, count) in &row.gpu.xid_event_counts {
                if !emitted {
                    builder
                        .help(
                            "all_smi_gpu_xid_events_total",
                            "Cumulative XID / ECC events observed since process start",
                        )
                        .type_("all_smi_gpu_xid_events_total", "counter");
                    emitted = true;
                }
                let xid_str = xid.to_string();
                let base = Self::base_labels(row);
                let labels = [
                    base[0],
                    base[1],
                    base[2],
                    base[3],
                    ("xid", xid_str.as_str()),
                ];
                builder.metric("all_smi_gpu_xid_events_total", &labels, *count);
            }
        }
    }
}

fn gpm_source_label(metrics: &GpmMetrics) -> &'static str {
    metrics.source.unwrap_or(TelemetrySource::Gpm).as_label()
}

fn emit_gpm_ratio<'a, F>(
    builder: &mut MetricBuilder,
    rows: &[Row<'a>],
    name: &str,
    help: &str,
    getter: F,
) where
    F: Fn(&GpmMetrics) -> Option<f32>,
{
    let mut emitted_header = false;
    for row in rows {
        let Some(ref metrics) = row.gpu.gpm_metrics else {
            continue;
        };
        let Some(value) = getter(metrics) else {
            continue;
        };
        if !emitted_header {
            builder.help(name, help).type_(name, "gauge");
            emitted_header = true;
        }
        let base = HardwareMetricExporter::base_labels(row);
        let source = gpm_source_label(metrics);
        let labels = [base[0], base[1], base[2], base[3], ("source", source)];
        builder.metric(name, &labels, value);
    }
}

fn emit_gpm_rate<'a, F>(
    builder: &mut MetricBuilder,
    rows: &[Row<'a>],
    name: &str,
    help: &str,
    getter: F,
) where
    F: Fn(&GpmMetrics) -> Option<f64>,
{
    let mut emitted_header = false;
    for row in rows {
        let Some(ref metrics) = row.gpu.gpm_metrics else {
            continue;
        };
        let Some(value) = getter(metrics) else {
            continue;
        };
        if !emitted_header {
            builder.help(name, help).type_(name, "gauge");
            emitted_header = true;
        }
        let base = HardwareMetricExporter::base_labels(row);
        let source = gpm_source_label(metrics);
        let labels = [base[0], base[1], base[2], base[3], ("source", source)];
        builder.metric(name, &labels, value);
    }
}

/// Returns true when a GPU has at least one hardware-detail field
/// populated. Used by [`HardwareMetricExporter::collect_rows`] so we skip
/// non-NVIDIA rows entirely without iterating them in every metric family.
fn has_any_hw_detail(gpu: &GpuInfo) -> bool {
    gpu.numa_node_id.is_some()
        || gpu.gsp_firmware_mode.is_some()
        || gpu.gsp_firmware_version.is_some()
        || !gpu.nvlink_remote_devices.is_empty()
        || gpu.gpm_metrics.is_some()
        || gpu.throttle_reasons.is_some()
        || gpu.energy_hw_millijoules.is_some()
        || gpu.remapped_rows.is_some()
        || !gpu.nvlink_errors.is_empty()
        || gpu
            .utilization_samples
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        || !gpu.xid_event_counts.is_empty()
}

/// Borrowed view of a single GPU row with its stringified index cached.
/// Exists purely to amortise the `.to_string()` on the index label across
/// the five metric families.
struct Row<'a> {
    gpu: &'a GpuInfo,
    index_str: String,
}

impl<'a> MetricExporter for HardwareMetricExporter<'a> {
    fn export_metrics(&self) -> String {
        let rows = self.collect_rows();
        if rows.is_empty() {
            return String::new();
        }

        let mut builder = MetricBuilder::new();
        self.export_numa_node_id(&mut builder, &rows);
        self.export_gsp_firmware_mode(&mut builder, &rows);
        self.export_gsp_firmware_version(&mut builder, &rows);
        self.export_nvlink_remote_device_type(&mut builder, &rows);
        self.export_gpm_metrics(&mut builder, &rows);
        self.export_throttle_reasons(&mut builder, &rows);
        self.export_energy_hw(&mut builder, &rows);
        self.export_remapped_rows(&mut builder, &rows);
        self.export_nvlink_errors(&mut builder, &rows);
        self.export_utilization_sample_summaries(&mut builder, &rows);
        self.export_xid_events(&mut builder, &rows);
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{GpmMetrics, NvLinkRemoteDevice, NvLinkRemoteType};
    use std::collections::HashMap;

    fn make_nvidia_gpu() -> GpuInfo {
        GpuInfo {
            uuid: "GPU-ABC".to_string(),
            time: String::new(),
            name: "NVIDIA A100".to_string(),
            device_type: "GPU".to_string(),
            host_id: "node-1".to_string(),
            hostname: "node-1".to_string(),
            instance: "node-1".to_string(),
            utilization: 0.0,
            ane_utilization: 0.0,
            dla_utilization: None,
            tensorcore_utilization: None,
            temperature: 0,
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
            gsp_firmware_mode: Some(1),
            gsp_firmware_version: Some("550.54.15".to_string()),
            nvlink_remote_devices: vec![
                NvLinkRemoteDevice {
                    link_index: 0,
                    remote_type: NvLinkRemoteType::Gpu,
                    bandwidth_mb_s: Some(400_000),
                },
                NvLinkRemoteDevice {
                    link_index: 1,
                    remote_type: NvLinkRemoteType::Switch,
                    bandwidth_mb_s: None,
                },
            ],
            gpm_metrics: Some(GpmMetrics {
                sm_occupancy: Some(0.67),
                memory_bandwidth_utilization: Some(0.42),
                source: Some(TelemetrySource::Gpm),
                ..Default::default()
            }),
            throttle_reasons: None,
            energy_hw_millijoules: None,
            remapped_rows: None,
            nvlink_errors: Vec::new(),
            utilization_samples: None,
            xid_event_counts: HashMap::new(),
            detail: HashMap::new(),
        }
    }

    fn make_non_nvidia_gpu() -> GpuInfo {
        let mut gpu = make_nvidia_gpu();
        gpu.uuid = "GPU-AMD".to_string();
        gpu.name = "Radeon RX".to_string();
        gpu.numa_node_id = None;
        gpu.gsp_firmware_mode = None;
        gpu.gsp_firmware_version = None;
        gpu.nvlink_remote_devices = Vec::new();
        gpu.gpm_metrics = None;
        gpu
    }

    #[test]
    fn empty_when_no_hardware_details_anywhere() {
        let gpus = vec![make_non_nvidia_gpu()];
        let exporter = HardwareMetricExporter::new(&gpus);
        assert_eq!(exporter.export_metrics(), "");
    }

    #[test]
    fn emits_numa_node_id_when_populated() {
        let gpus = vec![make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(out.contains("# HELP all_smi_gpu_numa_node_id"));
        assert!(out.contains("# TYPE all_smi_gpu_numa_node_id gauge"));
        // Pattern: `all_smi_gpu_numa_node_id{...} 0`
        assert!(
            out.lines()
                .any(|l| l.starts_with("all_smi_gpu_numa_node_id{") && l.ends_with(" 0")),
            "missing numa node id line in:\n{out}"
        );
    }

    #[test]
    fn omits_numa_node_id_when_none() {
        let mut gpu = make_nvidia_gpu();
        gpu.numa_node_id = None;
        let gpus = vec![gpu];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(
            !out.contains("all_smi_gpu_numa_node_id"),
            "should omit numa metric entirely:\n{out}"
        );
    }

    #[test]
    fn emits_gsp_firmware_mode_and_version() {
        let gpus = vec![make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(out.contains("all_smi_gpu_gsp_firmware_mode{"));
        assert!(out.contains("all_smi_gpu_gsp_firmware_version_info{"));
        assert!(out.contains(r#"version="550.54.15""#));
    }

    #[test]
    fn emits_one_nvlink_row_per_active_link() {
        let gpus = vec![make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        let nvlink_lines: Vec<_> = out
            .lines()
            .filter(|l| l.starts_with("all_smi_nvlink_remote_device_type{"))
            .collect();
        assert_eq!(nvlink_lines.len(), 2, "expected 2 NvLink rows:\n{out}");
        assert!(
            nvlink_lines
                .iter()
                .any(|l| l.contains(r#"remote_type="gpu""#))
        );
        assert!(
            nvlink_lines
                .iter()
                .any(|l| l.contains(r#"remote_type="switch""#))
        );
        assert!(nvlink_lines.iter().any(|l| l.contains(r#"link_index="0""#)));
        assert!(nvlink_lines.iter().any(|l| l.contains(r#"link_index="1""#)));
    }

    #[test]
    fn emits_no_nvlink_metric_when_vector_empty() {
        let mut gpu = make_nvidia_gpu();
        gpu.nvlink_remote_devices.clear();
        // But keep NUMA so the row survives collect_rows filter.
        let gpus = vec![gpu];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(
            !out.contains("all_smi_nvlink_remote_device_type"),
            "should omit NvLink metric entirely when no active links:\n{out}"
        );
    }

    #[test]
    fn emits_gpm_metrics_when_populated() {
        let gpus = vec![make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(out.contains("all_smi_gpu_sm_occupancy{"));
        assert!(out.contains("all_smi_gpu_memory_bandwidth_utilization{"));
        assert!(
            out.contains(r#"source="gpm""#),
            "GPM series must carry source label:\n{out}"
        );
    }

    #[test]
    fn omits_gpm_metrics_when_unsupported_device() {
        let mut gpu = make_nvidia_gpu();
        gpu.gpm_metrics = None;
        let gpus = vec![gpu];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(!out.contains("all_smi_gpu_sm_occupancy"));
        assert!(!out.contains("all_smi_gpu_memory_bandwidth_utilization"));
    }

    #[test]
    fn gpm_supported_but_unsampled_emits_nothing_for_gpm_values() {
        // First poll / empty snapshot must not publish zeros.
        let mut gpu = make_nvidia_gpu();
        gpu.gpm_metrics = Some(GpmMetrics::default());
        let gpus = vec![gpu];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        assert!(
            !out.contains("all_smi_gpu_sm_occupancy"),
            "unsampled GPM must not emit zero:\n{out}"
        );
        assert!(
            !out.contains("all_smi_gpu_memory_bandwidth_utilization"),
            "unsampled GPM must not emit zero:\n{out}"
        );
        assert!(
            !out.contains("all_smi_gpu_sm_active_ratio"),
            "unsampled GPM must not emit zero:\n{out}"
        );
    }

    #[test]
    fn emits_full_gpm_field_set() {
        let mut gpu = make_nvidia_gpu();
        gpu.gpm_metrics = Some(GpmMetrics {
            graphics_active: Some(0.90),
            sm_active: Some(0.10),
            sm_occupancy: Some(0.05),
            tensor_active: Some(0.02),
            tensor_hmma_active: Some(0.01),
            tensor_imma_active: Some(0.0),
            tensor_dfma_active: Some(0.0),
            fp64_active: Some(0.0),
            fp32_active: Some(0.08),
            fp16_active: Some(0.01),
            integer_active: Some(0.0),
            memory_bandwidth_utilization: Some(0.20),
            pcie_tx_bytes_per_sec: Some(1.2e9),
            pcie_rx_bytes_per_sec: Some(3.0e8),
            nvlink_tx_bytes_per_sec: Some(4.0e9),
            nvlink_rx_bytes_per_sec: Some(4.1e9),
            nvdec_active: Some(0.0),
            nvjpg_active: Some(0.0),
            nvofa_active: Some(0.0),
            source: Some(TelemetrySource::Gpm),
        });
        let out = HardwareMetricExporter::new(&[gpu]).export_metrics();
        for name in [
            "all_smi_gpu_graphics_active_ratio",
            "all_smi_gpu_sm_active_ratio",
            "all_smi_gpu_tensor_active_ratio",
            "all_smi_gpu_pcie_tx_bytes_per_second",
            "all_smi_gpu_nvlink_rx_bytes_per_second",
            "all_smi_gpu_nvdec_active_ratio",
        ] {
            assert!(
                out.contains(&format!("{name}{{")),
                "missing {name} in:\n{out}"
            );
        }
    }

    #[test]
    fn preserves_standard_gpu_labels_on_all_metrics() {
        let gpus = vec![make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        // Every emitted line must carry the canonical GPU label set so
        // operators can correlate hardware-detail rows with basic util /
        // temperature series by identical labels.
        let expected_fragments = [
            r#"gpu="NVIDIA A100""#,
            r#"instance="node-1""#,
            r#"gpu_uuid="GPU-ABC""#,
            r#"gpu_index="0""#,
        ];
        for line in out
            .lines()
            .filter(|l| l.starts_with("all_smi_") && l.contains('{'))
        {
            for frag in &expected_fragments {
                assert!(line.contains(frag), "label '{frag}' missing from {line}");
            }
        }
    }

    #[test]
    fn skips_non_nvidia_rows_when_nvidia_row_also_present() {
        let gpus = vec![make_non_nvidia_gpu(), make_nvidia_gpu()];
        let out = HardwareMetricExporter::new(&gpus).export_metrics();
        // Non-NVIDIA GPU has UUID "GPU-AMD" — it must not appear in the
        // exposition since it populated no hardware-detail fields.
        assert!(
            !out.contains("GPU-AMD"),
            "non-NVIDIA GPU leaked into hardware exporter:\n{out}"
        );
        // NVIDIA GPU must remain observable.
        assert!(out.contains("GPU-ABC"));
    }
}
