// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//! Runtime loader for the Linux DCGM companion backend.
//!
//! The main crate never links `libdcgm`. It loads a packaged
//! `liball_smi_dcgm.so` through a versioned function table and treats every
//! lookup / ABI / sampling failure as backend unavailability (no panic).

use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::device::GpuInfo;
use crate::device::readers::dcgm_plugin_api::{
    DCGM_PLUGIN_ABI_VERSION, DCGM_PLUGIN_ENTRY_SYMBOL, DCGM_PLUGIN_WIRE_FORMAT, DcgmPluginApiV1,
    DcgmPluginBuffer,
};
use crate::device::types::{GpmMetrics, RemappedRowsInfo, TelemetrySource, ThrottleReasons};

pub const DCGM_PLUGIN_ENV: &str = "ALL_SMI_DCGM_PLUGIN";
pub const DCGM_PLUGIN_FILENAME: &str = "liball_smi_dcgm.so";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DcgmBackendStatus {
    Loaded {
        path: PathBuf,
        abi_version: u32,
        plugin_version: String,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Deserialize)]
struct PluginMetadata {
    plugin_version: String,
    wire_format: String,
}

/// One GPU's DCGM sample as returned by the plugin JSON wire format.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DcgmGpuSample {
    pub uuid: String,
    #[serde(default)]
    pub graphics_active: Option<f32>,
    #[serde(default)]
    pub sm_active: Option<f32>,
    #[serde(default)]
    pub sm_occupancy: Option<f32>,
    #[serde(default)]
    pub tensor_active: Option<f32>,
    #[serde(default)]
    pub fp64_active: Option<f32>,
    #[serde(default)]
    pub fp32_active: Option<f32>,
    #[serde(default)]
    pub fp16_active: Option<f32>,
    #[serde(default)]
    pub memory_bandwidth_utilization: Option<f32>,
    #[serde(default)]
    pub pcie_tx_bytes_per_sec: Option<f64>,
    #[serde(default)]
    pub pcie_rx_bytes_per_sec: Option<f64>,
    #[serde(default)]
    pub nvlink_tx_bytes_per_sec: Option<f64>,
    #[serde(default)]
    pub nvlink_rx_bytes_per_sec: Option<f64>,
    #[serde(default)]
    pub throttle_reasons: Option<ThrottleReasons>,
    #[serde(default)]
    pub remapped_rows: Option<RemappedRowsInfo>,
    #[serde(default)]
    pub xid_event_counts: HashMap<u32, u64>,
}

#[derive(Deserialize)]
struct SampleEnvelope {
    gpus: Vec<DcgmGpuSample>,
}

struct LoadedPlugin {
    _library: Library,
    api: DcgmPluginApiV1,
    path: PathBuf,
    metadata: PluginMetadata,
    session: Mutex<PluginSession>,
}

struct PluginSession {
    handle: *mut c_void,
    create_backoff: CreateSessionBackoff,
}

impl Default for PluginSession {
    fn default() -> Self {
        Self {
            handle: std::ptr::null_mut(),
            create_backoff: CreateSessionBackoff::default(),
        }
    }
}

#[derive(Default)]
struct CreateSessionBackoff {
    consecutive_failures: u32,
    retry_at: Option<Instant>,
}

impl CreateSessionBackoff {
    fn retry_due_at(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|retry_at| now >= retry_at)
    }

    fn record_failure_at(&mut self, now: Instant) -> Duration {
        let delay = Self::delay_for_failures(self.consecutive_failures);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.retry_at = Some(now + delay);
        delay
    }

    fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.retry_at = None;
    }

    fn delay_for_failures(consecutive_failures: u32) -> Duration {
        let seconds = match consecutive_failures.min(6) {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            4 => 16,
            5 => 32,
            _ => 60,
        };
        Duration::from_secs(seconds)
    }
}

// SAFETY: session pointer and backoff state are only accessed under the Mutex;
// the plugin library outlives every session handle.
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

static PLUGIN: OnceLock<Result<Arc<LoadedPlugin>, String>> = OnceLock::new();

fn plugin() -> Result<Arc<LoadedPlugin>, String> {
    PLUGIN.get_or_init(load_plugin).clone()
}

#[allow(dead_code)]
pub fn backend_status() -> DcgmBackendStatus {
    match plugin() {
        Ok(plugin) => DcgmBackendStatus::Loaded {
            path: plugin.path.clone(),
            abi_version: plugin.api.abi_version,
            plugin_version: plugin.metadata.plugin_version.clone(),
        },
        Err(reason) => DcgmBackendStatus::Unavailable { reason },
    }
}

/// Sample DCGM fields and merge into existing GPU rows by UUID.
///
/// Precedence per scalar: keep existing GPM/NVML `Some`; fill only `None`
/// from DCGM and record `Source: <field>=dcgm` in `detail`.
pub fn merge_into_gpus(gpus: &mut [GpuInfo]) {
    let Ok(plugin) = plugin() else {
        return;
    };
    let Ok(samples) = sample_map(&plugin) else {
        return;
    };
    for gpu in gpus.iter_mut() {
        let Some(sample) = samples.get(&gpu.uuid) else {
            continue;
        };
        merge_sample(gpu, sample);
    }
}

fn sample_map(plugin: &LoadedPlugin) -> Result<HashMap<String, DcgmGpuSample>, String> {
    let mut session = plugin
        .session
        .lock()
        .map_err(|_| "dcgm session lock poisoned")?;
    if session.handle.is_null() {
        let now = Instant::now();
        if !session.create_backoff.retry_due_at(now) {
            let retry_in_ms = session
                .create_backoff
                .retry_at
                .map(|retry_at| retry_at.saturating_duration_since(now).as_millis())
                .unwrap_or_default();
            tracing::debug!(retry_in_ms, "skipping DCGM session create during backoff");
            return Err("dcgm create_session retry deferred by backoff".into());
        }
        tracing::debug!(
            consecutive_failures = session.create_backoff.consecutive_failures,
            "attempting DCGM hostengine session"
        );
        let create = plugin
            .api
            .create_session
            .ok_or_else(|| "dcgm create_session missing".to_string())?;
        // SAFETY: create_session is validated; returns null on failure.
        let handle = unsafe { create() };
        if handle.is_null() {
            let delay = session.create_backoff.record_failure_at(Instant::now());
            tracing::debug!(
                retry_after_secs = delay.as_secs(),
                "DCGM session create failed"
            );
            return Err(format!(
                "dcgm create_session returned null (retry in {}s)",
                delay.as_secs()
            ));
        }
        session.handle = handle;
        tracing::debug!("DCGM hostengine session created");
    }
    let handle = session.handle;
    let envelope: SampleEnvelope = match read_buffer(&plugin.api, |out| {
        let sample = plugin.api.sample_json.expect("validated function pointer");
        // SAFETY: handle owned by this plugin; out points to live buffer desc.
        unsafe { sample(handle, out) }
    }) {
        Ok(envelope) => envelope,
        Err(error) => {
            let destroy = plugin
                .api
                .destroy_session
                .expect("validated function pointer");
            // SAFETY: handle was created by this plugin and is still owned by
            // this session. A failed sample may mean hostengine lost it.
            unsafe { destroy(handle) };
            session.handle = std::ptr::null_mut();
            let delay = session.create_backoff.record_failure_at(Instant::now());
            tracing::warn!(
                error = %error,
                retry_after_secs = delay.as_secs(),
                "DCGM sample failed; discarded session and will reconnect"
            );
            return Err(format!(
                "dcgm sample failed; discarded session (retry in {}s): {error}",
                delay.as_secs()
            ));
        }
    };
    if session.create_backoff.consecutive_failures > 0 {
        session.create_backoff.reset();
        tracing::debug!("DCGM sample succeeded; retry backoff reset");
    }
    Ok(envelope
        .gpus
        .into_iter()
        .map(|g| (g.uuid.clone(), g))
        .collect())
}

fn merge_sample(gpu: &mut GpuInfo, sample: &DcgmGpuSample) {
    let metrics = gpu.gpm_metrics.get_or_insert_with(GpmMetrics::default);
    let mut filled_any = false;
    filled_any |= fill_f32(
        &mut metrics.graphics_active,
        sample.graphics_active,
        &mut gpu.detail,
        "graphics_active",
    );
    filled_any |= fill_f32(
        &mut metrics.sm_active,
        sample.sm_active,
        &mut gpu.detail,
        "sm_active",
    );
    filled_any |= fill_f32(
        &mut metrics.sm_occupancy,
        sample.sm_occupancy,
        &mut gpu.detail,
        "sm_occupancy",
    );
    filled_any |= fill_f32(
        &mut metrics.tensor_active,
        sample.tensor_active,
        &mut gpu.detail,
        "tensor_active",
    );
    filled_any |= fill_f32(
        &mut metrics.fp64_active,
        sample.fp64_active,
        &mut gpu.detail,
        "fp64_active",
    );
    filled_any |= fill_f32(
        &mut metrics.fp32_active,
        sample.fp32_active,
        &mut gpu.detail,
        "fp32_active",
    );
    filled_any |= fill_f32(
        &mut metrics.fp16_active,
        sample.fp16_active,
        &mut gpu.detail,
        "fp16_active",
    );
    filled_any |= fill_f32(
        &mut metrics.memory_bandwidth_utilization,
        sample.memory_bandwidth_utilization,
        &mut gpu.detail,
        "memory_bandwidth_utilization",
    );
    filled_any |= fill_f64(
        &mut metrics.pcie_tx_bytes_per_sec,
        sample.pcie_tx_bytes_per_sec,
        &mut gpu.detail,
        "pcie_tx",
    );
    filled_any |= fill_f64(
        &mut metrics.pcie_rx_bytes_per_sec,
        sample.pcie_rx_bytes_per_sec,
        &mut gpu.detail,
        "pcie_rx",
    );
    filled_any |= fill_f64(
        &mut metrics.nvlink_tx_bytes_per_sec,
        sample.nvlink_tx_bytes_per_sec,
        &mut gpu.detail,
        "nvlink_tx",
    );
    filled_any |= fill_f64(
        &mut metrics.nvlink_rx_bytes_per_sec,
        sample.nvlink_rx_bytes_per_sec,
        &mut gpu.detail,
        "nvlink_rx",
    );
    if filled_any {
        // Prefer an explicit DCGM source when we contributed ratio fields and
        // GPM did not already claim the snapshot.
        if metrics.source != Some(TelemetrySource::Gpm) {
            metrics.source = Some(TelemetrySource::Dcgm);
        }
    }

    if gpu.throttle_reasons.is_none()
        && let Some(t) = sample.throttle_reasons
    {
        gpu.throttle_reasons = Some(t);
        gpu.detail.insert("Source: throttle".into(), "dcgm".into());
    }
    if gpu.remapped_rows.is_none()
        && let Some(r) = sample.remapped_rows
    {
        gpu.remapped_rows = Some(r);
        gpu.detail
            .insert("Source: remapped_rows".into(), "dcgm".into());
    }
    for (&code, &count) in &sample.xid_event_counts {
        let entry = gpu.xid_event_counts.entry(code).or_insert(0);
        if *entry < count {
            *entry = count;
            gpu.detail
                .insert(format!("Source: xid_{code}"), "dcgm".into());
        }
    }
}

fn fill_f32(
    dst: &mut Option<f32>,
    src: Option<f32>,
    detail: &mut HashMap<String, String>,
    field: &str,
) -> bool {
    if dst.is_some() {
        return false;
    }
    let Some(v) = src else {
        return false;
    };
    *dst = Some(v);
    detail.insert(format!("Source: {field}"), "dcgm".into());
    true
}

fn fill_f64(
    dst: &mut Option<f64>,
    src: Option<f64>,
    detail: &mut HashMap<String, String>,
    field: &str,
) -> bool {
    if dst.is_some() {
        return false;
    }
    let Some(v) = src else {
        return false;
    };
    *dst = Some(v);
    detail.insert(format!("Source: {field}"), "dcgm".into());
    true
}

fn load_plugin() -> Result<Arc<LoadedPlugin>, String> {
    let candidates = configured_candidates()?;
    let mut failures = Vec::new();
    for candidate in candidates {
        if !candidate.exists() {
            failures.push(format!("{}: not found", candidate.display()));
            continue;
        }
        let path = match validate_candidate(&candidate) {
            Ok(path) => path,
            Err(error) => {
                failures.push(error);
                continue;
            }
        };
        match load_candidate(path.clone()) {
            Ok(plugin) => return Ok(Arc::new(plugin)),
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    Err(format!(
        "DCGM plugin unavailable; {}. Install {DCGM_PLUGIN_FILENAME} beside the executable or in /usr/lib/all-smi, or set {DCGM_PLUGIN_ENV} to a safe absolute path",
        failures.join("; ")
    ))
}

fn configured_candidates() -> Result<Vec<PathBuf>, String> {
    if let Some(override_path) = std::env::var_os(DCGM_PLUGIN_ENV) {
        if override_path.is_empty() {
            return Err(format!("{DCGM_PLUGIN_ENV} is set but empty"));
        }
        let path = PathBuf::from(override_path);
        if !path.is_absolute() {
            return Err(format!(
                "{DCGM_PLUGIN_ENV} must be an absolute path, got {}",
                path.display()
            ));
        }
        return Ok(vec![path]);
    }
    let executable = std::env::current_exe().map_err(|error| {
        format!("cannot resolve the current executable for DCGM plugin lookup: {error}")
    })?;
    Ok(default_candidates(&executable))
}

fn default_candidates(executable: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(bin_dir) = executable.parent() {
        candidates.push(bin_dir.join(DCGM_PLUGIN_FILENAME));
        candidates.push(
            bin_dir
                .join("..")
                .join("lib")
                .join("all-smi")
                .join(DCGM_PLUGIN_FILENAME),
        );
    }
    candidates.push(PathBuf::from("/usr/local/lib/all-smi").join(DCGM_PLUGIN_FILENAME));
    candidates.push(PathBuf::from("/usr/lib/all-smi").join(DCGM_PLUGIN_FILENAME));
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn validate_candidate(candidate: &Path) -> Result<PathBuf, String> {
    if !candidate.is_absolute() {
        return Err(format!(
            "{}: plugin path is not absolute",
            candidate.display()
        ));
    }
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "{}: cannot canonicalize plugin path: {error}",
            candidate.display()
        )
    })?;
    if !canonical.is_file() {
        return Err(format!(
            "{}: plugin path is not a regular file",
            canonical.display()
        ));
    }
    reject_world_writable_path(&canonical)?;
    Ok(canonical)
}

#[cfg(unix)]
fn reject_world_writable_path(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    for component in path.ancestors() {
        let metadata = component.metadata().map_err(|error| {
            format!(
                "{}: cannot inspect permissions: {error}",
                component.display()
            )
        })?;
        if metadata.permissions().mode() & 0o002 != 0 {
            return Err(format!(
                "{}: refusing DCGM plugin because {} is world-writable",
                path.display(),
                component.display()
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_world_writable_path(_path: &Path) -> Result<(), String> {
    Ok(())
}

type EntryV1 = unsafe extern "C" fn() -> *const DcgmPluginApiV1;

fn load_candidate(path: PathBuf) -> Result<LoadedPlugin, String> {
    // SAFETY: path is absolute, canonicalized, and checked for world-writable
    // ancestors. The library outlives every copied function pointer.
    let library =
        unsafe { Library::new(&path) }.map_err(|error| format!("dlopen failed: {error}"))?;
    let api = {
        // SAFETY: symbol is NUL-terminated and matches the versioned ABI.
        let entry = unsafe { library.get::<EntryV1>(DCGM_PLUGIN_ENTRY_SYMBOL) }
            .map_err(|error| format!("missing v1 entry point: {error}"))?;
        // SAFETY: entry has no arguments; returns immutable static storage.
        let api = unsafe { entry() };
        if api.is_null() {
            return Err("v1 entry point returned a null function table".to_string());
        }
        // SAFETY: header fields are always present at the start of the table.
        let abi_version = unsafe { std::ptr::addr_of!((*api).abi_version).read() };
        let struct_size = unsafe { std::ptr::addr_of!((*api).struct_size).read() };
        validate_api_header(abi_version, struct_size)?;
        // SAFETY: validated struct_size covers the complete v1 table.
        unsafe { api.read() }
    };
    validate_api(&api)?;
    let metadata: PluginMetadata = read_buffer(&api, |out| {
        // SAFETY: validated table; out is live for this call.
        unsafe { (api.read_metadata_json.expect("validated function pointer"))(out) }
    })?;
    validate_metadata(&metadata)?;
    Ok(LoadedPlugin {
        _library: library,
        api,
        path,
        metadata,
        session: Mutex::new(PluginSession::default()),
    })
}

fn validate_api_header(abi_version: u32, struct_size: usize) -> Result<(), String> {
    if abi_version != DCGM_PLUGIN_ABI_VERSION {
        return Err(format!(
            "unsupported DCGM plugin ABI version {abi_version} (want {DCGM_PLUGIN_ABI_VERSION})"
        ));
    }
    if struct_size < std::mem::size_of::<DcgmPluginApiV1>() {
        return Err(format!(
            "DCGM plugin table too small ({struct_size} < {})",
            std::mem::size_of::<DcgmPluginApiV1>()
        ));
    }
    Ok(())
}

fn validate_api(api: &DcgmPluginApiV1) -> Result<(), String> {
    if api.create_session.is_none()
        || api.destroy_session.is_none()
        || api.sample_json.is_none()
        || api.read_metadata_json.is_none()
        || api.free_buffer.is_none()
    {
        return Err("DCGM plugin table is missing one or more required function pointers".into());
    }
    Ok(())
}

fn validate_metadata(metadata: &PluginMetadata) -> Result<(), String> {
    if metadata.wire_format != DCGM_PLUGIN_WIRE_FORMAT {
        return Err(format!(
            "unexpected DCGM wire format {:?} (want {DCGM_PLUGIN_WIRE_FORMAT})",
            metadata.wire_format
        ));
    }
    Ok(())
}

fn read_buffer<T: DeserializeOwned>(
    api: &DcgmPluginApiV1,
    fill: impl FnOnce(*mut DcgmPluginBuffer) -> i32,
) -> Result<T, String> {
    let mut buffer = DcgmPluginBuffer::default();
    let status = fill(&mut buffer);
    if status != 0 {
        free_buffer(api, &mut buffer);
        return Err(format!(
            "DCGM plugin buffer call failed with status {status}"
        ));
    }
    if buffer.ptr.is_null() || buffer.len == 0 {
        free_buffer(api, &mut buffer);
        return Err("DCGM plugin returned an empty buffer".into());
    }
    // SAFETY: plugin allocated the buffer; we free it after copying.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) };
    let parsed = serde_json::from_slice(bytes).map_err(|e| format!("DCGM JSON parse: {e}"));
    free_buffer(api, &mut buffer);
    parsed
}

fn free_buffer(api: &DcgmPluginApiV1, buffer: &mut DcgmPluginBuffer) {
    if let Some(free) = api.free_buffer {
        // SAFETY: buffer was produced by this plugin's allocator.
        unsafe { free(buffer) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_backoff_doubles_and_caps_at_sixty_seconds() {
        let actual: Vec<_> = (0..9)
            .map(|failures| CreateSessionBackoff::delay_for_failures(failures).as_secs())
            .collect();

        assert_eq!(actual, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn create_session_backoff_respects_deadline_and_resets_on_success() {
        let start = Instant::now();
        let mut backoff = CreateSessionBackoff::default();

        assert!(backoff.retry_due_at(start));
        assert_eq!(backoff.record_failure_at(start), Duration::from_secs(1));
        assert!(!backoff.retry_due_at(start + Duration::from_millis(999)));
        assert!(backoff.retry_due_at(start + Duration::from_secs(1)));

        backoff.record_failure_at(start + Duration::from_secs(1));
        assert_eq!(backoff.retry_at, Some(start + Duration::from_secs(3)));
        backoff.reset();
        assert!(backoff.retry_due_at(start + Duration::from_secs(1)));
        assert_eq!(backoff.consecutive_failures, 0);
        assert_eq!(backoff.retry_at, None);
    }

    #[test]
    fn unavailable_plugin_does_not_panic_on_merge() {
        // Ensure merge is a no-op when the plugin is missing (common on
        // developer machines and musl). Must not panic.
        let mut gpus: Vec<GpuInfo> = Vec::new();
        merge_into_gpus(&mut gpus);
        let _ = backend_status();
    }

    #[test]
    fn fill_preserves_existing_some() {
        let mut dst = Some(0.5_f32);
        let mut detail = HashMap::new();
        assert!(!fill_f32(&mut dst, Some(0.1), &mut detail, "sm_active"));
        assert_eq!(dst, Some(0.5));
        assert!(detail.is_empty());
    }

    #[test]
    fn fill_sets_none_from_dcgm() {
        let mut dst = None;
        let mut detail = HashMap::new();
        assert!(fill_f32(&mut dst, Some(0.1), &mut detail, "sm_active"));
        assert_eq!(dst, Some(0.1));
        assert_eq!(
            detail.get("Source: sm_active").map(String::as_str),
            Some("dcgm")
        );
    }
}
