//! One bounded parent-PID snapshot per discovery pass (Windows only).
//!
//! A missing snapshot means "no lineage proven", never "assume lineage": the caller
//! fails closed. The map is capped so a pathological system snapshot cannot balloon
//! discovery memory.

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

const MAX_SNAPSHOT_ENTRIES: usize = 8192;

pub(crate) fn parent_map() -> Option<HashMap<u32, u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return None;
    }
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut taken = 0usize;
    let ok = unsafe {
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if taken >= MAX_SNAPSHOT_ENTRIES {
                    CloseHandle(snapshot);
                    return None;
                }
                map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                taken += 1;
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
            true
        } else {
            false
        }
    };
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    ok.then_some(map)
}
