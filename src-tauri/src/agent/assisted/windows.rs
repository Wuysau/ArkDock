//! Windows UIA access is confined to the explicitly bound top-level window.
use super::{AssistedFill, AssistedOffer, TargetInfo, CONFIRMATION_TTL_MS};
use crate::discovery::registration::process_started_at;
use crate::ipc::model::DeliveryState;
use std::collections::VecDeque;
use std::mem::{size_of, zeroed};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};
use windows::core::BSTR;
use windows::Win32::Foundation::HWND as UiaHwnd;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElemsize,
    SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::*;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevation,
    TokenIntegrityLevel, TokenUIAccess, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, PeekMessageW, PostThreadMessageW, EVENT_SYSTEM_FOREGROUND, GA_ROOT, MSG,
    PM_NOREMOVE, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 16;
const INSPECTION_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(super) struct Profile {
    executable: String,
    contract: Option<ComposerContract>,
}

#[derive(Clone)]
struct ComposerContract {
    automation_id: String,
    name: String,
    class_name: String,
}

/// Not compiled into production. A fixture cannot register an executable or
/// control as a supported Agent; tests own the precise process and HWND.
#[cfg(test)]
#[derive(Clone)]
pub(super) struct FixtureProfile {
    pub handle: isize,
    pub target: TargetInfo,
}

pub(super) struct Confirmation {
    pub offer: AssistedOffer,
    handle: isize,
    caller_handle: isize,
    target: TargetInfo,
    profile: Profile,
    snapshot: Snapshot,
    created: Instant,
    foreground: ForegroundGuard,
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    root_runtime: Vec<i32>,
    composer_runtime: Vec<i32>,
    composer_name: String,
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn windows_error(operation: &str, error: windows::core::Error) -> String {
    format!("{operation}: {}", error.code())
}

fn ensure_normal_integrity(process: HANDLE) -> Result<(), String> {
    unsafe {
        let mut raw_token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut raw_token) == 0 {
            return Err("无法核验目标权限；请复制修改要求。".into());
        }
        let token = Process(raw_token);
        let mut elevation: TOKEN_ELEVATION = zeroed();
        let mut needed = 0;
        if GetTokenInformation(
            token.0,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut needed,
        ) == 0
            || elevation.TokenIsElevated != 0
        {
            return Err("不支持向高权限窗口辅助填写，也不会请求提权。".into());
        }
        let mut ui_access: u32 = 0;
        if GetTokenInformation(
            token.0,
            TokenUIAccess,
            &mut ui_access as *mut _ as *mut _,
            size_of::<u32>() as u32,
            &mut needed,
        ) == 0
            || ui_access != 0
        {
            return Err("目标权限不属于普通桌面应用；请复制修改要求。".into());
        }
        // Aligned, fixed-size buffer. Unknown/oversized token data fails closed.
        let mut integrity = [0usize; 64];
        if GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            integrity.as_mut_ptr().cast(),
            size_of_val(&integrity) as u32,
            &mut needed,
        ) == 0
        {
            return Err("无法核验目标完整性级别；请复制修改要求。".into());
        }
        let label = &*integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 || *GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)) != 0x2000 {
            return Err("只允许普通完整性级别的桌面窗口；请复制修改要求。".into());
        }
    }
    Ok(())
}

fn executable(handle: isize, target: &TargetInfo) -> Result<String, String> {
    validate_window(handle, target)?;
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.process.pid) };
    if raw.is_null() {
        return Err("无法读取目标进程身份。".into());
    }
    let process = Process(raw);
    ensure_normal_integrity(process.0)?;
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err("无法核验目标应用路径。".into());
    }
    validate_window(handle, target)?;
    Ok(String::from_utf16_lossy(&path[..length as usize]))
}

fn validate_window(handle: isize, target: &TargetInfo) -> Result<(), String> {
    let hwnd = handle as HWND;
    unsafe {
        if handle == 0
            || IsWindow(hwnd) == 0
            || IsWindowVisible(hwnd) == 0
            || IsIconic(hwnd) != 0
            || IsWindowEnabled(hwnd) == 0
            || GetAncestor(hwnd, GA_ROOT) != hwnd
        {
            return Err("目标窗口不存在、不可见或不是可用的顶层窗口。".into());
        }
        let mut pid = 0;
        if GetWindowThreadProcessId(hwnd, &mut pid) == 0 || pid != target.process.pid {
            return Err("目标窗口的进程已改变；请重新绑定。".into());
        }
    }
    if process_started_at(target.process.pid)?.to_string() != target.process.started_at {
        return Err("目标进程生命周期已改变；请重新绑定。".into());
    }
    Ok(())
}

pub(super) fn profile(
    handle: isize,
    target: &TargetInfo,
    #[cfg(test)] fixture: Option<&FixtureProfile>,
) -> Result<Profile, String> {
    let executable = executable(handle, target)?;
    #[cfg(test)]
    if fixture.is_some_and(|fixture| fixture.handle == handle && fixture.target == *target) {
        return Ok(Profile {
            executable,
            contract: Some(ComposerContract {
                automation_id: "ArkDockFixtureComposer".into(),
                name: "ArkDock owned test composer".into(),
                class_name: "TextBox".into(),
            }),
        });
    }
    let normalized = executable.to_ascii_lowercase().replace('/', "\\");
    // The installed Codex app uses ChatGPT.exe. Its package identity is
    // required; a generic ChatGPT.exe or similarly titled window is not Codex.
    if normalized.contains("\\windowsapps\\openai.codex_")
        && normalized.ends_with("\\app\\chatgpt.exe")
    {
        return Ok(Profile {
            executable,
            contract: None,
        });
    }
    Err(
        "该应用没有已核验的聊天输入框规则；终端编辑区域也不能证明是 Agent 输入框。请复制修改要求。"
            .into(),
    )
}

pub(super) fn inspect(
    handle: isize,
    target: &TargetInfo,
    caller_handle: isize,
    now: u64,
    profile: Profile,
    cancelled: Option<&AtomicBool>,
) -> Result<Confirmation, String> {
    check_cancellation(cancelled)?;
    validate_caller(caller_handle)?;
    let foreground = ForegroundGuard::start(handle, caller_handle)?;
    let snapshot = inspect_composer(handle, target, &profile, None, None, cancelled)?;
    check_cancellation(cancelled)?;
    foreground.validate()?;
    let mut token = [0u8; 16];
    getrandom::fill(&mut token).map_err(|_| "无法创建辅助填写确认凭据。")?;
    let confirmation_id = token.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(Confirmation {
        offer: AssistedOffer {
            confirmation_id,
            target_id: target.id.clone(),
            app: target.app.clone(),
            pid: target.process.pid,
            composer_name: snapshot.composer_name.clone(),
            expires_at: now.saturating_add(CONFIRMATION_TTL_MS),
            reason:
                "仅填写此空输入框；窗口身份不证明 Agent 会话。填写后请在目标窗口检查并自行提交。"
                    .into(),
        },
        handle,
        caller_handle,
        target: target.clone(),
        profile,
        snapshot,
        created: Instant::now(),
        foreground,
    })
}

fn validate_caller(caller_handle: isize) -> Result<(), String> {
    // Zero is available to native tools operating while the chosen target is
    // foreground. The desktop always supplies its own native window handle.
    if caller_handle == 0 {
        return Ok(());
    }
    let mut pid = 0;
    if unsafe { IsWindow(caller_handle as HWND) } == 0
        || unsafe { GetWindowThreadProcessId(caller_handle as HWND, &mut pid) } == 0
        || pid != std::process::id()
    {
        return Err("辅助填写调用窗口无效。".into());
    }
    Ok(())
}

pub(super) struct FillRequest<'a> {
    pub handle: isize,
    pub target: &'a TargetInfo,
    pub caller_handle: isize,
    pub confirmation_id: &'a str,
    pub prompt: &'a str,
    pub now: u64,
}

pub(super) fn fill(
    pending: Confirmation,
    request: FillRequest<'_>,
    cancelled: Option<&AtomicBool>,
) -> Result<AssistedFill, String> {
    let FillRequest {
        handle,
        target,
        caller_handle,
        confirmation_id,
        prompt,
        now,
    } = request;
    check_cancellation(cancelled)?;
    if confirmation_id != pending.offer.confirmation_id
        || handle != pending.handle
        || caller_handle != pending.caller_handle
        || *target != pending.target
        || now >= pending.offer.expires_at
        || pending.created.elapsed() >= Duration::from_millis(CONFIRMATION_TTL_MS)
    {
        return Err("辅助填写确认已过期或目标已改变；请重新检查。".into());
    }
    validate_caller(caller_handle)?;
    pending.foreground.validate()?;
    if executable(handle, target)? != pending.profile.executable {
        return Err("目标应用身份已改变。".into());
    }
    let mut write_attempted = false;
    let result = inspect_composer(
        handle,
        target,
        &pending.profile,
        Some((&pending.snapshot, &pending.foreground)),
        Some((prompt, &mut write_attempted)),
        cancelled,
    );
    match result.and_then(|_| check_cancellation(cancelled)).and_then(|_| pending.foreground.validate()).and_then(|_| validate_window(handle, target)) {
        Ok(()) => Ok(AssistedFill {
            state: DeliveryState::FilledPendingUserSend,
            receipt_id: None,
            reason: "已核验文字填入所确认的输入框；尚未发送。请在目标窗口检查并自行提交。".into(),
        }),
        Err(_) if write_attempted => Ok(AssistedFill {
            state: DeliveryState::Unknown,
            receipt_id: None,
            reason: "已尝试填写，但无法核验最终输入框或目标状态；请检查目标窗口，不要直接重试。未自动提交。".into(),
        }),
        Err(reason) => Err(reason),
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn runtime_id(element: &IUIAutomationElement) -> Result<Vec<i32>, String> {
    unsafe {
        let array = element
            .GetRuntimeId()
            .map_err(|e| windows_error("UIA runtime ID unavailable", e))?;
        if array.is_null() {
            return Err("UIA runtime ID unavailable".into());
        }
        let result = (|| {
            if SafeArrayGetDim(array) != 1 || SafeArrayGetElemsize(array) != size_of::<i32>() as u32
            {
                return Err("Invalid UIA runtime ID shape".into());
            }
            let lower = SafeArrayGetLBound(array, 1)
                .map_err(|e| windows_error("UIA runtime ID lower bound", e))?;
            let upper = SafeArrayGetUBound(array, 1)
                .map_err(|e| windows_error("UIA runtime ID upper bound", e))?;
            let length = i64::from(upper) - i64::from(lower) + 1;
            if !(1..=32).contains(&length) {
                return Err("Invalid UIA runtime ID size".into());
            }
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data)
                .map_err(|e| windows_error("UIA runtime ID access", e))?;
            let result = if data.is_null() {
                Err("Invalid UIA runtime ID data".into())
            } else {
                Ok(std::slice::from_raw_parts(data.cast::<i32>(), length as usize).to_vec())
            };
            let unlocked = SafeArrayUnaccessData(array)
                .map_err(|e| windows_error("UIA runtime ID release", e));
            unlocked.and(result)
        })();
        let destroyed =
            SafeArrayDestroy(array).map_err(|e| windows_error("UIA runtime ID disposal", e));
        destroyed.and(result)
    }
}

fn inspect_composer(
    handle: isize,
    target: &TargetInfo,
    profile: &Profile,
    expected: Option<(&Snapshot, &ForegroundGuard)>,
    write: Option<(&str, &mut bool)>,
    cancelled: Option<&AtomicBool>,
) -> Result<Snapshot, String> {
    check_cancellation(cancelled)?;
    validate_window(handle, target)?;
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| windows_error("UIA apartment unavailable", e))?;
        let _apartment = Apartment;
        let uia: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| windows_error("UIA unavailable", e))?;
        uia.SetAutoSetFocus(false)
            .map_err(|e| windows_error("UIA focus safety unavailable", e))?;
        uia.SetConnectionTimeout(500)
            .map_err(|e| windows_error("UIA timeout unavailable", e))?;
        uia.SetTransactionTimeout(500)
            .map_err(|e| windows_error("UIA timeout unavailable", e))?;
        let root = uia
            .ElementFromHandle(UiaHwnd(handle as *mut _))
            .map_err(|e| windows_error("UIA target unavailable", e))?;
        check_cancellation(cancelled)?;
        if root
            .CurrentProcessId()
            .map_err(|e| windows_error("UIA process unavailable", e))? as u32
            != target.process.pid
        {
            return Err("UIA 根节点进程与选定窗口不一致。".into());
        }
        let root_runtime = runtime_id(&root)?;
        let walker = uia
            .RawViewWalker()
            .map_err(|e| windows_error("UIA walker unavailable", e))?;
        let mut queue = VecDeque::from([(root, 0)]);
        let deadline = Instant::now() + INSPECTION_DEADLINE;
        let mut count = 0;
        let mut candidates = Vec::new();
        while let Some((parent, depth)) = queue.pop_front() {
            check_cancellation(cancelled)?;
            let mut next = walker.GetFirstChildElement(&parent).ok();
            while let Some(element) = next {
                check_cancellation(cancelled)?;
                count += 1;
                if count > MAX_NODES || Instant::now() >= deadline {
                    return Err("UIA 输入框检查超过限额；请复制修改要求。".into());
                }
                let kind = element
                    .CurrentControlType()
                    .map_err(|e| windows_error("UIA control unavailable", e))?;
                if kind == UIA_EditControlTypeId {
                    // Values and text patterns are never read during discovery.
                    // Unknown/password controls are never read beyond metadata.
                    if !element
                        .CurrentIsPassword()
                        .map_err(|e| windows_error("UIA password classification unavailable", e))?
                        .as_bool()
                    {
                        if let Some(contract) = &profile.contract {
                            let id = element
                                .CurrentAutomationId()
                                .map_err(|e| windows_error("UIA composer ID unavailable", e))?;
                            if id == contract.automation_id.as_str()
                                && element.CurrentName().map_err(|e| {
                                    windows_error("UIA composer name unavailable", e)
                                })? == contract.name.as_str()
                                && element.CurrentClassName().map_err(|e| {
                                    windows_error("UIA composer class unavailable", e)
                                })? == contract.class_name.as_str()
                            {
                                candidates.push(element.clone());
                            }
                        }
                    }
                } else if depth < MAX_DEPTH
                    && [
                        UIA_PaneControlTypeId,
                        UIA_GroupControlTypeId,
                        UIA_CustomControlTypeId,
                        UIA_DocumentControlTypeId,
                        UIA_ToolBarControlTypeId,
                    ]
                    .contains(&kind)
                {
                    // Container traversal reads no names, document text, chat
                    // history, list items, clipboard contents, or passwords.
                    queue.push_back((element.clone(), depth + 1));
                }
                next = walker.GetNextSiblingElement(&element).ok();
            }
        }
        if candidates.len() != 1 {
            return Err(
                "当前窗口未暴露唯一且已核验的聊天输入框及可写 UIA ValuePattern；请复制修改要求。"
                    .into(),
            );
        }
        let composer = candidates.pop().expect("exactly one composer");
        check_cancellation(cancelled)?;
        validate_composer(&composer, target)?;
        let pattern = composer
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .map_err(|_| "输入框不支持可核验的 ValuePattern 填写；请复制修改要求。")?;
        if pattern
            .CurrentIsReadOnly()
            .map_err(|e| windows_error("UIA read-only state unavailable", e))?
            .as_bool()
        {
            return Err("目标输入框为只读；请复制修改要求。".into());
        }
        let snapshot = Snapshot {
            root_runtime,
            composer_runtime: runtime_id(&composer)?,
            composer_name: composer
                .CurrentName()
                .map_err(|e| windows_error("UIA composer name unavailable", e))?
                .to_string(),
        };
        if let Some((expected, guard)) = expected {
            if snapshot != *expected {
                return Err("输入框已更换；请重新检查并确认。".into());
            }
            guard.validate()?;
        }
        // This is the only pre-fill value read: the recognized composer must
        // be empty. Existing drafts are neither logged nor returned or replaced.
        if !pattern
            .CurrentValue()
            .map_err(|e| windows_error("UIA composer empty-state unavailable", e))?
            .is_empty()
        {
            return Err("目标输入框已有草稿，已取消填写以保留内容。".into());
        }
        validate_window(handle, target)?;
        if let Some((prompt, attempted)) = write {
            validate_composer(&composer, target)?;
            if let Some((_, guard)) = expected {
                guard.validate()?;
            }
            check_cancellation(cancelled)?;
            *attempted = true;
            pattern
                .SetValue(&BSTR::from(prompt))
                .map_err(|e| windows_error("UIA composer fill failed", e))?;
            check_cancellation(cancelled)?;
            // No Invoke/Enter/SendInput/WM_SETTEXT/SetFocus. Only this exact
            // recognized input's value is compared; no chat history is read.
            if pattern
                .CurrentValue()
                .map_err(|e| windows_error("UIA fill verification unavailable", e))?
                != prompt
                || runtime_id(&composer)? != snapshot.composer_runtime
            {
                return Err("UIA 填写结果未能核验。".into());
            }
        }
        validate_window(handle, target)?;
        Ok(snapshot)
    }
}

fn check_cancellation(cancelled: Option<&AtomicBool>) -> Result<(), String> {
    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::SeqCst)) {
        Err("辅助填写已取消或作用域已改变。".into())
    } else {
        Ok(())
    }
}

unsafe fn validate_composer(
    composer: &IUIAutomationElement,
    target: &TargetInfo,
) -> Result<(), String> {
    let valid = composer
        .CurrentProcessId()
        .map_err(|e| windows_error("UIA composer process unavailable", e))? as u32
        == target.process.pid
        && composer
            .CurrentIsEnabled()
            .map_err(|e| windows_error("UIA composer enabled state unavailable", e))?
            .as_bool()
        && !composer
            .CurrentIsOffscreen()
            .map_err(|e| windows_error("UIA composer visibility unavailable", e))?
            .as_bool()
        && !composer
            .CurrentIsPassword()
            .map_err(|e| windows_error("UIA composer password classification unavailable", e))?
            .as_bool();
    if valid {
        Ok(())
    } else {
        Err("输入框进程、可见性或安全状态已改变。".into())
    }
}

// The event hook observes only whether the foreground HWND changes. It never
// reads other windows, process metadata, keystrokes, or any UI text. A switch
// away and back still invalidates consent, unlike polling only at fill time.
thread_local! {
    static FOREGROUND_WATCH: std::cell::RefCell<Option<(isize, Arc<AtomicBool>)>> = const { std::cell::RefCell::new(None) };
}
unsafe extern "system" fn foreground_event(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    FOREGROUND_WATCH.with(|watch| {
        if let Some((expected, changed)) = &*watch.borrow() {
            if hwnd as isize != *expected {
                changed.store(true, Ordering::SeqCst);
            }
        }
    });
}

struct ForegroundGuard {
    expected: isize,
    changed: Arc<AtomicBool>,
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl ForegroundGuard {
    fn start(target: isize, caller: isize) -> Result<Self, String> {
        let expected = unsafe { GetForegroundWindow() } as isize;
        if expected == 0 || (expected != target && expected != caller) {
            return Err("前台窗口已切换；请回到 ArkDock 后重新确认。".into());
        }
        let changed = Arc::new(AtomicBool::new(false));
        let watcher_changed = changed.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let mut message: MSG = zeroed();
            PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            FOREGROUND_WATCH.with(|watch| *watch.borrow_mut() = Some((expected, watcher_changed)));
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                std::ptr::null_mut(),
                Some(foreground_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_null() {
                let _ = tx.send(Err("无法监测目标切换；请复制修改要求。".to_owned()));
                return;
            }
            if tx.send(Ok(GetCurrentThreadId())).is_err() {
                UnhookWinEvent(hook);
                return;
            }
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {}
            UnhookWinEvent(hook);
            FOREGROUND_WATCH.with(|watch| *watch.borrow_mut() = None);
        });
        let thread_id = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "前台监测启动超时。")??;
        let guard = Self {
            expected,
            changed,
            thread_id,
            thread: Some(thread),
        };
        guard.validate()?;
        Ok(guard)
    }
    fn validate(&self) -> Result<(), String> {
        if self.changed.load(Ordering::SeqCst)
            || unsafe { GetForegroundWindow() } as isize != self.expected
        {
            return Err("确认期间前台窗口已切换；辅助填写已取消。".into());
        }
        Ok(())
    }
}
impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests;
