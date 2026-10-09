//! Four transient, click-through border windows, owned entirely by the hook thread.
use super::geometry::{border_strips, Rect};
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, HBRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);

pub(super) struct Outline {
    handles: Vec<HWND>,
    class: Vec<u16>,
    instance: HINSTANCE,
    brush: HBRUSH,
    registered: bool,
    shown: Option<(Rect, i32)>,
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}

impl Outline {
    pub fn new() -> Result<Self, String> {
        let class: Vec<u16> = format!(
            "ArkDockCandidateOutline-{}",
            NEXT_CLASS.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let instance = unsafe { GetModuleHandleW(null_mut()) };
        let brush = unsafe { CreateSolidBrush(0x00b3e7a6) };
        if brush.is_null() {
            return Err("Cannot allocate candidate outline brush".into());
        }
        let mut outline = Self {
            handles: Vec::new(),
            class,
            instance,
            brush,
            registered: false,
            shown: None,
        };
        let mut descriptor: WNDCLASSEXW = unsafe { zeroed() };
        descriptor.cbSize = size_of::<WNDCLASSEXW>() as u32;
        descriptor.lpfnWndProc = Some(procedure);
        descriptor.hInstance = instance;
        descriptor.hbrBackground = brush;
        descriptor.lpszClassName = outline.class.as_ptr();
        if unsafe { RegisterClassExW(&descriptor) } == 0 {
            return Err(format!("Cannot register candidate outline: {}", unsafe {
                GetLastError()
            }));
        }
        // Windows owns the class background brush after successful registration.
        outline.registered = true;
        for _ in 0..4 {
            let handle = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    outline.class.as_ptr(),
                    outline.class.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    instance,
                    null_mut(),
                )
            };
            if handle.is_null() {
                return Err(format!("Cannot create candidate outline: {}", unsafe {
                    GetLastError()
                }));
            }
            outline.handles.push(handle);
            if unsafe { SetLayeredWindowAttributes(handle, 0, 220, LWA_ALPHA) } == 0 {
                return Err("Cannot configure candidate outline transparency".into());
            }
        }
        Ok(outline)
    }

    pub fn contains(&self, hwnd: HWND) -> bool {
        self.handles.contains(&hwnd)
    }

    pub fn show(&mut self, bounds: Rect, thickness: i32) -> Result<(), String> {
        if self.shown == Some((bounds, thickness)) {
            return Ok(());
        }
        let strips =
            border_strips(bounds, thickness).ok_or("Candidate outline geometry unavailable")?;
        // A failed partial update must hide every strip too.
        self.shown = Some((bounds, thickness));
        for (&handle, strip) in self.handles.iter().zip(strips) {
            // Transient normal Z order only; no topmost, owner, target mutation or focus change.
            if unsafe {
                SetWindowPos(
                    handle,
                    HWND_TOP,
                    strip.left,
                    strip.top,
                    strip.width(),
                    strip.height(),
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                )
            } == 0
            {
                self.hide();
                return Err("Candidate outline positioning failed".into());
            }
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.shown.take().is_some() {
            for &handle in &self.handles {
                unsafe {
                    ShowWindow(handle, SW_HIDE);
                }
            }
        }
    }
}

impl Drop for Outline {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            unsafe {
                DestroyWindow(handle);
            }
        }
        unsafe {
            if self.registered {
                UnregisterClassW(self.class.as_ptr(), self.instance);
            } else {
                DeleteObject(self.brush);
            }
        }
    }
}
