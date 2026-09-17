// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//! DCGM sampling session: dlopen `libdcgm.so.4`, connect to hostengine
//! (fallback embedded), watch PROF_* + XID/remap/throttle fields, sample.

#![cfg(all(target_os = "linux", not(target_env = "musl")))]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_longlong, c_uint, c_ushort, c_void};

use all_smi::device::types::{RemappedRowsInfo, ThrottleReasons};
use libloading::{Library, Symbol};
use serde::Serialize;

use crate::bindings::{
    self, DCGM_CLOCKS_EVENT_REASON_CLOCKS_SETTING, DCGM_CLOCKS_EVENT_REASON_DISPLAY_CLOCKS,
    DCGM_CLOCKS_EVENT_REASON_GPU_IDLE, DCGM_CLOCKS_EVENT_REASON_HW_POWER_BRAKE,
    DCGM_CLOCKS_EVENT_REASON_HW_SLOWDOWN, DCGM_CLOCKS_EVENT_REASON_HW_THERMAL,
    DCGM_CLOCKS_EVENT_REASON_SW_POWER_CAP, DCGM_CLOCKS_EVENT_REASON_SW_THERMAL,
    DCGM_CLOCKS_EVENT_REASON_SYNC_BOOST, DCGM_FI_DEV_CLOCK_THROTTLE_REASONS,
    DCGM_FI_DEV_ROW_REMAP_CORRECTABLE_TOTAL, DCGM_FI_DEV_ROW_REMAP_FAILED,
    DCGM_FI_DEV_ROW_REMAP_PENDING, DCGM_FI_DEV_ROW_REMAP_UNCORRECTABLE_TOTAL, DCGM_FI_DEV_UUID,
    DCGM_FI_DEV_XID_ERRORS, DCGM_FI_PROF_DRAM_ACTIVE, DCGM_FI_PROF_GR_ENGINE_ACTIVE,
    DCGM_FI_PROF_NVLINK_RX_BYTES, DCGM_FI_PROF_NVLINK_TX_BYTES, DCGM_FI_PROF_PCIE_RX_BYTES,
    DCGM_FI_PROF_PCIE_TX_BYTES, DCGM_FI_PROF_PIPE_FP16_ACTIVE, DCGM_FI_PROF_PIPE_FP32_ACTIVE,
    DCGM_FI_PROF_PIPE_FP64_ACTIVE, DCGM_FI_PROF_PIPE_TENSOR_ACTIVE, DCGM_FI_PROF_SM_ACTIVE,
    DCGM_FI_PROF_SM_OCCUPANCY, DCGM_FP64_BLANK, DCGM_FT_DOUBLE, DCGM_FT_INT64, DCGM_FT_STRING,
    DCGM_INT64_BLANK, DCGM_MAX_NUM_DEVICES, dcgmConnectV2Params_t, dcgmFieldValue_v1,
    dcgmGroupType_enum_DCGM_GROUP_DEFAULT, dcgmHandle_t,
    dcgmOperationMode_enum_DCGM_OPERATION_MODE_AUTO, dcgmReturn_enum_DCGM_ST_OK, dcgmReturn_t,
    make_dcgm_version,
};

const FIELD_IDS: &[u16] = &[
    DCGM_FI_DEV_UUID as u16,
    DCGM_FI_PROF_GR_ENGINE_ACTIVE as u16,
    DCGM_FI_PROF_SM_ACTIVE as u16,
    DCGM_FI_PROF_SM_OCCUPANCY as u16,
    DCGM_FI_PROF_PIPE_TENSOR_ACTIVE as u16,
    DCGM_FI_PROF_DRAM_ACTIVE as u16,
    DCGM_FI_PROF_PIPE_FP64_ACTIVE as u16,
    DCGM_FI_PROF_PIPE_FP32_ACTIVE as u16,
    DCGM_FI_PROF_PIPE_FP16_ACTIVE as u16,
    DCGM_FI_PROF_PCIE_TX_BYTES as u16,
    DCGM_FI_PROF_PCIE_RX_BYTES as u16,
    DCGM_FI_PROF_NVLINK_TX_BYTES as u16,
    DCGM_FI_PROF_NVLINK_RX_BYTES as u16,
    DCGM_FI_DEV_CLOCK_THROTTLE_REASONS as u16,
    DCGM_FI_DEV_ROW_REMAP_CORRECTABLE_TOTAL as u16,
    DCGM_FI_DEV_ROW_REMAP_UNCORRECTABLE_TOTAL as u16,
    DCGM_FI_DEV_ROW_REMAP_PENDING as u16,
    DCGM_FI_DEV_ROW_REMAP_FAILED as u16,
    DCGM_FI_DEV_XID_ERRORS as u16,
];

type FnInit = unsafe extern "C" fn() -> dcgmReturn_t;
type FnShutdown = unsafe extern "C" fn() -> dcgmReturn_t;
type FnConnectV2 = unsafe extern "C" fn(
    *const c_char,
    *mut dcgmConnectV2Params_t,
    *mut dcgmHandle_t,
) -> dcgmReturn_t;
type FnStartEmbedded = unsafe extern "C" fn(c_uint, *mut dcgmHandle_t) -> dcgmReturn_t;
type FnDisconnect = unsafe extern "C" fn(dcgmHandle_t) -> dcgmReturn_t;
type FnStopEmbedded = unsafe extern "C" fn(dcgmHandle_t) -> dcgmReturn_t;
type FnGetAllDevices = unsafe extern "C" fn(dcgmHandle_t, *mut c_uint, *mut c_int) -> dcgmReturn_t;
type FnGroupCreate =
    unsafe extern "C" fn(dcgmHandle_t, c_uint, *const c_char, *mut usize) -> dcgmReturn_t;
type FnGroupDestroy = unsafe extern "C" fn(dcgmHandle_t, usize) -> dcgmReturn_t;
type FnFieldGroupCreate = unsafe extern "C" fn(
    dcgmHandle_t,
    c_int,
    *const c_ushort,
    *const c_char,
    *mut usize,
) -> dcgmReturn_t;
type FnFieldGroupDestroy = unsafe extern "C" fn(dcgmHandle_t, usize) -> dcgmReturn_t;
type FnWatchFields =
    unsafe extern "C" fn(dcgmHandle_t, usize, usize, c_longlong, f64, c_int) -> dcgmReturn_t;
type FnUpdateAllFields = unsafe extern "C" fn(dcgmHandle_t, c_int) -> dcgmReturn_t;
type FnProfResume = unsafe extern "C" fn(dcgmHandle_t) -> dcgmReturn_t;
type FnGetLatest = unsafe extern "C" fn(
    dcgmHandle_t,
    c_int,
    *mut c_ushort,
    c_uint,
    *mut dcgmFieldValue_v1,
) -> dcgmReturn_t;

struct DcgmFns {
    _lib: Library,
    init: Symbol<'static, FnInit>,
    shutdown: Symbol<'static, FnShutdown>,
    connect_v2: Symbol<'static, FnConnectV2>,
    start_embedded: Symbol<'static, FnStartEmbedded>,
    disconnect: Symbol<'static, FnDisconnect>,
    stop_embedded: Symbol<'static, FnStopEmbedded>,
    get_all_devices: Symbol<'static, FnGetAllDevices>,
    group_create: Symbol<'static, FnGroupCreate>,
    group_destroy: Symbol<'static, FnGroupDestroy>,
    field_group_create: Symbol<'static, FnFieldGroupCreate>,
    field_group_destroy: Symbol<'static, FnFieldGroupDestroy>,
    watch_fields: Symbol<'static, FnWatchFields>,
    update_all_fields: Symbol<'static, FnUpdateAllFields>,
    get_latest: Symbol<'static, FnGetLatest>,
    prof_resume: Option<Symbol<'static, FnProfResume>>,
}

impl DcgmFns {
    unsafe fn load() -> Result<Self, String> {
        // SAFETY: libdcgm is a system shared library; symbols are resolved below.
        let lib = unsafe { Library::new("libdcgm.so.4").or_else(|_| Library::new("libdcgm.so")) }
            .map_err(|e| format!("dlopen libdcgm: {e}"))?;
        // SAFETY: symbols match DCGM 4.x ABI; we transmute lifetimes to
        // 'static by keeping Library owned in the same struct.
        unsafe {
            let init = std::mem::transmute::<Symbol<'_, FnInit>, Symbol<'static, FnInit>>(
                lib.get(b"dcgmInit\0").map_err(|e| e.to_string())?,
            );
            let shutdown = std::mem::transmute::<Symbol<'_, FnShutdown>, Symbol<'static, FnShutdown>>(
                lib.get(b"dcgmShutdown\0").map_err(|e| e.to_string())?,
            );
            let connect_v2 = std::mem::transmute::<
                Symbol<'_, FnConnectV2>,
                Symbol<'static, FnConnectV2>,
            >(lib.get(b"dcgmConnect_v2\0").map_err(|e| e.to_string())?);
            let start_embedded = std::mem::transmute::<
                Symbol<'_, FnStartEmbedded>,
                Symbol<'static, FnStartEmbedded>,
            >(
                lib.get(b"dcgmStartEmbedded\0").map_err(|e| e.to_string())?
            );
            let disconnect = std::mem::transmute::<
                Symbol<'_, FnDisconnect>,
                Symbol<'static, FnDisconnect>,
            >(lib.get(b"dcgmDisconnect\0").map_err(|e| e.to_string())?);
            let stop_embedded =
                std::mem::transmute::<Symbol<'_, FnStopEmbedded>, Symbol<'static, FnStopEmbedded>>(
                    lib.get(b"dcgmStopEmbedded\0").map_err(|e| e.to_string())?,
                );
            let get_all_devices = std::mem::transmute::<
                Symbol<'_, FnGetAllDevices>,
                Symbol<'static, FnGetAllDevices>,
            >(
                lib.get(b"dcgmGetAllDevices\0").map_err(|e| e.to_string())?
            );
            let group_create =
                std::mem::transmute::<Symbol<'_, FnGroupCreate>, Symbol<'static, FnGroupCreate>>(
                    lib.get(b"dcgmGroupCreate\0").map_err(|e| e.to_string())?,
                );
            let group_destroy =
                std::mem::transmute::<Symbol<'_, FnGroupDestroy>, Symbol<'static, FnGroupDestroy>>(
                    lib.get(b"dcgmGroupDestroy\0").map_err(|e| e.to_string())?,
                );
            let field_group_create = std::mem::transmute::<
                Symbol<'_, FnFieldGroupCreate>,
                Symbol<'static, FnFieldGroupCreate>,
            >(
                lib.get(b"dcgmFieldGroupCreate\0")
                    .map_err(|e| e.to_string())?,
            );
            let field_group_destroy = std::mem::transmute::<
                Symbol<'_, FnFieldGroupDestroy>,
                Symbol<'static, FnFieldGroupDestroy>,
            >(
                lib.get(b"dcgmFieldGroupDestroy\0")
                    .map_err(|e| e.to_string())?,
            );
            let watch_fields =
                std::mem::transmute::<Symbol<'_, FnWatchFields>, Symbol<'static, FnWatchFields>>(
                    lib.get(b"dcgmWatchFields\0").map_err(|e| e.to_string())?,
                );
            let update_all_fields = std::mem::transmute::<
                Symbol<'_, FnUpdateAllFields>,
                Symbol<'static, FnUpdateAllFields>,
            >(
                lib.get(b"dcgmUpdateAllFields\0")
                    .map_err(|e| e.to_string())?,
            );
            let get_latest =
                std::mem::transmute::<Symbol<'_, FnGetLatest>, Symbol<'static, FnGetLatest>>(
                    lib.get(b"dcgmGetLatestValuesForFields\0")
                        .map_err(|e| e.to_string())?,
                );
            let prof_resume = lib.get(b"dcgmProfResume\0").ok().map(|s| {
                std::mem::transmute::<Symbol<'_, FnProfResume>, Symbol<'static, FnProfResume>>(s)
            });
            Ok(Self {
                _lib: lib,
                init,
                shutdown,
                connect_v2,
                start_embedded,
                disconnect,
                stop_embedded,
                get_all_devices,
                group_create,
                group_destroy,
                field_group_create,
                field_group_destroy,
                watch_fields,
                update_all_fields,
                get_latest,
                prof_resume,
            })
        }
    }
}

#[derive(Serialize)]
pub struct SampleEnvelope {
    pub gpus: Vec<GpuSample>,
}

#[derive(Serialize, Default)]
pub struct GpuSample {
    pub uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graphics_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sm_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sm_occupancy: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tensor_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fp64_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fp32_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fp16_active: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bandwidth_utilization: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcie_tx_bytes_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcie_rx_bytes_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvlink_tx_bytes_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvlink_rx_bytes_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_reasons: Option<ThrottleReasons>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remapped_rows: Option<RemappedRowsInfo>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub xid_event_counts: HashMap<u32, u64>,
}

pub struct Session {
    fns: DcgmFns,
    handle: dcgmHandle_t,
    embedded: bool,
    group: usize,
    field_group: usize,
    watched: bool,
}

impl Session {
    pub fn open() -> Result<Self, String> {
        // SAFETY: freshly loaded library; symbols validated above.
        let fns = unsafe { DcgmFns::load() }?;
        let st = unsafe { (fns.init)() };
        if st != dcgmReturn_enum_DCGM_ST_OK {
            return Err(format!("dcgmInit failed: {st}"));
        }

        let mut handle: dcgmHandle_t = 0;
        let mut embedded = false;
        let addr = CString::new("127.0.0.1").unwrap();
        let mut params = dcgmConnectV2Params_t {
            version: make_dcgm_version::<dcgmConnectV2Params_t>(2),
            persistAfterDisconnect: 0,
            timeoutMs: 3000,
            addressIsUnixSocket: 0,
        };
        let st = unsafe { (fns.connect_v2)(addr.as_ptr(), &mut params, &mut handle) };
        if st != dcgmReturn_enum_DCGM_ST_OK {
            // Fallback: embedded mode (may require elevated privileges).
            let st2 = unsafe {
                (fns.start_embedded)(dcgmOperationMode_enum_DCGM_OPERATION_MODE_AUTO, &mut handle)
            };
            if st2 != dcgmReturn_enum_DCGM_ST_OK {
                let _ = unsafe { (fns.shutdown)() };
                return Err(format!(
                    "dcgmConnect_v2({st}) and dcgmStartEmbedded({st2}) both failed \
                     (embedded often needs root / CAP_SYS_ADMIN)"
                ));
            }
            embedded = true;
        }

        let mut group: usize = 0;
        let gname = CString::new("all-smi-dcgm").unwrap();
        let st = unsafe {
            (fns.group_create)(
                handle,
                dcgmGroupType_enum_DCGM_GROUP_DEFAULT,
                gname.as_ptr(),
                &mut group,
            )
        };
        if st != dcgmReturn_enum_DCGM_ST_OK {
            Self::teardown(&fns, handle, embedded, 0, 0);
            return Err(format!("dcgmGroupCreate failed: {st}"));
        }

        let mut field_group: usize = 0;
        let fgname = CString::new("all-smi-dcgm-fields").unwrap();
        let st = unsafe {
            (fns.field_group_create)(
                handle,
                FIELD_IDS.len() as c_int,
                FIELD_IDS.as_ptr(),
                fgname.as_ptr(),
                &mut field_group,
            )
        };
        if st != dcgmReturn_enum_DCGM_ST_OK {
            Self::teardown(&fns, handle, embedded, group, 0);
            return Err(format!("dcgmFieldGroupCreate failed: {st}"));
        }

        // 100ms update frequency (microseconds).
        let update_freq: c_longlong = 100_000;
        let st = unsafe { (fns.watch_fields)(handle, group, field_group, update_freq, 60.0, 20) };
        let watched = st == dcgmReturn_enum_DCGM_ST_OK;
        if let Some(ref resume) = fns.prof_resume {
            let _ = unsafe { resume(handle) };
        }
        let _ = unsafe { (fns.update_all_fields)(handle, 1) };
        // Allow the first profiling window to accumulate (matches dcgmi dmon).
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let _ = unsafe { (fns.update_all_fields)(handle, 1) };

        Ok(Self {
            fns,
            handle,
            embedded,
            group,
            field_group,
            watched,
        })
    }

    fn teardown(
        fns: &DcgmFns,
        handle: dcgmHandle_t,
        embedded: bool,
        group: usize,
        field_group: usize,
    ) {
        if field_group != 0 {
            let _ = unsafe { (fns.field_group_destroy)(handle, field_group) };
        }
        if group != 0 {
            let _ = unsafe { (fns.group_destroy)(handle, group) };
        }
        if handle != 0 {
            if embedded {
                let _ = unsafe { (fns.stop_embedded)(handle) };
            } else {
                let _ = unsafe { (fns.disconnect)(handle) };
            }
        }
        let _ = unsafe { (fns.shutdown)() };
    }

    pub fn sample(&mut self) -> Result<SampleEnvelope, String> {
        let _ = unsafe { (self.fns.update_all_fields)(self.handle, 1) };
        let mut ids = [0u32; DCGM_MAX_NUM_DEVICES as usize];
        let mut count: c_int = 0;
        let st = unsafe { (self.fns.get_all_devices)(self.handle, ids.as_mut_ptr(), &mut count) };
        if st != dcgmReturn_enum_DCGM_ST_OK {
            return Err(format!("dcgmGetAllDevices failed: {st}"));
        }
        let mut gpus = Vec::new();
        for &gpu_id in ids.iter().take(count.max(0) as usize) {
            let mut fields: Vec<c_ushort> = FIELD_IDS.to_vec();
            let mut values = vec![
                dcgmFieldValue_v1 {
                    version: make_dcgm_version::<dcgmFieldValue_v1>(1),
                    fieldId: 0,
                    fieldType: 0,
                    status: 0,
                    ts: 0,
                    value: unsafe { std::mem::zeroed() },
                };
                fields.len()
            ];
            let st = unsafe {
                (self.fns.get_latest)(
                    self.handle,
                    gpu_id as c_int,
                    fields.as_mut_ptr(),
                    fields.len() as c_uint,
                    values.as_mut_ptr(),
                )
            };
            if st != dcgmReturn_enum_DCGM_ST_OK {
                continue;
            }
            if let Some(sample) = decode_values(&values) {
                gpus.push(sample);
            }
        }
        let _ = self.watched; // silence unused when watch fails
        Ok(SampleEnvelope { gpus })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        Self::teardown(
            &self.fns,
            self.handle,
            self.embedded,
            self.group,
            self.field_group,
        );
        self.handle = 0;
        self.group = 0;
        self.field_group = 0;
    }
}

fn decode_values(values: &[dcgmFieldValue_v1]) -> Option<GpuSample> {
    let mut sample = GpuSample::default();
    let mut remap = RemappedRowsInfo::default();
    let mut have_remap = false;

    for v in values {
        if v.status != dcgmReturn_enum_DCGM_ST_OK {
            continue;
        }
        match v.fieldId as u32 {
            x if x == DCGM_FI_DEV_UUID => {
                if let Some(s) = read_string(v) {
                    sample.uuid = s;
                }
            }
            x if x == DCGM_FI_PROF_GR_ENGINE_ACTIVE => {
                sample.graphics_active = read_ratio(v);
            }
            x if x == DCGM_FI_PROF_SM_ACTIVE => sample.sm_active = read_ratio(v),
            x if x == DCGM_FI_PROF_SM_OCCUPANCY => sample.sm_occupancy = read_ratio(v),
            x if x == DCGM_FI_PROF_PIPE_TENSOR_ACTIVE => sample.tensor_active = read_ratio(v),
            x if x == DCGM_FI_PROF_DRAM_ACTIVE => {
                sample.memory_bandwidth_utilization = read_ratio(v);
            }
            x if x == DCGM_FI_PROF_PIPE_FP64_ACTIVE => sample.fp64_active = read_ratio(v),
            x if x == DCGM_FI_PROF_PIPE_FP32_ACTIVE => sample.fp32_active = read_ratio(v),
            x if x == DCGM_FI_PROF_PIPE_FP16_ACTIVE => sample.fp16_active = read_ratio(v),
            x if x == DCGM_FI_PROF_PCIE_TX_BYTES => {
                sample.pcie_tx_bytes_per_sec = read_f64(v);
            }
            x if x == DCGM_FI_PROF_PCIE_RX_BYTES => {
                sample.pcie_rx_bytes_per_sec = read_f64(v);
            }
            x if x == DCGM_FI_PROF_NVLINK_TX_BYTES => {
                sample.nvlink_tx_bytes_per_sec = read_f64(v);
            }
            x if x == DCGM_FI_PROF_NVLINK_RX_BYTES => {
                sample.nvlink_rx_bytes_per_sec = read_f64(v);
            }
            x if x == DCGM_FI_DEV_CLOCK_THROTTLE_REASONS => {
                if let Some(bits) = read_i64(v) {
                    sample.throttle_reasons = Some(throttle_from_bits(bits as u64));
                }
            }
            x if x == DCGM_FI_DEV_ROW_REMAP_CORRECTABLE_TOTAL => {
                if let Some(n) = read_i64(v) {
                    remap.correctable = n.max(0) as u32;
                    have_remap = true;
                }
            }
            x if x == DCGM_FI_DEV_ROW_REMAP_UNCORRECTABLE_TOTAL => {
                if let Some(n) = read_i64(v) {
                    remap.uncorrectable = n.max(0) as u32;
                    have_remap = true;
                }
            }
            x if x == DCGM_FI_DEV_ROW_REMAP_PENDING => {
                if let Some(n) = read_i64(v) {
                    remap.pending = n != 0;
                    have_remap = true;
                }
            }
            x if x == DCGM_FI_DEV_ROW_REMAP_FAILED => {
                if let Some(n) = read_i64(v) {
                    remap.failed = n != 0;
                    have_remap = true;
                }
            }
            x if x == DCGM_FI_DEV_XID_ERRORS => {
                if let Some(code) = read_i64(v)
                    && code > 0
                {
                    *sample.xid_event_counts.entry(code as u32).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    if have_remap {
        sample.remapped_rows = Some(remap);
    }
    if sample.uuid.is_empty() {
        None
    } else {
        Some(sample)
    }
}

fn throttle_from_bits(bits: u64) -> ThrottleReasons {
    ThrottleReasons {
        gpu_idle: bits & DCGM_CLOCKS_EVENT_REASON_GPU_IDLE as u64 != 0,
        app_clocks: bits & DCGM_CLOCKS_EVENT_REASON_CLOCKS_SETTING as u64 != 0,
        sw_power_cap: bits & DCGM_CLOCKS_EVENT_REASON_SW_POWER_CAP as u64 != 0,
        hw_slowdown: bits & DCGM_CLOCKS_EVENT_REASON_HW_SLOWDOWN as u64 != 0,
        sync_boost: bits & DCGM_CLOCKS_EVENT_REASON_SYNC_BOOST as u64 != 0,
        sw_thermal: bits & DCGM_CLOCKS_EVENT_REASON_SW_THERMAL as u64 != 0,
        hw_thermal: bits & DCGM_CLOCKS_EVENT_REASON_HW_THERMAL as u64 != 0,
        hw_power_brake: bits & DCGM_CLOCKS_EVENT_REASON_HW_POWER_BRAKE as u64 != 0,
        display_clocks: bits & DCGM_CLOCKS_EVENT_REASON_DISPLAY_CLOCKS as u64 != 0,
    }
}

fn read_ratio(v: &dcgmFieldValue_v1) -> Option<f32> {
    read_f64(v).map(|x| x.clamp(0.0, 1.0) as f32)
}

fn read_f64(v: &dcgmFieldValue_v1) -> Option<f64> {
    unsafe {
        match v.fieldType as u8 {
            x if x == DCGM_FT_DOUBLE => {
                let d = v.value.dbl;
                if d == DCGM_FP64_BLANK || !d.is_finite() {
                    None
                } else {
                    Some(d)
                }
            }
            x if x == DCGM_FT_INT64 => {
                let i = v.value.i64_;
                if i as u64 == DCGM_INT64_BLANK {
                    None
                } else {
                    Some(i as f64)
                }
            }
            _ => None,
        }
    }
}

fn read_i64(v: &dcgmFieldValue_v1) -> Option<i64> {
    unsafe {
        match v.fieldType as u8 {
            x if x == DCGM_FT_INT64 => {
                let i = v.value.i64_;
                if i as u64 == DCGM_INT64_BLANK {
                    None
                } else {
                    Some(i)
                }
            }
            x if x == DCGM_FT_DOUBLE => {
                let d = v.value.dbl;
                if d == DCGM_FP64_BLANK || !d.is_finite() {
                    None
                } else {
                    Some(d as i64)
                }
            }
            _ => None,
        }
    }
}

fn read_string(v: &dcgmFieldValue_v1) -> Option<String> {
    if v.fieldType as u8 != DCGM_FT_STRING {
        return None;
    }
    unsafe {
        let s = CStr::from_ptr(v.value.str_.as_ptr());
        let text = s.to_string_lossy();
        if text.is_empty() || text.contains("NULL") {
            None
        } else {
            Some(text.into_owned())
        }
    }
}

// Silence unused import of c_void in some rustc versions.
#[allow(dead_code)]
fn _touch(_: *mut c_void) {}
#[allow(dead_code)]
fn _touch_bindings(_: &bindings::dcgmHandle_t) {}
