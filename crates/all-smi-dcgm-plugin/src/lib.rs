// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

#![cfg(all(target_os = "linux", not(target_env = "musl")))]

mod bindings;
mod session;

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use all_smi::device::readers::dcgm_plugin_api::{
    DCGM_PLUGIN_ABI_VERSION, DCGM_PLUGIN_WIRE_FORMAT, DcgmPluginApiV1, DcgmPluginBuffer,
};
use serde::Serialize;

use session::Session;

const STATUS_OK: i32 = 0;
const STATUS_INVALID_ARGUMENT: i32 = -1;
const STATUS_FAILED: i32 = -2;

#[derive(Serialize)]
struct PluginMetadata<'a> {
    plugin_version: &'a str,
    wire_format: &'a str,
}

fn write_json<T: Serialize>(value: &T, out: *mut DcgmPluginBuffer) -> i32 {
    if out.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let Ok(mut bytes) = serde_json::to_vec(value) else {
        return STATUS_FAILED;
    };
    let buffer = DcgmPluginBuffer {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    // SAFETY: `out` was checked for null and points to host-provided storage.
    unsafe { out.write(buffer) };
    STATUS_OK
}

extern "C" fn create_session() -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| match Session::open() {
        Ok(session) => Box::into_raw(Box::new(session)).cast::<c_void>(),
        Err(_) => std::ptr::null_mut(),
    }))
    .unwrap_or(std::ptr::null_mut())
}

extern "C" fn destroy_session(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: handle came from create_session; ABI requires one destroy.
        unsafe { drop(Box::from_raw(handle.cast::<Session>())) };
    }));
}

extern "C" fn sample_json(handle: *mut c_void, out: *mut DcgmPluginBuffer) -> i32 {
    if handle.is_null() || out.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: handle is a live Session from create_session.
        let session = unsafe { &mut *handle.cast::<Session>() };
        match session.sample() {
            Ok(envelope) => write_json(&envelope, out),
            Err(_) => STATUS_FAILED,
        }
    }))
    .unwrap_or(STATUS_FAILED)
}

extern "C" fn read_metadata_json(out: *mut DcgmPluginBuffer) -> i32 {
    let meta = PluginMetadata {
        plugin_version: env!("CARGO_PKG_VERSION"),
        wire_format: DCGM_PLUGIN_WIRE_FORMAT,
    };
    write_json(&meta, out)
}

extern "C" fn free_buffer(buffer: *mut DcgmPluginBuffer) {
    if buffer.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: buffer was produced by write_json in this plugin.
        unsafe {
            let buf = buffer.read();
            if !buf.ptr.is_null() && buf.capacity > 0 {
                let _ = Vec::from_raw_parts(buf.ptr, buf.len, buf.capacity);
            }
            buffer.write(DcgmPluginBuffer::default());
        }
    }));
}

static API: DcgmPluginApiV1 = DcgmPluginApiV1 {
    abi_version: DCGM_PLUGIN_ABI_VERSION,
    struct_size: std::mem::size_of::<DcgmPluginApiV1>(),
    create_session: Some(create_session),
    destroy_session: Some(destroy_session),
    sample_json: Some(sample_json),
    read_metadata_json: Some(read_metadata_json),
    free_buffer: Some(free_buffer),
};

#[unsafe(no_mangle)]
pub extern "C" fn all_smi_dcgm_plugin_entry_v1() -> *const DcgmPluginApiV1 {
    &API
}
