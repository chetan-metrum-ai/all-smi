// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! Vendored bindgen output from DCGM 4.x headers.
//! Regenerate with `./regen-bindings.sh` on a host that has
//! `datacenter-gpu-manager-4-dev` installed.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all
)]

mod dcgm;
pub use dcgm::*;

/// `MAKE_DCGM_VERSION(type, ver)` from `dcgm_structs.h`.
pub const fn make_dcgm_version<T>(ver: u32) -> u32 {
    (std::mem::size_of::<T>() as u32) | (ver << 24)
}
