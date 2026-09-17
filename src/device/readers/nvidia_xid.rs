// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.

//! Best-effort NVML XID / ECC event watcher (P2).
//!
//! Spawns a background thread with its own NVML handle, registers for
//! critical XID and ECC events, and accumulates per-UUID counts. The
//! NVIDIA reader copies a snapshot into `GpuInfo::xid_event_counts` each
//! poll. Failures (unsupported platform, driver too old, no GPU) leave
//! the map empty — never invent zeros.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

const RING_CAP: usize = 50;

#[derive(Default)]
struct XidState {
    /// uuid → (xid → count)
    counts: HashMap<String, HashMap<u32, u64>>,
    /// Recent events (uuid, xid), newest last.
    ring: VecDeque<(String, u32)>,
}

static STATE: OnceLock<Mutex<XidState>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

fn state() -> &'static Mutex<XidState> {
    STATE.get_or_init(|| Mutex::new(XidState::default()))
}

/// Start the watcher once. Safe to call from every poll.
pub fn ensure_xid_watcher_started() {
    #[cfg(target_os = "linux")]
    {
        let _ = STARTED.get_or_init(|| {
            std::thread::Builder::new()
                .name("allsmi-nvml-xid".into())
                .spawn(xid_watcher_loop)
                .ok();
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = &STARTED;
    }
}

/// Snapshot counts for one GPU UUID.
pub fn xid_counts_for(uuid: &str) -> HashMap<u32, u64> {
    state()
        .lock()
        .map(|s| s.counts.get(uuid).cloned().unwrap_or_default())
        .unwrap_or_default()
}

/// Recent (uuid, xid) events, newest last (for alerts / webhooks).
#[allow(dead_code)]
pub fn recent_xid_events() -> Vec<(String, u32)> {
    state()
        .lock()
        .map(|s| s.ring.iter().cloned().collect())
        .unwrap_or_default()
}

fn record_event(uuid: String, xid: u32) {
    if let Ok(mut s) = state().lock() {
        *s.counts
            .entry(uuid.clone())
            .or_default()
            .entry(xid)
            .or_insert(0) += 1;
        if s.ring.len() >= RING_CAP {
            s.ring.pop_front();
        }
        s.ring.push_back((uuid, xid));
    }
}

#[cfg(target_os = "linux")]
fn xid_watcher_loop() {
    use nvml_wrapper::Nvml;
    use nvml_wrapper::bitmasks::event::EventTypes;
    use nvml_wrapper::enums::event::XidError;
    use std::thread;
    use std::time::Duration;

    let Ok(nvml) = Nvml::init() else {
        return;
    };
    let Ok(mut set) = nvml.create_event_set() else {
        return;
    };
    let want = EventTypes::CRITICAL_XID_ERROR
        | EventTypes::DOUBLE_BIT_ECC_ERROR
        | EventTypes::SINGLE_BIT_ECC_ERROR;
    let Ok(count) = nvml.device_count() else {
        return;
    };
    for i in 0..count {
        let Ok(device) = nvml.device_by_index(i) else {
            continue;
        };
        let Ok(supported) = device.supported_event_types() else {
            continue;
        };
        let events = supported & want;
        if events.is_empty() {
            continue;
        }
        match device.register_events(events, set) {
            Ok(s) => set = s,
            Err(_) => return,
        }
    }

    loop {
        match set.wait(1000) {
            Ok(data) => {
                let uuid = data.device.uuid().unwrap_or_else(|_| "unknown".to_string());
                let xid = if data.event_type.contains(EventTypes::CRITICAL_XID_ERROR) {
                    match data.event_data {
                        Some(XidError::Value(v)) => v as u32,
                        Some(XidError::Unknown) | None => 999,
                    }
                } else if data.event_type.contains(EventTypes::DOUBLE_BIT_ECC_ERROR) {
                    48
                } else if data.event_type.contains(EventTypes::SINGLE_BIT_ECC_ERROR) {
                    94
                } else {
                    continue;
                };
                record_event(uuid, xid);
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
