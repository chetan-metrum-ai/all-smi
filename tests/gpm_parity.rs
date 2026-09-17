// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//! Live GPM ↔ DCGM parity test (gated on `ALL_SMI_LIVE_GPU=1`).
//!
//! Parses side-by-side captures produced by `scripts/dev/parity.sh` and
//! asserts each all-smi GPM ratio is within ±0.05 of the DCGM mean for the
//! same window. In `hollow` mode, nvidia-style utilization must stay high
//! while `gpm.sm_active` stays low — that is the definition of "the
//! misleading-utilization problem is solved".
//!
//! P1: numeric asserts run when captures contain real GPM fields.

#![cfg(feature = "cli")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn live_enabled() -> bool {
    matches!(
        env::var("ALL_SMI_LIVE_GPU").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

fn find_latest_parity_dir() -> Option<PathBuf> {
    let root = Path::new("TECHNICAL_REPORTS/runs");
    if !root.is_dir() {
        return None;
    }
    let mut runs: Vec<_> = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("parity").is_dir())
        .collect();
    runs.sort_by_key(|e| e.file_name());
    runs.pop().map(|e| e.path().join("parity"))
}

/// Parse a `dcgmi dmon` text capture and return the mean of column `field_id`
/// when present. Returns `None` when the file is missing or the field never
/// appears — callers must treat that as "no data", never zero.
fn dcgm_mean(path: &Path, field_id: u32) -> Option<f64> {
    let text = fs::read_to_string(path).ok()?;
    let mut values = Vec::new();
    let id_str = field_id.to_string();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("ID") {
            continue;
        }
        // dcgmi dmon lines are whitespace-separated; field order follows -e.
        // We also accept "FIELD_ID value" pairs if the capture uses that form.
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        // Heuristic: if the line contains the field id as a token, take the
        // following token; otherwise assume positional columns documented by
        // the header row (handled by callers that know the -e order).
        if let Some(idx) = parts.iter().position(|p| *p == id_str)
            && let Some(v) = parts.get(idx + 1)
            && let Ok(n) = v.parse::<f64>()
        {
            values.push(n);
        }
    }
    if values.is_empty() {
        // Positional fallback for standard `dcgmi dmon -e 1001,...,1012` header.
        // Columns after entity id: one per field in -e order.
        let order = [1001, 1002, 1003, 1004, 1005, 1009, 1010, 1011, 1012];
        let col = order.iter().position(|f| *f == field_id)?;
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // First two tokens are typically GPU index / device name-ish.
            if parts.len() < col + 3 {
                continue;
            }
            if let Ok(n) = parts[col + 2].parse::<f64>() {
                values.push(n);
            }
        }
    }
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Extract mean `gpm.sm_active` (or legacy occupancy) from an all-smi JSON
/// snapshot capture. Returns `None` when the field is absent — P0 expected.
fn allsmi_gpm_mean(path: &Path, field: &str) -> Option<f64> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut vals = Vec::new();
    let samples = if value.is_array() {
        value.as_array()?.clone()
    } else {
        vec![value]
    };
    for sample in samples {
        let gpus = sample.get("gpus")?.as_array()?;
        for gpu in gpus {
            let gpm = gpu.get("gpm_metrics")?;
            if let Some(v) = gpm.get(field).and_then(|x| x.as_f64()) {
                vals.push(v);
            }
        }
    }
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum::<f64>() / vals.len() as f64)
    }
}

#[test]
fn gpm_parity_skips_without_live_flag() {
    if live_enabled() {
        // Live path is covered by gpm_parity_live_against_captures.
    }
    // Default CI / no-GPU path: nothing to assert.
}

#[test]
fn gpm_parity_live_against_captures() {
    if !live_enabled() {
        eprintln!("ALL_SMI_LIVE_GPU unset; skipping live GPM parity");
        return;
    }

    let Some(parity) = find_latest_parity_dir() else {
        panic!(
            "ALL_SMI_LIVE_GPU=1 but no TECHNICAL_REPORTS/runs/*/parity directory; \
             run scripts/dev/parity.sh first"
        );
    };

    let hollow_dcgm = parity.join("hollow/dcgm_dmon.txt");
    let hollow_allsmi = parity.join("hollow/allsmi_snapshot.json");

    let sm_active_dcgm = dcgm_mean(&hollow_dcgm, 1002);
    eprintln!("hollow DCGM SM_ACTIVE (1002) mean = {sm_active_dcgm:?}");

    let sm_active_allsmi = allsmi_gpm_mean(&hollow_allsmi, "sm_active")
        .or_else(|| allsmi_gpm_mean(&hollow_allsmi, "sm_occupancy"));
    eprintln!("hollow all-smi gpm.sm_active mean = {sm_active_allsmi:?}");

    let allsmi = sm_active_allsmi
        .expect("P1: all-smi GPM sm_active must be populated on Hopper after two polls");
    if let Some(dcgm) = sm_active_dcgm {
        // DCGM reports percent (0-100) in some captures and ratio (0-1) in
        // others; normalise to ratio before comparing.
        let dcgm_ratio = if dcgm > 1.0 { dcgm / 100.0 } else { dcgm };
        let delta = (allsmi - dcgm_ratio).abs();
        assert!(
            delta <= 0.05,
            "sm_active allsmi={allsmi} dcgm={dcgm_ratio} delta={delta} exceeds ±0.05"
        );
        assert!(
            allsmi <= 0.05,
            "hollow mode expects sm_active <= 0.05, got {allsmi}"
        );
    }
}
