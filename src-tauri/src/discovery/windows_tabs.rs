//! Reads selected tab metadata only in a lifecycle-verified target. No document text or values.
use super::resolver::{title_paths, WorkspaceResolver};
use super::{path_environment, registration::process_started_at};
use crate::ipc::model::{Evidence, EvidenceSource, TargetInfo};
use std::path::Path;
use std::time::{Duration, Instant};
use windows::core::Result as WinResult;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub(crate) struct WindowsTabResolver {
    result: Result<Vec<Evidence>, String>,
}

impl WindowsTabResolver {
    pub fn collect(handle: isize, target: &TargetInfo, now: u64) -> Self {
        Self {
            result: collect(handle, target, now),
        }
    }
}
impl WorkspaceResolver for WindowsTabResolver {
    fn evidence(&self, _target: &TargetInfo, _now: u64) -> Result<Vec<Evidence>, String> {
        self.result.clone()
    }
}

fn collect(handle: isize, target: &TargetInfo, now: u64) -> Result<Vec<Evidence>, String> {
    let expected_start = target
        .process
        .started_at
        .parse::<u64>()
        .map_err(|_| "Invalid process lifecycle")?;
    if process_started_at(target.process.pid)? != expected_start {
        return Err("UIA target lifecycle changed".into());
    }
    let result = unsafe { selected_tabs(handle, target, now) }
        .map_err(|error| format!("UIA selected-tab metadata unavailable: {}", error.code()))?;
    if process_started_at(target.process.pid)? != expected_start {
        return Err("UIA target lifecycle changed after reading".into());
    }
    Ok(result)
}

unsafe fn selected_tabs(handle: isize, target: &TargetInfo, now: u64) -> WinResult<Vec<Evidence>> {
    // This backend thread owns no windows. No automation interfaces leave this MTA apartment.
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let _apartment = Apartment;
    let uia: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
    uia.SetAutoSetFocus(false)?;
    uia.SetConnectionTimeout(500)?;
    uia.SetTransactionTimeout(500)?;
    let root = uia.ElementFromHandle(HWND(handle as *mut _))?;
    if root.CurrentProcessId()? as u32 != target.process.pid {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80070005_u32 as i32,
        )));
    }
    let walker = uia.ControlViewWalker()?;
    let mut queue = std::collections::VecDeque::from([(root, 0)]);
    let mut count = 0;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut result = Vec::new();
    while let Some((parent, depth)) = queue.pop_front() {
        let mut child = walker.GetFirstChildElement(&parent).ok();
        while let Some(element) = child {
            count += 1;
            if count > 128 || Instant::now() >= deadline {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x800705b4_u32 as i32,
                )));
            }
            let kind = element.CurrentControlType()?;
            if kind == UIA_TabItemControlTypeId {
                if let Ok(selection) = element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                {
                    if selection.CurrentIsSelected()?.as_bool()
                        && !element.CurrentIsOffscreen()?.as_bool()
                    {
                        let name = element.CurrentName()?;
                        if name.len() <= 4096 {
                            for path in title_paths(&name.to_string()) {
                                result.push(Evidence {
                                    source: EvidenceSource::Uia,
                                    target_id: Some(target.id.clone()),
                                    process: Some(target.process.clone()),
                                    session_id: Some(format!("uia-{}", target.id)),
                                    observed_at: now,
                                    expires_at: now.saturating_add(30_000),
                                    environment: path_environment(Path::new(&path)),
                                    raw_path: path,
                                    verifiable: true,
                                    // Selection alone proves no generic application's workspace semantics.
                                    active_scope: false,
                                });
                            }
                        }
                    }
                }
            } else if depth < 4
                && [
                    UIA_PaneControlTypeId,
                    UIA_TabControlTypeId,
                    UIA_ToolBarControlTypeId,
                    UIA_GroupControlTypeId,
                ]
                .contains(&kind)
            {
                queue.push_back((element.clone(), depth + 1));
            }
            // Document, text, edit, list and browser content are never descended or read.
            child = walker.GetNextSiblingElement(&element).ok();
        }
    }
    Ok(result)
}
