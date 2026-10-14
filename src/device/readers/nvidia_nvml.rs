// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! Process-wide NVML lifetime management.
//!
//! `nvml-wrapper::Nvml::drop` calls `nvmlShutdown`, which invalidates NVML
//! device handles and GPM samples held elsewhere in the process. Keep the
//! successful handle alive for the process lifetime so one AllSmi reader (or
//! diagnostic probe) cannot shut NVML down underneath another.

use nvml_wrapper::Nvml;
use nvml_wrapper::error::NvmlError;
use std::sync::{Mutex, OnceLock};

static NVML: OnceLock<&'static Nvml> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());
static API_LOCK: Mutex<()> = Mutex::new(());

/// Return the process-wide NVML handle, initializing it once on first use.
///
/// Failed initialization is not cached: callers can retry if the driver or
/// device becomes available later. A successful handle is intentionally
/// leaked because `Nvml::drop` invokes the process-global `nvmlShutdown`.
pub(crate) fn shared_nvml() -> Result<&'static Nvml, NvmlError> {
    if let Some(nvml) = NVML.get() {
        return Ok(nvml);
    }

    let _guard = INIT_LOCK.lock().map_err(|_| NvmlError::Unknown)?;
    if let Some(nvml) = NVML.get() {
        return Ok(nvml);
    }

    let nvml = Box::leak(Box::new(Nvml::init()?));
    Ok(*NVML.get_or_init(|| nvml))
}

/// Run a group of NVML calls with process-wide serialization.
///
/// NVML itself supports concurrent ordinary calls, but GPM sample operations
/// can crash when interleaved with the library's other NVML queries. Holding
/// this lock across each complete reader operation also coordinates the
/// background event watcher and diagnostic GPM probes.
pub(crate) fn with_shared_nvml<F, T>(f: F) -> Result<T, NvmlError>
where
    F: FnOnce(&'static Nvml) -> T,
{
    let nvml = shared_nvml()?;
    let _guard = API_LOCK.lock().map_err(|_| NvmlError::Unknown)?;
    Ok(f(nvml))
}
