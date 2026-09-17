// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//! Versioned C ABI shared by the Linux DCGM runtime loader and its plugin.
//!
//! Mirrors the AMD plugin table shape: opaque session handle + JSON buffer
//! wire format. The plugin owns libdcgm via dlopen; the main binary never
//! links against it.

use std::ffi::c_void;

pub const DCGM_PLUGIN_ABI_VERSION: u32 = 1;
pub const DCGM_PLUGIN_ENTRY_SYMBOL: &[u8] = b"all_smi_dcgm_plugin_entry_v1\0";
pub const DCGM_PLUGIN_WIRE_FORMAT: &str = "all-smi-dcgm-json-v1";

#[repr(C)]
#[derive(Debug, Default)]
pub struct DcgmPluginBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

pub type CreateSessionFn = unsafe extern "C" fn() -> *mut c_void;
pub type DestroySessionFn = unsafe extern "C" fn(*mut c_void);
pub type SampleJsonFn = unsafe extern "C" fn(*mut c_void, *mut DcgmPluginBuffer) -> i32;
pub type ReadMetadataFn = unsafe extern "C" fn(*mut DcgmPluginBuffer) -> i32;
pub type FreeBufferFn = unsafe extern "C" fn(*mut DcgmPluginBuffer);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DcgmPluginApiV1 {
    pub abi_version: u32,
    pub struct_size: usize,
    pub create_session: Option<CreateSessionFn>,
    pub destroy_session: Option<DestroySessionFn>,
    pub sample_json: Option<SampleJsonFn>,
    pub read_metadata_json: Option<ReadMetadataFn>,
    pub free_buffer: Option<FreeBufferFn>,
}
