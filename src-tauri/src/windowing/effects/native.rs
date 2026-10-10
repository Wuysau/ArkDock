use super::*;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, INFINITE};
use windows_sys::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);

struct Shared {
    latest: Latest,
    wake: isize,
    handle: AtomicIsize,
    preferences: AtomicU8,
    requested: AtomicBool,
    stop: AtomicBool,
    frames: AtomicU64,
    coalesced: AtomicU64,
    failures: AtomicU64,
    max_frame_micros: AtomicU64,
    render_total_micros: AtomicU64,
    active_total_micros: AtomicU64,
    wake_count: AtomicU64,
}

impl Shared {
    fn wake(&self) {
        unsafe { SetEvent(self.wake as HANDLE) };
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.wake as HANDLE) };
    }
}

/// Counts are observational only: they do not drive native lifecycle or timing.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::windowing) struct EffectMetrics {
    pub frames: u64,
    pub coalesced_updates: u64,
    pub failed_frames: u64,
    pub max_frame_us: u64,
    pub render_total_us: u64,
    pub active_total_us: u64,
    pub wake_count: u64,
}

pub(in crate::windowing) struct Effects {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Effects {
    pub fn new() -> Result<Self, String> {
        let wake = unsafe { CreateEventW(null(), 0, 0, null()) };
        if wake.is_null() {
            return Err(format!("Cannot allocate particle wake event: {}", unsafe {
                GetLastError()
            }));
        }
        let shared = Arc::new(Shared {
            latest: Latest::new(),
            wake: wake as isize,
            handle: AtomicIsize::new(0),
            preferences: AtomicU8::new(2),
            requested: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            max_frame_micros: AtomicU64::new(0),
            render_total_micros: AtomicU64::new(0),
            active_total_micros: AtomicU64::new(0),
            wake_count: AtomicU64::new(0),
        });
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("arkdock-particles".into())
            .spawn(move || run(state, ready_tx))
            .map_err(|error| format!("Cannot start particle worker: {error}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                shared.stop.store(true, Ordering::Release);
                shared.wake();
                let _ = worker.join();
                Err(error)
            }
            Err(error) => {
                shared.stop.store(true, Ordering::Release);
                shared.wake();
                let _ = worker.join();
                Err(format!("Particle worker did not initialize: {error}"))
            }
        }
    }

    /// No waiting, pixel drawing or HWND work runs on the producer/hook thread.
    pub fn update(&self, target: Option<EffectTarget>) {
        if self.shared.preferences.load(Ordering::Acquire) == 0 {
            return;
        }
        if target.is_none() && !self.shared.requested.swap(false, Ordering::AcqRel) {
            return;
        }
        if target.is_some() {
            self.shared.requested.store(true, Ordering::Release);
        }
        self.publish(target.map(Command::Target).unwrap_or(Command::Fade));
    }

    /// Hard lifecycle cancellation. Normal candidate loss uses update(None) to fade.
    pub fn cancel(&self) {
        self.shared.requested.store(false, Ordering::Release);
        self.publish(Command::Cancel);
    }

    fn publish(&self, command: Command) {
        if self.shared.latest.publish(command) {
            self.shared.coalesced.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.wake();
    }

    pub fn set_preferences(&self, reduced: bool, intensity: MotionIntensity) {
        let value = if reduced {
            0
        } else {
            intensity_value(&intensity)
        };
        self.shared.preferences.store(value, Ordering::Release);
        if value == 0 {
            // Preserve cancellation even if preferences turn on again before the
            // worker observes the disabled value. Fresh candidates can restart it.
            self.cancel();
        } else {
            self.shared.wake();
        }
    }

    pub fn contains(&self, hwnd: HWND) -> bool {
        !hwnd.is_null() && self.shared.handle.load(Ordering::Acquire) == hwnd as isize
    }

    pub fn metrics(&self) -> EffectMetrics {
        EffectMetrics {
            frames: self.shared.frames.load(Ordering::Relaxed),
            coalesced_updates: self.shared.coalesced.load(Ordering::Relaxed),
            failed_frames: self.shared.failures.load(Ordering::Relaxed),
            max_frame_us: self.shared.max_frame_micros.load(Ordering::Relaxed),
            render_total_us: self.shared.render_total_micros.load(Ordering::Relaxed),
            active_total_us: self.shared.active_total_micros.load(Ordering::Relaxed),
            wake_count: self.shared.wake_count.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Effects {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match message {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        // An external close does not transfer ownership or leave a stale cached HWND.
        WM_CLOSE => 0,
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}

struct Surface {
    hwnd: HWND,
    instance: HINSTANCE,
    class: Vec<u16>,
    registered: bool,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
    size: (i32, i32),
    shown: bool,
}

impl Surface {
    fn new() -> Result<Self, String> {
        let instance = unsafe { GetModuleHandleW(null()) };
        let class = format!(
            "ArkDockMagneticParticles-{}",
            NEXT_CLASS.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
        let mut surface = Self {
            hwnd: null_mut(),
            instance,
            class,
            registered: false,
            dc: null_mut(),
            bitmap: null_mut(),
            previous: null_mut(),
            pixels: null_mut(),
            size: (0, 0),
            shown: false,
        };
        let mut descriptor: WNDCLASSEXW = unsafe { zeroed() };
        descriptor.cbSize = size_of::<WNDCLASSEXW>() as u32;
        descriptor.lpfnWndProc = Some(procedure);
        descriptor.hInstance = instance;
        descriptor.lpszClassName = surface.class.as_ptr();
        if unsafe { RegisterClassExW(&descriptor) } == 0 {
            return Err("Cannot register magnetic particle window".into());
        }
        surface.registered = true;
        surface.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                surface.class.as_ptr(),
                surface.class.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if surface.hwnd.is_null() {
            return Err("Cannot create magnetic particle window".into());
        }
        surface.dc = unsafe { CreateCompatibleDC(null_mut()) };
        if surface.dc.is_null() {
            return Err("Cannot allocate magnetic particle DC".into());
        }
        Ok(surface)
    }

    fn clear_bitmap(&mut self) {
        if !self.bitmap.is_null() {
            unsafe {
                SelectObject(self.dc, self.previous);
                DeleteObject(self.bitmap);
            }
            self.bitmap = null_mut();
            self.previous = null_mut();
            self.pixels = null_mut();
            self.size = (0, 0);
        }
    }

    fn resize(&mut self, width: i32, height: i32) -> bool {
        if self.size == (width, height) {
            return true;
        }
        self.clear_bitmap();
        if width <= 0 || height <= 0 || width as usize * height as usize > MAX_PIXELS {
            return false;
        }
        let mut info: BITMAPINFO = unsafe { zeroed() };
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height; // Top-down 32-bit BGRA DIB.
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(self.dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            eprintln!(
                "ArkDock particle CreateDIBSection failed: code={} DIB={width}x{height}",
                unsafe { GetLastError() }
            );
            if !bitmap.is_null() {
                unsafe { DeleteObject(bitmap) };
            }
            return false;
        }
        let previous = unsafe { SelectObject(self.dc, bitmap) };
        if previous.is_null() || previous as isize == -1 {
            eprintln!(
                "ArkDock particle SelectObject failed: code={} DIB={width}x{height}",
                unsafe { GetLastError() }
            );
            unsafe { DeleteObject(bitmap) };
            return false;
        }
        self.bitmap = bitmap;
        self.previous = previous;
        self.pixels = bits.cast();
        self.size = (width, height);
        true
    }

    fn hide(&mut self) {
        // Demote before parking too: an inactive helper must never retain a
        // visible global Z reservation after its candidate's bounded lifetime.
        let topmost = !self.hwnd.is_null()
            && unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
        if self.shown || topmost {
            if unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_HIDEWINDOW,
                )
            } == 0
            {
                eprintln!("ArkDock particle demotion failed: code={}", unsafe {
                    GetLastError()
                });
                unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            }
            self.shown = false;
        }
    }

    fn draw(&mut self, shape: Geometry, active: &Active, now: Instant, intensity: u8) -> bool {
        if !self.resize(shape.bounds.width(), shape.bounds.height()) {
            self.hide();
            return false;
        }
        let len = self.size.0 as usize * self.size.1 as usize * 4;
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.pixels, len) };
        let fade = active
            .fade
            .map(|start| fade_opacity(now.duration_since(start)))
            .unwrap_or(1.0);
        render(
            pixels,
            shape,
            &active.target,
            now.duration_since(active.started),
            fade,
            intensity,
        );
        let position = POINT {
            x: shape.bounds.left,
            y: shape.bounds.top,
        };
        let size = SIZE {
            cx: self.size.0,
            cy: self.size.1,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                null_mut(),
                &position,
                &size,
                self.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        } == 0
        {
            eprintln!(
                "ArkDock particle UpdateLayeredWindow failed: code={} bounds={:?} DIB={}x{}",
                unsafe { GetLastError() },
                shape.bounds.tuple(),
                self.size.0,
                self.size.1
            );
            self.hide();
            return false;
        }
        if !self.shown {
            // Foreground native move loops can keep the main above normal HWND_TOP
            // helpers. Reserve the upper band only while a fresh candidate or its
            // 160ms fade exists; hide always removes it with HWND_NOTOPMOST.
            if unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                )
            } == 0
            {
                eprintln!(
                    "ArkDock particle SetWindowPos failed: code={} bounds={:?}",
                    unsafe { GetLastError() },
                    shape.bounds.tuple()
                );
                self.hide();
                return false;
            }
            self.shown = true;
        }
        true
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.hide();
        self.clear_bitmap();
        unsafe {
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            if self.registered {
                UnregisterClassW(self.class.as_ptr(), self.instance);
            }
        }
    }
}

struct Active {
    target: EffectTarget,
    started: Instant,
    refreshed: Instant,
    fade: Option<Instant>,
}

fn work_area(target: &EffectTarget) -> Option<Rect> {
    // The gap chooses its display rather than the target's center; this matters
    // for windows crossing a monitor boundary or using negative coordinates.
    let edge = RECT {
        left: target.target.right.min(target.own.left),
        top: target.target.top.max(target.own.top),
        right: target.target.right.max(target.own.left).checked_add(1)?,
        bottom: target.target.bottom.min(target.own.bottom),
    };
    if edge.right <= edge.left || edge.bottom <= edge.top {
        return None;
    }
    let monitor = unsafe { MonitorFromRect(&edge, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    Rect::new(
        info.rcWork.left,
        info.rcWork.top,
        info.rcWork.right,
        info.rcWork.bottom,
    )
}

fn run(shared: Arc<Shared>, ready: mpsc::SyncSender<Result<(), String>>) {
    let previous_dpi =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous_dpi.is_null() {
        let _ = ready.send(Err(format!(
            "Cannot enable physical-pixel particle coordinates: {}",
            unsafe { GetLastError() }
        )));
        return;
    }
    let result = Surface::new();
    match result {
        Err(error) => {
            let _ = ready.send(Err(error));
        }
        Ok(mut surface) => {
            shared
                .handle
                .store(surface.hwnd as isize, Ordering::Release);
            if ready.send(Ok(())).is_ok() {
                render_loop(&shared, &mut surface);
            }
            shared.handle.store(0, Ordering::Release);
        }
    }
    if !previous_dpi.is_null() {
        unsafe { SetThreadDpiAwarenessContext(previous_dpi) };
    }
}

fn render_loop(shared: &Shared, surface: &mut Surface) {
    let mut active: Option<Active> = None;
    let mut due = Instant::now();
    let mut observed = due;
    while !shared.stop.load(Ordering::Acquire) {
        let mut message: MSG = unsafe { zeroed() };
        // This HWND's messages are serviced on its owner/render thread too.
        while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if message.message == WM_QUIT {
                return;
            }
            if message.message == WM_DISPLAYCHANGE {
                active = None;
                surface.hide();
                surface.clear_bitmap();
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let now = Instant::now();
        if active.is_some() {
            shared.active_total_micros.fetch_add(
                now.duration_since(observed)
                    .as_micros()
                    .min(u128::from(u64::MAX)) as u64,
                Ordering::Relaxed,
            );
        }
        observed = now;
        let intensity = shared.preferences.load(Ordering::Acquire);
        let pending = shared.latest.take();
        if intensity == 0 {
            active = None;
            surface.hide();
        } else if let Some(command) = pending {
            match command {
                Command::Cancel => {
                    active = None;
                    surface.hide();
                }
                Command::Fade => {
                    if let Some(current) = active.as_mut() {
                        current.fade.get_or_insert(now);
                    }
                }
                Command::Target(target) => {
                    let changed = active
                        .as_ref()
                        .is_some_and(|current| current.target.target_id != target.target_id);
                    if changed {
                        active = None;
                        surface.hide();
                    }
                    if let Some(current) = active.as_mut() {
                        current.target = target;
                        current.refreshed = now;
                        current.fade = None;
                    } else {
                        active = Some(Active {
                            target,
                            started: now,
                            refreshed: now,
                            fade: None,
                        });
                        due = now;
                    }
                }
            }
        }
        if let Some(current) = active.as_mut() {
            if now.duration_since(current.refreshed) >= CANDIDATE_FRESHNESS {
                // A stalled producer cannot hold a decorative topmost window.
                current
                    .fade
                    .get_or_insert(current.refreshed + CANDIDATE_FRESHNESS);
            }
        }
        if active.as_ref().is_some_and(|current| {
            current
                .fade
                .is_some_and(|start| now.duration_since(start) >= DISSIPATE)
        }) {
            active = None;
            surface.hide();
        }
        if let Some(current) = active.as_ref() {
            if now >= due {
                let started = Instant::now();
                let shape =
                    work_area(&current.target).and_then(|work| geometry(&current.target, work));
                if let Some(shape) = shape {
                    let ok = surface.draw(shape, current, now, intensity);
                    let micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    shared.max_frame_micros.fetch_max(micros, Ordering::Relaxed);
                    shared
                        .render_total_micros
                        .fetch_add(micros, Ordering::Relaxed);
                    if ok {
                        shared.frames.fetch_add(1, Ordering::Relaxed);
                    } else {
                        shared.failures.fetch_add(1, Ordering::Relaxed);
                        active = None;
                        surface.hide();
                    }
                } else {
                    // Candidate validation and work-area clipping may reject a
                    // transient drag sample. No presentation API has failed; a
                    // later valid sample may resume this same healthy renderer.
                    active = None;
                    surface.hide();
                }
                due = Instant::now() + FRAME_INTERVAL;
            }
        }
        let timeout = if active.is_some() {
            due.saturating_duration_since(Instant::now())
                .as_millis()
                .max(1)
                .min(u128::from(u32::MAX)) as u32
        } else {
            INFINITE
        };
        let handle = shared.wake as HANDLE;
        unsafe {
            MsgWaitForMultipleObjectsEx(1, &handle, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        shared.wake_count.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_hidden(hwnd: HWND) {
        let deadline = Instant::now() + Duration::from_millis(700);
        while unsafe { IsWindowVisible(hwnd) } != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
            0
        );
    }

    fn active_target() -> EffectTarget {
        EffectTarget {
            own: Rect::new(600, 100, 900, 600).unwrap(),
            target: Rect::new(100, 100, 600, 600).unwrap(),
            target_id: "temporary-z-regression".into(),
            distance: 0,
            threshold: 80,
            dpi: 192,
        }
    }

    fn wait_active(effects: &Effects, target: EffectTarget) -> HWND {
        let before = effects.metrics().frames;
        effects.update(Some(target));
        let deadline = Instant::now() + Duration::from_millis(500);
        while effects.metrics().frames == before && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(effects.metrics().frames > before);
        let hwnd = effects.shared.handle.load(Ordering::Acquire) as HWND;
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 1);
        assert_ne!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
            0
        );
        hwnd
    }

    #[test]
    fn temporary_upper_band_stays_above_a_normal_window_without_activation_then_demotes() {
        struct OwnedWindow(HWND);
        impl Drop for OwnedWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
            }
        }
        let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
        // Entirely off-screen owned native windows; no user window is raised,
        // moved or activated by this actual Win32 Z-order regression.
        let main = OwnedWindow(unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                -32_000,
                -32_000,
                800,
                900,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        });
        assert!(!main.0.is_null());
        assert_ne!(
            unsafe {
                SetWindowPos(
                    main.0,
                    HWND_TOP,
                    -32_000,
                    -32_000,
                    800,
                    900,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            },
            0
        );
        let foreground = unsafe { GetForegroundWindow() };
        let mut surface = Surface::new().unwrap();
        let target = EffectTarget {
            own: Rect::new(-31_500, -32_000, -31_200, -31_100).unwrap(),
            target: Rect::new(-32_000, -32_000, -31_500, -31_100).unwrap(),
            target_id: "normal-window-z-fixture".into(),
            distance: 0,
            threshold: 80,
            dpi: 192,
        };
        let shape = geometry(
            &target,
            Rect::new(-32_768, -32_768, -30_000, -30_000).unwrap(),
        )
        .unwrap();
        let now = Instant::now();
        assert!(surface.draw(
            shape,
            &Active {
                target,
                started: now,
                refreshed: now,
                fade: None
            },
            now,
            2
        ));
        // Simulate the foreground move loop's normal raise after presentation.
        assert_ne!(
            unsafe {
                SetWindowPos(
                    main.0,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )
            },
            0
        );
        let mut cursor = main.0;
        let mut above = false;
        for _ in 0..1024 {
            cursor = unsafe { GetWindow(cursor, GW_HWNDPREV) };
            if cursor.is_null() {
                break;
            }
            if cursor == surface.hwnd {
                above = true;
                break;
            }
        }
        assert!(
            above,
            "fresh decorative surface must survive a normal main-window raise"
        );
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        assert_eq!(
            unsafe { SendMessageW(surface.hwnd, WM_NCHITTEST, 0, 0) },
            HTTRANSPARENT as LRESULT
        );
        surface.hide();
        wait_hidden(surface.hwnd);
    }

    #[test]
    fn native_overlap_selects_actual_work_area_and_presents_a_clipped_dib() {
        struct DpiGuard(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
        impl Drop for DpiGuard {
            fn drop(&mut self) {
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        assert!(!previous.is_null());
        let _dpi = DpiGuard(previous);
        let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFO = unsafe { zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        assert_ne!(unsafe { GetMonitorInfoW(monitor, &mut info) }, 0);
        let work = Rect::new(
            info.rcWork.left,
            info.rcWork.top,
            info.rcWork.right,
            info.rcWork.bottom,
        )
        .unwrap();
        let own_left = work.right.checked_sub(153).unwrap();
        let target = EffectTarget {
            own: Rect::new(
                own_left,
                work.top,
                own_left.checked_add(884).unwrap(),
                work.bottom,
            )
            .unwrap(),
            target: work,
            target_id: "native-max-edge-renderer-only".into(),
            distance: 1,
            threshold: 80,
            dpi: 192,
        };
        assert_eq!(work_area(&target), Some(work));
        let shape = geometry(&target, work).unwrap();
        assert_eq!(shape.bounds.right, work.right);
        assert!(
            shape.bounds.left >= work.left
                && shape.bounds.top >= work.top
                && shape.bounds.bottom <= work.bottom
        );
        assert!(shape.left_edge > shape.right_edge);
        // Only an owned decorative HWND is presented; this is a real GDI regression,
        // without fabricating a native application binding or an ArkUI frame.
        let mut surface = Surface::new().unwrap();
        let now = Instant::now();
        assert!(surface.draw(
            shape,
            &Active {
                target,
                started: now,
                refreshed: now,
                fade: None
            },
            now,
            2
        ));
        let mut bounds: RECT = unsafe { zeroed() };
        assert_ne!(unsafe { GetWindowRect(surface.hwnd, &mut bounds) }, 0);
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            shape.bounds.tuple()
        );
        surface.hide();
        wait_hidden(surface.hwnd);
    }

    #[test]
    fn cancel_off_reduced_switch_and_stalled_producer_remove_upper_band_and_stop_frames() {
        let effects = Effects::new().unwrap();
        let hwnd = wait_active(&effects, active_target());
        effects.cancel();
        wait_hidden(hwnd);
        wait_active(&effects, active_target());
        effects.set_preferences(false, MotionIntensity::Off);
        wait_hidden(hwnd);
        effects.set_preferences(false, MotionIntensity::Standard);
        wait_active(&effects, active_target());
        effects.set_preferences(true, MotionIntensity::Enhanced);
        wait_hidden(hwnd);
        effects.set_preferences(false, MotionIntensity::Standard);
        wait_active(&effects, active_target());
        let mut invalid = active_target();
        invalid.target_id = "switched-invalid-target".into();
        invalid.distance = invalid.threshold + 1;
        effects.update(Some(invalid));
        wait_hidden(hwnd);
        wait_active(&effects, active_target());
        // No new updates: 300ms freshness + at most one 160ms fade, then park.
        wait_hidden(hwnd);
        thread::sleep(Duration::from_millis(30));
        let parked = effects.metrics();
        thread::sleep(Duration::from_millis(60));
        let idle = effects.metrics();
        assert_eq!(idle.frames, parked.frames);
        assert_eq!(idle.wake_count, parked.wake_count);
        assert_eq!(idle.failed_frames, 0);
    }

    #[test]
    fn layered_surface_resizes_and_presents_moving_native_dibs() {
        // A renderer regression only: no application binding, discovery or ArkUI
        // frame is manufactured. The owned click-through HWND is off-screen.
        let mut surface = Surface::new().unwrap();
        let mut target = EffectTarget {
            own: Rect::new(-31_850, -32_000, -31_500, -31_100).unwrap(),
            target: Rect::new(-32_700, -32_000, -31_900, -31_100).unwrap(),
            target_id: "gdi-resize-regression".into(),
            distance: 50,
            threshold: 80,
            dpi: 192,
        };
        let work = Rect::new(-32_768, -32_768, -30_000, -30_000).unwrap();
        let started = Instant::now();
        for (index, gap) in [59, 29, 0, 15, 65, 0, 40, 19, 5, 0].into_iter().enumerate() {
            target.own.left = target.target.right + gap;
            target.own.right = target.own.left + 350;
            target.own.bottom = target.target.bottom - index as i32 * 3;
            target.distance = gap;
            let shape = geometry(&target, work).unwrap();
            let now = Instant::now();
            let active = Active {
                target: target.clone(),
                started,
                refreshed: now,
                fade: None,
            };
            assert!(
                surface.draw(shape, &active, now, 2),
                "native layered presentation failed at step {index}, size={:?}",
                shape.bounds.tuple()
            );
            assert_eq!(unsafe { IsWindowVisible(surface.hwnd) }, 1);
            assert_ne!(
                unsafe { GetWindowLongPtrW(surface.hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST,
                0
            );
        }
    }

    #[test]
    fn rejected_drag_geometry_is_not_a_renderer_failure_and_a_new_valid_sample_resumes() {
        // Exercise worker validation, not a manufactured application binding.
        // A stale distance is not an API fault and cannot degrade the renderer.
        let effects = Effects::new().unwrap();
        let mut target = EffectTarget {
            own: Rect::new(600, 100, 900, 600).unwrap(),
            target: Rect::new(100, 100, 600, 600).unwrap(),
            target_id: "worker-geometry-regression".into(),
            distance: 81,
            threshold: 80,
            dpi: 192,
        };
        effects.update(Some(target.clone()));
        thread::sleep(Duration::from_millis(40));
        assert_eq!(
            effects.metrics().failed_frames,
            0,
            "candidate rejection must not permanently degrade a healthy renderer"
        );
        target.own.left = 600;
        target.distance = 0;
        effects.update(Some(target));
        let deadline = Instant::now() + Duration::from_millis(500);
        while effects.metrics().frames < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            effects.metrics().frames >= 2,
            "real layered worker must resume on valid geometry"
        );
        assert_eq!(effects.metrics().failed_frames, 0);
        effects.cancel();
    }

    #[test]
    fn dib_resources_are_replaced_and_released_without_showing_a_window() {
        let mut surface = Surface::new().unwrap();
        assert!(surface.resize(48, 80));
        let bitmap = surface.bitmap;
        assert_eq!(unsafe { GetObjectType(bitmap) }, OBJ_BITMAP as u32);
        assert!(!surface.pixels.is_null());
        assert!(surface.resize(48, 80));
        assert_eq!(surface.bitmap, bitmap);
        surface.clear_bitmap();
        assert_ne!(unsafe { GdiFlush() }, 0);
        // Type-only queries may recognize a stale encoded GDI handle. Ask for
        // actual object data to prove that the bitmap allocation was released.
        let mut bitmap_info: BITMAP = unsafe { zeroed() };
        assert_eq!(
            unsafe {
                GetObjectW(
                    bitmap,
                    size_of::<BITMAP>() as i32,
                    (&mut bitmap_info as *mut BITMAP).cast(),
                )
            },
            0
        );
        assert!(surface.bitmap.is_null());
        assert!(surface.pixels.is_null());
        assert!(!surface.resize(1024, 4096));
        assert!(surface.resize(32, 64));
        let bitmap = surface.bitmap;
        let dc = surface.dc;
        drop(surface);
        assert_ne!(unsafe { GdiFlush() }, 0);
        assert_eq!(
            unsafe {
                GetObjectW(
                    bitmap,
                    size_of::<BITMAP>() as i32,
                    (&mut bitmap_info as *mut BITMAP).cast(),
                )
            },
            0
        );
        assert!(unsafe { GetCurrentObject(dc, OBJ_BITMAP as u32) }.is_null());
    }

    #[test]
    fn native_surface_is_independent_click_through_hidden_and_destroyed_on_drop() {
        let foreground = unsafe { GetForegroundWindow() };
        let effects = Effects::new().unwrap();
        let hwnd = effects.shared.handle.load(Ordering::Acquire) as HWND;
        assert!(effects.contains(hwnd));
        assert!(!effects.contains(null_mut()));
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        let required = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        assert_eq!(style & required, required);
        assert_eq!(style & WS_EX_TOPMOST, 0);
        assert!(unsafe { GetParent(hwnd) }.is_null());
        assert!(unsafe { GetWindow(hwnd, GW_OWNER) }.is_null());
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(
            unsafe { SendMessageW(hwnd, WM_NCHITTEST, 0, 0) },
            HTTRANSPARENT as LRESULT
        );
        assert_eq!(
            unsafe { SendMessageW(hwnd, WM_MOUSEACTIVATE, 0, 0) },
            MA_NOACTIVATE as LRESULT
        );
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        drop(effects);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0);
    }

    #[test]
    fn worker_schedules_no_frames_or_wakes_when_idle_and_reduced_blocks_queued_targets() {
        let effects = Effects::new().unwrap();
        thread::sleep(Duration::from_millis(30));
        let before = effects.metrics();
        for _ in 0..1000 {
            effects.update(None);
        }
        thread::sleep(Duration::from_millis(60));
        let after = effects.metrics();
        assert_eq!(after.frames, 0);
        assert_eq!(after.wake_count, before.wake_count);
        effects.set_preferences(true, MotionIntensity::Enhanced);
        for _ in 0..1000 {
            effects.update(Some(EffectTarget {
                own: Rect::new(112, 0, 212, 300).unwrap(),
                target: Rect::new(0, 0, 100, 300).unwrap(),
                target_id: "disabled-test-input".into(),
                distance: 12,
                threshold: 64,
                dpi: 96,
            }));
        }
        thread::sleep(Duration::from_millis(30));
        let reduced = effects.metrics();
        assert_eq!(reduced.frames, 0);
        assert_eq!(reduced.failed_frames, 0);
        assert_eq!(reduced.render_total_us, 0);
        effects.set_preferences(false, MotionIntensity::Off);
        effects.update(None);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(effects.metrics().frames, 0);
        let hwnd = effects.shared.handle.load(Ordering::Acquire) as HWND;
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
    }
}
