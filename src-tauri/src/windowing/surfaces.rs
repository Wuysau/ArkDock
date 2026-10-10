//! Small Win32 recovery surfaces. Both HWNDs belong to the existing window actor.
use super::display::orb_edge;
use super::geometry::Rect;
use crate::ipc::model::{DockSide, MotionIntensity, MotionSettings};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_LBUTTON,
};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const TRAY_MESSAGE: u32 = WM_APP + 41;
static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);
static TASKBAR_MESSAGE: AtomicU32 = AtomicU32::new(0);
const TRAY_KEY_SELECT: u32 = NIN_SELECT | 1;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Press(POINT),
    Move(POINT),
    Release(POINT),
    CancelDrag,
    OrbMenu,
    TrayMenu,
    Expand,
    ShowOrb,
    Hide,
    Exit,
    AutoHide,
    FixLeft,
    FixRight,
    ExplorerRestart,
}

thread_local! {
    static ACTIONS: RefCell<VecDeque<Action>> = RefCell::new(VecDeque::with_capacity(128));
}

fn queue(action: Action) {
    ACTIONS.with(|queue| {
        let mut queue = queue.borrow_mut();
        if matches!(action, Action::Move(_)) && matches!(queue.back(), Some(Action::Move(_))) {
            queue.pop_back();
        }
        if queue.len() < 128 {
            queue.push_back(action);
        }
    });
}

fn pointer() -> Option<POINT> {
    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some(point)
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            if let Some(p) = pointer() {
                SetCapture(hwnd);
                queue(Action::Press(p));
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(p) = pointer() {
                queue(Action::Move(p));
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(p) = pointer() {
                queue(Action::Release(p));
            }
            ReleaseCapture();
            0
        }
        WM_CAPTURECHANGED => {
            queue(Action::CancelDrag);
            0
        }
        WM_RBUTTONUP => {
            queue(Action::OrbMenu);
            0
        }
        WM_COMMAND => {
            match w as u32 & 0xffff {
                1 => queue(Action::Expand),
                2 => queue(Action::ShowOrb),
                3 => queue(Action::Exit),
                4 => queue(Action::FixLeft),
                5 => queue(Action::FixRight),
                6 => queue(Action::AutoHide),
                7 => queue(Action::Hide),
                _ => {}
            }
            0
        }
        TRAY_MESSAGE => {
            match l as u32 & 0xffff {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_SELECT | TRAY_KEY_SELECT => {
                    queue(Action::Expand)
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => queue(Action::TrayMenu),
                _ => {}
            }
            0
        }
        _ => {
            // Registered messages use their dynamically assigned identifier.
            if message == taskbar_message() {
                queue(Action::ExplorerRestart);
                0
            } else {
                DefWindowProcW(hwnd, message, w, l)
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn taskbar_message() -> u32 {
    let existing = TASKBAR_MESSAGE.load(Ordering::Relaxed);
    if existing != 0 {
        return existing;
    }
    let message = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
    TASKBAR_MESSAGE.store(message, Ordering::Relaxed);
    message
}
fn failure(label: &str) -> String {
    format!("{label}: {}", unsafe { GetLastError() })
}

struct Drag {
    start: POINT,
    bounds: Rect,
    moved: bool,
}

struct Animation {
    from: Rect,
    to: Rect,
    from_alpha: u8,
    to_alpha: u8,
    start: Instant,
    duration: Duration,
}

struct OrbBitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    size: i32,
}

impl OrbBitmap {
    fn new(size: i32) -> Result<Self, String> {
        let dc = unsafe { CreateCompatibleDC(null_mut()) };
        if dc.is_null() {
            return Err(failure("Cannot allocate Orb DC"));
        }
        let mut info: BITMAPINFO = unsafe { zeroed() };
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        info.bmiHeader.biHeight = -size;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(dc);
            }
            return Err(failure("Cannot allocate Orb bitmap"));
        }
        let previous = unsafe { SelectObject(dc, bitmap) };
        if previous.is_null() || previous as isize == -1 {
            unsafe {
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
            return Err(failure("Cannot select Orb bitmap"));
        }
        let pixels = orb_pixels(size);
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());
        }
        Ok(Self {
            dc,
            bitmap,
            previous,
            size,
        })
    }

    fn draw(&self, hwnd: HWND, rect: Rect, opacity: u8) -> Result<(), String> {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if unsafe {
            UpdateLayeredWindow(
                hwnd,
                null_mut(),
                &POINT {
                    x: rect.left,
                    y: rect.top,
                },
                &SIZE {
                    cx: self.size,
                    cy: self.size,
                },
                self.dc,
                &POINT { x: 0, y: 0 },
                0,
                &blend,
                ULW_ALPHA,
            )
        } == 0
        {
            Err(failure("Cannot draw Orb alpha surface"))
        } else {
            Ok(())
        }
    }
}

impl Drop for OrbBitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub(super) struct Surfaces {
    orb: HWND,
    receiver: HWND,
    class: Vec<u16>,
    instance: HINSTANCE,
    icon: HICON,
    pub tray_available: bool,
    tray_attempt: Instant,
    bounds: Option<Rect>,
    edge_bounds: Option<Rect>,
    dpi: u32,
    side: DockSide,
    fixed: Option<DockSide>,
    drag: Option<Drag>,
    auto_hide: bool,
    tucked: bool,
    last_interaction: Instant,
    bitmap: Option<OrbBitmap>,
    animation: Option<Animation>,
    motion: MotionSettings,
}

impl Surfaces {
    pub fn new(main: HWND) -> Result<Self, String> {
        let class = wide(&format!(
            "ArkDockOrb-{}",
            NEXT_CLASS.fetch_add(1, Ordering::Relaxed)
        ));
        let instance = unsafe { GetModuleHandleW(null_mut()) };
        let mut descriptor: WNDCLASSEXW = unsafe { zeroed() };
        descriptor.cbSize = size_of::<WNDCLASSEXW>() as u32;
        descriptor.lpfnWndProc = Some(procedure);
        descriptor.hInstance = instance;
        descriptor.lpszClassName = class.as_ptr();
        descriptor.hCursor = unsafe { LoadCursorW(null_mut(), IDC_HAND) };
        if unsafe { RegisterClassExW(&descriptor) } == 0 {
            return Err(failure("Cannot register Orb"));
        }
        let pixels = orb_pixels(32);
        let mask = [0u8; 128];
        let icon = unsafe {
            CreateIcon(
                instance,
                32,
                32,
                1,
                32,
                mask.as_ptr(),
                pixels.as_ptr() as *const u8,
            )
        };
        let mut surfaces = Self {
            orb: null_mut(),
            receiver: null_mut(),
            class,
            instance,
            icon,
            tray_available: false,
            tray_attempt: Instant::now() - Duration::from_secs(3),
            bounds: None,
            edge_bounds: None,
            dpi: 96,
            side: DockSide::Right,
            fixed: None,
            drag: None,
            auto_hide: false,
            tucked: false,
            last_interaction: Instant::now(),
            bitmap: None,
            animation: None,
            motion: MotionSettings {
                reduced: false,
                intensity: MotionIntensity::Standard,
            },
        };
        surfaces.orb = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                surfaces.class.as_ptr(),
                wide("ArkDock Orb").as_ptr(),
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
        // Hidden top-level receives Explorer's TaskbarCreated broadcast; message-only HWNDs do not.
        surfaces.receiver = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                surfaces.class.as_ptr(),
                wide("ArkDock Tray Receiver").as_ptr(),
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
        if surfaces.orb.is_null() || surfaces.receiver.is_null() || icon.is_null() {
            return Err(failure("Cannot create native recovery surfaces"));
        }
        let mut rect = unsafe { zeroed() };
        if unsafe { GetWindowRect(main, &mut rect) } == 0 {
            return Err(failure("Main bounds unavailable"));
        }
        // Initial entry uses the right work-area edge. User drag releases still choose
        // the nearest edge unless an explicit context-menu side is fixed.
        surfaces.fixed = Some(DockSide::Right);
        surfaces.snap(POINT {
            x: rect.right,
            y: rect.top + (rect.bottom - rect.top) / 2,
        })?;
        surfaces.fixed = None;
        surfaces.retry_tray();
        Ok(surfaces)
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.receiver;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = self.icon;
        let tip = wide("ArkDock · 点击展开，右键恢复菜单");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        data
    }

    pub fn retry_tray(&mut self) {
        if self.tray_available || self.tray_attempt.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.tray_attempt = Instant::now();
        let mut data = self.tray_data();
        // The return value is Explorer's registration acknowledgment, never an assumed success.
        self.tray_available = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        if self.tray_available {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            unsafe {
                Shell_NotifyIconW(NIM_SETVERSION, &data);
            }
        }
    }

    pub fn explorer_restarted(&mut self) {
        // TaskbarCreated can be repeated, including a synthetic owned-window test.
        // Delete only our exact receiver/uID; a new Explorer simply has nothing to delete.
        // Re-registration still requires a fresh NIM_ADD success acknowledgment.
        if !self.receiver.is_null() {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            }
        }
        self.tray_available = false;
        self.tray_attempt = Instant::now() - Duration::from_secs(3);
    }

    pub fn orb_visible(&self) -> bool {
        (unsafe { IsWindowVisible(self.orb) }) != 0
    }
    pub fn show_orb(&mut self) -> Result<(), String> {
        if self.orb_visible() {
            self.reveal()?;
            return self.raise_orb();
        }
        if let Some(rect) = self.edge_bounds {
            self.snap(POINT {
                x: rect.left + rect.width() / 2,
                y: rect.top + rect.height() / 2,
            })?;
        }
        if self.animated() {
            if let Some(to) = self.edge_bounds {
                let offset = self.dpi as i32 * 12 / 96;
                let offset = if self.side == DockSide::Left {
                    offset
                } else {
                    -offset
                };
                let from = Rect::new(to.left + offset, to.top, to.right + offset, to.bottom)
                    .ok_or("Orb entry bounds overflow")?;
                self.draw(from, 80)?;
                self.animation = Some(Animation {
                    from,
                    to,
                    from_alpha: 80,
                    to_alpha: 255,
                    start: Instant::now(),
                    duration: Duration::from_millis(300),
                });
            }
        }
        unsafe {
            ShowWindow(self.orb, SW_SHOWNOACTIVATE);
        }
        self.raise_orb()?;
        self.last_interaction = Instant::now();
        if self.orb_visible() {
            Ok(())
        } else {
            Err(failure("Windows did not show Orb"))
        }
    }

    fn raise_orb(&self) -> Result<(), String> {
        // The small circular recovery entry stays above applications while visible.
        // Its NOACTIVATE/TOOLWINDOW styles and region still exclude focus/exterior input.
        // Main and magnetic overlays keep their separate ordinary-window Z policy.
        if unsafe {
            SetWindowPos(
                self.orb,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            )
        } == 0
        {
            return Err(failure(
                "Cannot keep the visible Orb recovery entry above applications",
            ));
        }
        if self.orb_visible() {
            Ok(())
        } else {
            Err(failure("Orb visibility was not confirmed after raise"))
        }
    }
    pub fn hide_orb(&mut self) -> Result<(), String> {
        if unsafe {
            SetWindowPos(
                self.orb,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_HIDEWINDOW,
            )
        } == 0
        {
            return Err(failure("Cannot remove Orb's visible-mode topmost state"));
        }
        self.drag = None;
        self.animation = None;
        if !self.orb_visible()
            && unsafe { GetWindowLongPtrW(self.orb, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST == 0
        {
            Ok(())
        } else {
            Err(failure("Windows did not hide Orb"))
        }
    }
    pub fn set_auto_hide(&mut self, enabled: bool) -> Result<(), String> {
        self.auto_hide = enabled;
        if !enabled {
            self.reveal()?;
        }
        Ok(())
    }

    pub fn set_motion(&mut self, motion: MotionSettings) -> Result<(), String> {
        self.motion = motion;
        if !self.animated() {
            if let Some(animation) = self.animation.take() {
                self.draw(animation.to, 255)?;
            }
        }
        Ok(())
    }

    fn animated(&self) -> bool {
        !self.motion.reduced && self.motion.intensity != MotionIntensity::Off
    }

    pub fn actions(&self) -> Vec<Action> {
        ACTIONS.with(|queue| queue.borrow_mut().drain(..).collect())
    }

    pub fn handle_pointer(&mut self, action: Action) -> Result<bool, String> {
        match action {
            Action::Press(start) => {
                self.reveal()?;
                self.animation = None;
                if self.animated() {
                    if let Some(bounds) = self.bounds {
                        self.draw(bounds, 230)?;
                    }
                }
                if let Some(bounds) = self.bounds {
                    self.drag = Some(Drag {
                        start,
                        bounds,
                        moved: false,
                    });
                }
            }
            Action::Move(point) => {
                self.last_interaction = Instant::now();
                self.reveal()?;
                if let Some(drag) = &mut self.drag {
                    let dx = point.x.saturating_sub(drag.start.x);
                    let dy = point.y.saturating_sub(drag.start.y);
                    let threshold = (self.dpi as i32 * 5 / 96).max(3);
                    drag.moved |=
                        i64::from(dx).abs().max(i64::from(dy).abs()) > i64::from(threshold);
                    if drag.moved {
                        let bounds = drag.bounds;
                        let left = bounds.left.saturating_add(dx);
                        let top = bounds.top.saturating_add(dy);
                        let rect = Rect::new(
                            left,
                            top,
                            left.saturating_add(bounds.width()),
                            top.saturating_add(bounds.height()),
                        )
                        .ok_or("Orb drag coordinates overflowed")?;
                        self.move_to(rect)?;
                    }
                }
            }
            Action::Release(point) => {
                if let Some(bounds) = self.bounds {
                    self.draw(bounds, 255)?;
                }
                if let Some(drag) = self.drag.take() {
                    if !drag.moved {
                        return Ok(true);
                    }
                    self.snap(point)?;
                }
            }
            Action::CancelDrag => {
                if self.drag.take().is_some() {
                    if let Some(point) = pointer() {
                        self.snap(point)?;
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    pub fn fix_edge(&mut self, side: DockSide) -> Result<(), String> {
        self.fixed = if self.fixed.as_ref() == Some(&side) {
            None
        } else {
            Some(side)
        };
        if let Some(rect) = self.edge_bounds {
            self.snap(POINT {
                x: rect.left + rect.width() / 2,
                y: rect.top + rect.height() / 2,
            })?;
        }
        Ok(())
    }

    pub fn tick(&mut self) -> Result<(), String> {
        self.retry_tray();
        if let Some(animation) = &self.animation {
            let t = (animation.start.elapsed().as_secs_f64() / animation.duration.as_secs_f64())
                .min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            let interpolate = |from: i32, to: i32| {
                (f64::from(from) + f64::from(to - from) * eased).round() as i32
            };
            let rect = Rect::new(
                interpolate(animation.from.left, animation.to.left),
                interpolate(animation.from.top, animation.to.top),
                interpolate(animation.from.right, animation.to.right),
                interpolate(animation.from.bottom, animation.to.bottom),
            )
            .ok_or("Orb animation geometry invalid")?;
            let alpha = (f64::from(animation.from_alpha)
                + (f64::from(animation.to_alpha) - f64::from(animation.from_alpha)) * eased)
                .round() as u8;
            self.draw(rect, alpha)?;
            if t >= 1.0 {
                self.animation = None;
            }
        }
        if self.drag.is_some() {
            // NOACTIVATE windows cannot rely on foreground capture outside their bounds.
            // Poll only the pointer/button for this own-window gesture; no keyboard hook.
            if let Some(point) = pointer() {
                self.handle_pointer(Action::Move(point))?;
                if unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } >= 0 {
                    if self.handle_pointer(Action::Release(point))? {
                        queue(Action::Expand);
                    }
                    unsafe {
                        ReleaseCapture();
                    }
                }
            }
            return Ok(());
        }
        if !self.orb_visible() {
            return Ok(());
        }
        // Monitor removal, taskbar relocation and DPI changes never retain stale coordinates.
        if let Some(rect) = self.edge_bounds {
            let point = POINT {
                x: rect.left + rect.width() / 2,
                y: rect.top + rect.height() / 2,
            };
            let (work, dpi) = monitor(point)?;
            let safe = (dpi as i32 * 8 / 96).max(1);
            let size = (dpi as i32 * 58 / 96).max(1);
            let expected = orb_edge(
                work,
                (point.x, point.y),
                size,
                safe,
                Some(self.side.clone()),
            )
            .ok_or("Orb does not fit monitor work area")?
            .0;
            if expected != rect || dpi != self.dpi {
                self.snap(point)?;
            }
        }
        if let (Some(point), Some(rect)) = (pointer(), self.bounds) {
            if rect.contains(point.x, point.y) {
                self.last_interaction = Instant::now();
                self.reveal()?;
            }
        }
        if self.auto_hide
            && !self.tucked
            && self.last_interaction.elapsed() > Duration::from_millis(1500)
        {
            if let Some(rect) = self.edge_bounds {
                let lip = (self.dpi as i32 * 14 / 96).max(1);
                let left = if self.side == DockSide::Left {
                    rect.left - rect.width() + lip
                } else {
                    rect.right - lip
                };
                self.animate_position(
                    Rect::new(left, rect.top, left + rect.width(), rect.bottom)
                        .ok_or("Orb tuck overflow")?,
                )?;
                self.tucked = true;
            }
        }
        Ok(())
    }

    fn reveal(&mut self) -> Result<(), String> {
        if self.tucked {
            if let Some(rect) = self.edge_bounds {
                self.animate_position(rect)?;
            }
            self.tucked = false;
        }
        Ok(())
    }

    fn snap(&mut self, point: POINT) -> Result<(), String> {
        let (work, dpi) = monitor(point)?;
        let size = (dpi as i32 * 58 / 96).max(1);
        let (rect, side) = orb_edge(
            work,
            (point.x, point.y),
            size,
            (dpi as i32 * 8 / 96).max(1),
            self.fixed.clone(),
        )
        .ok_or("Orb does not fit monitor work area")?;
        self.render(size, rect)?;
        self.animation = None;
        self.dpi = dpi;
        self.side = side;
        self.edge_bounds = Some(rect);
        self.tucked = false;
        self.last_interaction = Instant::now();
        self.move_to(rect)
    }

    fn move_to(&mut self, rect: Rect) -> Result<(), String> {
        if unsafe {
            SetWindowPos(
                self.orb,
                HWND_TOP,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        } == 0
        {
            return Err(failure("Cannot position Orb"));
        }
        self.bounds = Some(rect);
        Ok(())
    }

    fn draw(&mut self, rect: Rect, alpha: u8) -> Result<(), String> {
        self.bitmap
            .as_ref()
            .ok_or("Orb bitmap unavailable")?
            .draw(self.orb, rect, alpha)?;
        self.bounds = Some(rect);
        Ok(())
    }

    fn animate_position(&mut self, to: Rect) -> Result<(), String> {
        if self.animated() {
            if let Some(from) = self.bounds {
                self.animation = Some(Animation {
                    from,
                    to,
                    from_alpha: 255,
                    to_alpha: 255,
                    start: Instant::now(),
                    duration: Duration::from_millis(140),
                });
                return Ok(());
            }
        }
        self.draw(to, 255)
    }

    fn render(&mut self, size: i32, rect: Rect) -> Result<(), String> {
        if self
            .bitmap
            .as_ref()
            .is_none_or(|bitmap| bitmap.size != size)
        {
            self.bitmap = Some(OrbBitmap::new(size)?);
            let region = unsafe { CreateEllipticRgn(0, 0, size + 1, size + 1) };
            if region.is_null() {
                return Err(failure("Cannot allocate Orb hit region"));
            }
            if unsafe { SetWindowRgn(self.orb, region, 0) } == 0 {
                unsafe {
                    DeleteObject(region);
                }
                return Err(failure("Cannot configure circular Orb hit region"));
            }
            // SetWindowRgn owns region after success: exterior cannot capture input.
        }
        self.draw(rect, 255)
    }
    pub fn menu(&self, tray: bool) -> Option<Action> {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return None;
        }
        let entries: Vec<(u32, &str, bool)> = if tray {
            vec![
                (1, "显示 ArkDock", false),
                (2, "显示悬浮球", false),
                (3, "退出", false),
            ]
        } else {
            vec![
                (1, "展开 ArkDock", false),
                (4, "固定到左侧", self.fixed == Some(DockSide::Left)),
                (5, "固定到右侧", self.fixed == Some(DockSide::Right)),
                (6, "自动隐藏悬浮球", self.auto_hide),
                (7, "隐藏到托盘", false),
                (3, "退出", false),
            ]
        };
        for (id, label, checked) in entries {
            unsafe {
                AppendMenuW(
                    menu,
                    MF_STRING | if checked { MF_CHECKED } else { 0 },
                    id as usize,
                    wide(label).as_ptr(),
                );
            }
        }
        let point = pointer().unwrap_or(POINT { x: 0, y: 0 });
        // Explorer requires a foreground owner for its menu dismissal contract. The hidden
        // tray receiver is used only for explicit right-click menus; Orb clicks never activate.
        unsafe {
            SetForegroundWindow(self.receiver);
        }
        let id = unsafe {
            TrackPopupMenuEx(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                self.receiver,
                null_mut(),
            )
        };
        unsafe {
            PostMessageW(self.receiver, WM_NULL, 0, 0);
            DestroyMenu(menu);
        }
        match id {
            1 => Some(Action::Expand),
            2 => Some(Action::ShowOrb),
            3 => Some(Action::Exit),
            4 => Some(Action::FixLeft),
            5 => Some(Action::FixRight),
            6 => Some(Action::AutoHide),
            7 => Some(Action::Hide),
            _ => None,
        }
    }
}

fn monitor(point: POINT) -> Result<(Rect, u32), String> {
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    let (mut x, mut y) = (0, 0);
    if monitor.is_null()
        || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0
        || unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } < 0
        || x == 0
        || x > 768
    {
        return Err("Orb monitor or DPI cannot be verified".into());
    }
    let work = info.rcWork;
    Ok((
        Rect::new(work.left, work.top, work.right, work.bottom).ok_or("Orb work area invalid")?,
        x,
    ))
}

fn orb_pixels(size: i32) -> Vec<u32> {
    let scale = f64::from(size) / 58.0;
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = (f64::from(x) + 0.5) / scale - 29.0;
            let dy = (f64::from(y) + 0.5) / scale - 29.0;
            let radius = dx.hypot(dy);
            let coverage = ((28.5 - radius) * scale + 0.5).clamp(0.0, 1.0);
            let alpha = (coverage * 245.0).round() as u32;
            let glass = (-((dx + 8.0).powi(2) + (dy + 13.0).powi(2)) / 460.0).exp();
            let center = (1.0 - radius / 28.5).max(0.0);
            let mut color = [
                10.0 + glass * 16.0 + center * 2.0,
                21.0 + glass * 25.0 + center * 3.0,
                39.0 + glass * 35.0 + center * 5.0,
            ];
            let mut blend = |foreground: [f64; 3], amount: f64| {
                for (channel, foreground) in color.iter_mut().zip(foreground) {
                    *channel += (foreground - *channel) * amount.clamp(0.0, 1.0);
                }
            };
            let ring = ((0.5 - (radius - 27.7).abs()) * scale + 0.5).clamp(0.0, 1.0);
            let accent = (-((dx - 10.0).powi(2) + (dy + 24.0).powi(2)) / 100.0).exp();
            blend(
                [
                    79.0 + accent * 26.0,
                    116.0 + accent * 35.0,
                    157.0 + accent * 43.0,
                ],
                ring * 0.62,
            );
            // The two rounded blue gradient legs match styles.css .mark spans.
            // No crossbar, letter glyph, or vertical docking seam is added.
            for (center_x, angle, top, bottom) in [
                (
                    -4.5,
                    std::f64::consts::PI / 6.0,
                    [63.0, 105.0, 255.0],
                    [66.0, 195.0, 250.0],
                ),
                (
                    4.5,
                    -std::f64::consts::PI / 6.0,
                    [91.0, 147.0, 255.0],
                    [40.0, 116.0, 240.0],
                ),
            ] {
                let local_x = angle.cos() * (dx - center_x) + angle.sin() * dy;
                let local_y = -angle.sin() * (dx - center_x) + angle.cos() * dy;
                let cap_y = (local_y.abs() - 9.0).max(0.0);
                let distance = local_x.hypot(cap_y) - 4.0;
                let stroke = (0.5 - distance * scale).clamp(0.0, 1.0);
                let gradient = ((local_y + 13.0) / 26.0).clamp(0.0, 1.0);
                let mut ink = [0.0; 3];
                for channel in 0..3 {
                    ink[channel] = top[channel] + (bottom[channel] - top[channel]) * gradient;
                }
                blend(ink, stroke);
            }
            // Blue denotes this native recovery entry. It never claims a bound project,
            // running Previewer, or build success; those remain in the actual snapshot.
            let dot_distance = (dx - 11.0).hypot(dy - 11.0);
            blend(
                [8.0, 18.0, 33.0],
                ((3.2 - dot_distance) * scale + 0.5).clamp(0.0, 1.0),
            );
            blend(
                [78.0, 158.0, 255.0],
                ((2.05 - dot_distance) * scale + 0.5).clamp(0.0, 1.0),
            );
            let premultiply = |channel: f64| (channel * f64::from(alpha) / 255.0).round() as u32;
            pixels.push(
                alpha << 24
                    | premultiply(color[0]) << 16
                    | premultiply(color[1]) << 8
                    | premultiply(color[2]),
            );
        }
    }
    pixels
}
impl Drop for Surfaces {
    fn drop(&mut self) {
        if self.tray_available {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            }
        }
        unsafe {
            if !self.orb.is_null() {
                DestroyWindow(self.orb);
            }
            if !self.receiver.is_null() {
                DestroyWindow(self.receiver);
            }
            if !self.icon.is_null() {
                DestroyIcon(self.icon);
            }
            UnregisterClassW(self.class.as_ptr(), self.instance);
        }
        ACTIONS.with(|queue| queue.borrow_mut().clear());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_orb_is_independent_nonactivating_circular_hidden_and_released() {
        let fixture = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                wide("STATIC").as_ptr(),
                wide("ArkDock Orb resource fixture").as_ptr(),
                WS_POPUP,
                0,
                0,
                400,
                600,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null_mut()),
                null_mut(),
            )
        };
        assert!(!fixture.is_null());
        let mut surfaces =
            Surfaces::new(fixture).expect("Native test requires the Windows desktop");
        let orb = surfaces.orb;
        let receiver = surfaces.receiver;
        assert_eq!(surfaces.side, DockSide::Right);
        assert!(surfaces.fixed.is_none());
        assert!(!surfaces.orb_visible());
        assert_eq!(unsafe { GetAncestor(orb, GA_ROOT) }, orb);
        assert!(unsafe { GetWindow(orb, GW_OWNER) }.is_null());
        let style = unsafe { GetWindowLongPtrW(orb, GWL_EXSTYLE) } as u32;
        assert_eq!(
            style & (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW),
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
        );
        assert_eq!(
            unsafe { SendMessageW(orb, WM_MOUSEACTIVATE, 0, 0) },
            MA_NOACTIVATE as isize
        );
        let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
        assert!(!region.is_null());
        assert_ne!(unsafe { GetWindowRgn(orb, region) }, 0);
        let size = surfaces.bitmap.as_ref().unwrap().size;
        assert_eq!(unsafe { PtInRegion(region, 0, 0) }, 0);
        assert_ne!(unsafe { PtInRegion(region, size / 2, size / 2) }, 0);
        unsafe {
            DeleteObject(region);
        }
        let foreground = unsafe { GetForegroundWindow() };
        surfaces.show_orb().unwrap();
        assert!(surfaces.orb_visible());
        assert_ne!(
            unsafe { GetWindowLongPtrW(orb, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
            0
        );
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        surfaces.hide_orb().unwrap();
        assert!(!surfaces.orb_visible());
        assert_eq!(
            unsafe { GetWindowLongPtrW(orb, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
            0
        );
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        let tray = surfaces.tray_data();
        assert_eq!(tray.hWnd, receiver);
        assert_eq!(tray.uID, 1);
        // Repeated owned-window restart notifications must remain recoverable without
        // restarting or enumerating Explorer, and deletion is scoped to this icon.
        if surfaces.tray_available {
            surfaces.explorer_restarted();
            surfaces.explorer_restarted();
            assert!(!surfaces.tray_available);
            surfaces.retry_tray();
            assert!(surfaces.tray_available);
        }
        drop(surfaces);
        assert_eq!(unsafe { IsWindow(orb) }, 0);
        assert_eq!(unsafe { IsWindow(receiver) }, 0);
        unsafe {
            DestroyWindow(fixture);
        }
    }

    #[test]
    fn orb_logo_has_blue_gradient_legs_transparent_exterior_and_no_crossbar() {
        let size = 116;
        let pixels = orb_pixels(size);
        let pixel = |x: usize, y: usize| pixels[y * size as usize + x];
        assert_eq!(pixel(0, 0), 0);
        let color = |packed: u32| ((packed >> 16) & 255, (packed >> 8) & 255, packed & 255);
        let center = color(pixel(58, 68));
        let leg = color(pixel(48, 62));
        assert!(leg.2 > 150 && leg.2 > leg.0 + 70);
        assert!(
            center.2 < 100,
            "The center below the apex stays navy without an A crossbar"
        );
        assert!(pixels.iter().all(|packed| {
            let alpha = packed >> 24;
            ((packed >> 16) & 255) <= alpha
                && ((packed >> 8) & 255) <= alpha
                && (packed & 255) <= alpha
        }));
    }
}
