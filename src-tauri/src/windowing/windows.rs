use super::geometry::{dock_rect, edge_score, edge_visible, side_by_side, Rect};
use super::lifecycle::{Identity, Layout};
use super::outline::Outline;
use super::WindowUpdate;
use crate::ipc::model::{PairLayoutState, PairLayoutStatus, ProcessRef, TargetInfo};
mod joint;
use joint::{JointCompensation, JointGroup};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, FILETIME, HANDLE, HWND, RECT, WAIT_TIMEOUT,
};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CALLBACK_LIMIT: usize = 2048;
const WINDOW_LIMIT: usize = 1024;
const LOOP_INTERVAL: Duration = Duration::from_millis(16);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct Event {
    kind: u32,
    handle: isize,
}

thread_local! {
    static EVENTS: RefCell<VecDeque<Event>> = RefCell::new(VecDeque::with_capacity(CALLBACK_LIMIT));
    static EVENT_GAP: Cell<bool> = const { Cell::new(false) };
}

/// OUTOFCONTEXT callbacks run on the installing thread. Never inspect windows,
/// emit UI state, call SetWindowPos or lock application state in this callback.
unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    kind: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.is_null() || (kind >= EVENT_OBJECT_CREATE && (object != OBJID_WINDOW || child != 0)) {
        return;
    }
    let relevant = matches!(
        kind,
        EVENT_OBJECT_DESTROY
            | EVENT_OBJECT_LOCATIONCHANGE
            | EVENT_OBJECT_SHOW
            | EVENT_OBJECT_HIDE
            | EVENT_OBJECT_NAMECHANGE
            | EVENT_OBJECT_CLOAKED
            | EVENT_OBJECT_UNCLOAKED
            | EVENT_SYSTEM_MOVESIZESTART
            | EVENT_SYSTEM_MOVESIZEEND
            | EVENT_SYSTEM_MINIMIZESTART
            | EVENT_SYSTEM_MINIMIZEEND
            | EVENT_SYSTEM_FOREGROUND
    );
    if !relevant {
        return;
    }
    EVENTS.with(|events| {
        if let Ok(mut events) = events.try_borrow_mut() {
            if events.len() < CALLBACK_LIMIT {
                events.push_back(Event {
                    kind,
                    handle: hwnd as isize,
                });
            } else {
                EVENT_GAP.with(|gap| gap.set(true));
            }
        } else {
            EVENT_GAP.with(|gap| gap.set(true));
        }
    });
}

enum Command {
    BeginDrag(mpsc::SyncSender<Result<(), String>>),
    Arrange(mpsc::SyncSender<Result<(), String>>),
    MaximizePair(String, mpsc::SyncSender<Result<(), String>>),
    RestorePair(String, mpsc::SyncSender<Result<(), String>>),
    #[cfg(feature = "desktop")]
    Scope(
        String,
        mpsc::SyncSender<Result<super::DiscoveryScope, String>>,
    ),
}

pub(super) struct Controller {
    commands: mpsc::SyncSender<Command>,
    detach_requested: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    snapshot: Arc<Mutex<WindowUpdate>>,
    thread: Option<JoinHandle<()>>,
}

impl Controller {
    #[cfg(feature = "desktop")]
    pub fn discovery_scope(&self, target_id: &str) -> Result<super::DiscoveryScope, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .try_send(Command::Scope(target_id.to_owned(), tx))
            .map_err(|_| "Window controller is busy".to_owned())?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Window scope request timed out".to_owned())?
    }
    pub fn start(
        own: isize,
        emit: impl Fn(WindowUpdate) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let (commands, receiver) = mpsc::sync_channel(32);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let detach_requested = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(WindowUpdate::default()));
        let thread_stop = stop.clone();
        let thread_detach = detach_requested.clone();
        let thread_snapshot = snapshot.clone();
        let thread = thread::Builder::new()
            .name("arkdock-window-events".into())
            .spawn(
                move || match Native::start(own, Arc::new(emit), thread_snapshot) {
                    Ok(mut native) => {
                        let _ = ready_tx.send(Ok(()));
                        native.run(receiver, &thread_stop, &thread_detach);
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                },
            )
            .map_err(|error| format!("Cannot start window event thread: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                commands,
                detach_requested,
                stop,
                snapshot,
                thread: Some(thread),
            }),
            result => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                match result {
                    Ok(Err(error)) => Err(error),
                    _ => Err("Window event thread did not initialize".into()),
                }
            }
        }
    }

    fn request(
        &self,
        make: impl FnOnce(mpsc::SyncSender<Result<(), String>>) -> Command,
    ) -> Result<(), String> {
        if self.stop.load(Ordering::Acquire) {
            return Err("Window controller has stopped".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.commands
            .try_send(make(tx))
            .map_err(|_| "Window controller is busy or unavailable".to_string())?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Window controller did not respond".to_string())?
    }

    pub fn begin_drag(&self) -> Result<(), String> {
        self.request(Command::BeginDrag)
    }
    pub fn arrange_side_by_side(&self) -> Result<(), String> {
        self.request(Command::Arrange)
    }
    pub fn maximize_pair(&self, target_id: &str) -> Result<(), String> {
        self.request(|reply| Command::MaximizePair(target_id.to_owned(), reply))
    }
    pub fn restore_pair(&self, target_id: &str) -> Result<(), String> {
        self.request(|reply| Command::RestorePair(target_id.to_owned(), reply))
    }
    pub fn detach(&self) {
        self.detach_requested.store(true, Ordering::Release);
    }
    pub fn current_target(&self) -> Option<TargetInfo> {
        self.snapshot
            .lock()
            .ok()
            .and_then(|state| state.target.clone())
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct TrackedWindow {
    identity: Identity,
    process: Process,
    info: TargetInfo,
}

struct Hooks(Vec<HWINEVENTHOOK>);
impl Drop for Hooks {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            unsafe {
                UnhookWinEvent(hook);
            }
        }
    }
}

struct DpiContext(DPI_AWARENESS_CONTEXT);
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

struct Candidate {
    window: TrackedWindow,
    distance: i32,
    z_index: usize,
}
struct PendingLayout {
    layout: Layout,
    deadline: Instant,
    original_target: WINDOWPLACEMENT,
    original_own: WINDOWPLACEMENT,
}

struct PendingArrangement {
    original_target: WINDOWPLACEMENT,
    original_own: WINDOWPLACEMENT,
    own_bounds: Rect,
    deadline: Instant,
}

enum CancelledOperation {
    Layout(PendingLayout),
    Arrangement(PendingArrangement),
    Joint(Box<JointGroup>),
}
struct CancelledLayout {
    target: TrackedWindow,
    operation: CancelledOperation,
    deadline: Instant,
}

struct Native {
    own: TrackedWindow,
    own_pid: u32,
    target: Option<TrackedWindow>,
    candidate: Option<Candidate>,
    dragging: bool,
    move_started: bool,
    armed_at: Option<Instant>,
    hidden_by_us: bool,
    layout: Option<PendingLayout>,
    arrangement: Option<PendingArrangement>,
    cancelled: Option<CancelledLayout>,
    joint: Option<JointGroup>,
    pair_status: PairLayoutStatus,
    epochs: HashMap<isize, u64>,
    destruction_sequence: u64,
    emit: Arc<dyn Fn(WindowUpdate) + Send + Sync>,
    snapshot: Arc<Mutex<WindowUpdate>>,
    hint: Option<String>,
    layout_required: bool,
    last_sample: Instant,
    outline: Outline,
    _hooks: Hooks,
    _dpi: DpiContext,
}

impl Native {
    fn start(
        own: isize,
        emit: Arc<dyn Fn(WindowUpdate) + Send + Sync>,
        snapshot: Arc<Mutex<WindowUpdate>>,
    ) -> Result<Self, String> {
        // Only this worker thread changes DPI context; never change global process settings.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            return Err(last_error(
                "Per-monitor physical coordinate context unavailable",
            ));
        }
        let dpi = DpiContext(previous);
        let outline = Outline::new()?;
        let own_pid = unsafe { GetCurrentProcessId() };
        let own_window = track(own, 0)?;
        if own_window.identity.pid != own_pid
            || unsafe { GetAncestor(own as HWND, GA_ROOT) } != own as HWND
            || !(unsafe { GetWindow(own as HWND, GW_OWNER) }).is_null()
            || unsafe { GetDpiForWindow(own as HWND) } == 0
        {
            return Err("Docking requires ArkDock's own independent top-level window".into());
        }
        // Create the message queue before registering out-of-context hooks.
        let mut message: MSG = unsafe { zeroed() };
        unsafe {
            PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        }
        let mut hooks = Hooks(Vec::new());
        for (first, last) in [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_UNCLOAKED),
        ] {
            // No SKIPOWNPROCESS: ArkDock's native drag start/end events are required.
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    null_mut(),
                    Some(on_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if hook.is_null() {
                return Err(last_error("Cannot install desktop window event hook"));
            }
            hooks.0.push(hook);
        }
        Ok(Self {
            own: own_window,
            own_pid,
            target: None,
            candidate: None,
            dragging: false,
            move_started: false,
            armed_at: None,
            hidden_by_us: false,
            layout: None,
            arrangement: None,
            cancelled: None,
            joint: None,
            pair_status: PairLayoutStatus::default(),
            epochs: HashMap::new(),
            destruction_sequence: 0,
            emit,
            snapshot,
            hint: None,
            layout_required: false,
            last_sample: Instant::now(),
            outline,
            _hooks: hooks,
            _dpi: dpi,
        })
    }

    fn run(&mut self, commands: mpsc::Receiver<Command>, stop: &AtomicBool, detach: &AtomicBool) {
        while !stop.load(Ordering::Acquire) {
            let mut message: MSG = unsafe { zeroed() };
            // Bound each pump iteration so a desktop event storm cannot starve stop/detach.
            for _ in 0..CALLBACK_LIMIT {
                if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                    break;
                }
                if message.message == WM_QUIT {
                    stop.store(true, Ordering::Release);
                    break;
                }
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let events = EVENTS.with(|queue| queue.borrow_mut().drain(..).collect::<Vec<_>>());
            if EVENT_GAP.with(|gap| gap.replace(false)) {
                self.invalidate("Window event continuity was lost; drag again to rebind");
                self.dragging = false;
                self.move_started = false;
                self.armed_at = None;
            }
            let mut released = false;
            for event in events {
                if matches!(
                    event.kind,
                    EVENT_SYSTEM_MOVESIZESTART | EVENT_SYSTEM_MINIMIZESTART
                ) {
                    if self.cancelled.as_ref().is_some_and(|cancelled| {
                        cancelled.target.identity.handle == event.handle
                            || self.own.identity.handle == event.handle
                    }) {
                        // The user's gesture wins even before its first changed rectangle is observable.
                        self.cancelled = None;
                    }
                    if self
                        .target
                        .as_ref()
                        .is_some_and(|target| target.identity.handle == event.handle)
                    {
                        self.layout = None;
                        self.arrangement = None;
                    }
                }
                if event.kind == EVENT_OBJECT_DESTROY {
                    self.destruction_sequence = self.destruction_sequence.saturating_add(1);
                    if self.epochs.len() >= 4096 {
                        self.invalidate("Window lifecycle tracking limit reached; drag again");
                        self.epochs.clear();
                        self.candidate = None;
                        self.dragging = false;
                    }
                    self.epochs.insert(event.handle, self.destruction_sequence);
                    if self
                        .target
                        .as_ref()
                        .is_some_and(|target| target.identity.handle == event.handle)
                    {
                        self.invalidate("Target window closed; binding was removed");
                    }
                    if self
                        .candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.window.identity.handle == event.handle)
                    {
                        self.candidate = None;
                    }
                    if self.own.identity.handle == event.handle {
                        stop.store(true, Ordering::Release);
                    }
                }
                if event.handle == self.own.identity.handle {
                    if event.kind == EVENT_SYSTEM_MOVESIZESTART {
                        if self.dragging {
                            self.move_started = true;
                            self.armed_at = None;
                        } else {
                            self.begin_joint_gesture(true);
                        }
                    }
                    if event.kind == EVENT_SYSTEM_MOVESIZEEND && self.dragging && self.move_started
                    {
                        released = true;
                    }
                    if event.kind == EVENT_SYSTEM_MOVESIZEEND && !self.dragging {
                        self.end_joint_gesture();
                    }
                }
                if self
                    .target
                    .as_ref()
                    .is_some_and(|target| target.identity.handle == event.handle)
                {
                    if event.kind == EVENT_SYSTEM_MOVESIZESTART {
                        self.begin_joint_gesture(false);
                    }
                    if event.kind == EVENT_SYSTEM_MOVESIZEEND {
                        self.end_joint_gesture();
                    }
                }
            }
            // Destroy events in the whole batch are processed before release or commands.
            if !self.valid(&self.own) {
                stop.store(true, Ordering::Release);
                break;
            }
            if detach.swap(false, Ordering::AcqRel) {
                self.detach(true);
            }
            for _ in 0..32 {
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                match command {
                    #[cfg(feature = "desktop")]
                    Command::Scope(id, reply) => {
                        let result = self
                            .target
                            .as_ref()
                            .filter(|target| target.info.id == id && self.valid(target))
                            .map(|target| super::DiscoveryScope {
                                handle: target.identity.handle,
                            })
                            .ok_or("STALE_TARGET: discovery scope is no longer live".to_owned());
                        let _ = reply.send(result);
                    }
                    Command::BeginDrag(reply) => {
                        // Moving the companion is a user layout change. Never
                        // enqueue a restore that could arrive during its drag.
                        self.detach(false);
                        self.dragging = true;
                        self.move_started = false;
                        self.armed_at = Some(Instant::now());
                        self.hint = Some("Drag beside an ordinary application window".into());
                        let _ = reply.send(Ok(()));
                    }
                    Command::Arrange(reply) => {
                        let result = self.arrange();
                        let _ = reply.send(result);
                    }
                    Command::MaximizePair(id, reply) => {
                        let result = if self
                            .target
                            .as_ref()
                            .is_some_and(|target| target.info.id == id && self.valid(target))
                        {
                            self.maximize_joint()
                        } else {
                            Err("STALE_TARGET: joint command belongs to a different window".into())
                        };
                        self.publish();
                        let _ = reply.send(result);
                    }
                    Command::RestorePair(id, reply) => {
                        let result = if self
                            .target
                            .as_ref()
                            .is_some_and(|target| target.info.id == id && self.valid(target))
                        {
                            self.restore_joint()
                        } else {
                            Err("STALE_TARGET: joint command belongs to a different window".into())
                        };
                        self.publish();
                        let _ = reply.send(result);
                    }
                }
            }
            self.restore_cancelled();
            if self.dragging {
                if self
                    .armed_at
                    .is_some_and(|time| time.elapsed() > Duration::from_secs(3))
                    && !self.move_started
                {
                    self.dragging = false;
                    self.armed_at = None;
                    self.hint = Some("Native drag did not start; binding remains detached".into());
                } else if released || self.move_started && !self.own_move_loop_active() {
                    self.release();
                } else if self.move_started && self.last_sample.elapsed() >= SAMPLE_INTERVAL {
                    self.detect();
                    self.last_sample = Instant::now();
                }
            } else if self.last_sample.elapsed() >= SAMPLE_INTERVAL || self.joint_gesture_active() {
                self.follow();
                self.last_sample = Instant::now();
            }
            self.publish();
            thread::sleep(LOOP_INTERVAL);
        }
        // Closing ArkDock only releases its resources. Do not move another app on exit.
        self.detach(false);
        self.publish();
    }

    fn epoch(&self, handle: isize) -> u64 {
        self.epochs.get(&handle).copied().unwrap_or(0)
    }

    fn own_move_loop_active(&self) -> bool {
        let mut info: GUITHREADINFO = unsafe { zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        (unsafe { GetGUIThreadInfo(self.own.identity.thread_id, &mut info) }) != 0
            && info.flags & GUI_INMOVESIZE != 0
    }

    fn valid(&self, window: &TrackedWindow) -> bool {
        let hwnd = window.identity.handle as HWND;
        // Callbacks can arrive reentrantly during a Win32 query. A destroy
        // already delivered but not yet drained must invalidate immediately.
        if EVENT_GAP.with(Cell::get)
            || EVENTS.with(|events| {
                events.borrow().iter().any(|event| {
                    event.kind == EVENT_OBJECT_DESTROY && event.handle == window.identity.handle
                })
            })
        {
            return false;
        }
        if unsafe { IsWindow(hwnd) } == 0
            || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
            || !(unsafe { GetWindow(hwnd, GW_OWNER) }).is_null()
            || unsafe { WaitForSingleObject(window.process.0, 0) } != WAIT_TIMEOUT
        {
            return false;
        }
        let mut pid = 0;
        let thread_id = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let Some(started_at) = process_start(window.process.0) else {
            return false;
        };
        window.identity.matches(Identity {
            handle: hwnd as isize,
            pid,
            started_at,
            thread_id,
            destruction_epoch: self.epoch(hwnd as isize),
        })
    }

    fn detect(&mut self) {
        self.candidate = None;
        let Some(own) = visible_bounds(self.own.identity.handle as HWND) else {
            self.outline.hide();
            self.hint = Some("Dock bounds are unavailable".into());
            return;
        };
        let mut handles = Vec::<isize>::with_capacity(WINDOW_LIMIT);
        let result =
            unsafe { EnumWindows(Some(enumerate), &mut handles as *mut Vec<isize> as isize) };
        if result == 0 {
            self.outline.hide();
            self.hint = Some("Desktop enumeration was incomplete; no binding will be made".into());
            return;
        }
        for handle in handles {
            if handle == self.own.identity.handle {
                continue;
            }
            let hwnd = handle as HWND;
            if !visible(hwnd) {
                continue;
            }
            let Some(bounds) = visible_bounds(hwnd) else {
                continue;
            };
            if eligible(hwnd, self.own_pid) {
                let threshold = scaled(hwnd, 40);
                let proximity = edge_score(own, bounds, threshold).or_else(|| {
                    // At a full-width maximized edge Windows clamps the cursor; the grabbed
                    // Dock edge cannot cross it. Use the actual drag pointer at that edge.
                    if unsafe { IsZoomed(hwnd) } == 0 {
                        return None;
                    }
                    let mut pointer = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                    if unsafe { GetCursorPos(&mut pointer) } == 0 {
                        return None;
                    }
                    let distance = (i64::from(pointer.x) - i64::from(bounds.right)).abs();
                    (distance <= i64::from(threshold)
                        && pointer.y >= bounds.top
                        && pointer.y < bounds.bottom)
                        .then_some(distance as i32)
                });
                if let Some(distance) = proximity {
                    // EnumWindows identifies candidates. Explicit GW_HWNDPREV
                    // checks z order; don't rely on undocumented enum ordering.
                    if let Some((z_index, higher)) =
                        higher_windows(hwnd, self.own.identity.handle, &self.outline)
                    {
                        if edge_visible(bounds, own, &higher) {
                            if let Ok(window) = track(handle, self.epoch(handle)) {
                                let candidate = Candidate {
                                    window,
                                    distance,
                                    z_index,
                                };
                                let better = self.candidate.as_ref().is_none_or(|best| {
                                    (distance, z_index) < (best.distance, best.z_index)
                                });
                                if better {
                                    self.candidate = Some(candidate);
                                }
                            }
                        }
                    }
                }
            }
        }
        self.hint = Some(match &self.candidate {
            Some(candidate) => format!(
                "Release to bind {} ({} physical px from its edge)",
                candidate.window.info.title, candidate.distance
            ),
            None => "No unobscured ordinary window edge is close enough".into(),
        });
        if let Some(candidate) = &self.candidate {
            if let Some(bounds) = visible_bounds(candidate.window.identity.handle as HWND) {
                if let Err(error) = self
                    .outline
                    .show(bounds, scaled(candidate.window.identity.handle as HWND, 2))
                {
                    self.hint = Some(error);
                }
            } else {
                self.outline.hide();
            }
        } else {
            self.outline.hide();
        }
    }

    fn release(&mut self) {
        // Re-enumerate at release rather than binding a stale highlighted candidate.
        self.detect();
        self.outline.hide();
        self.dragging = false;
        self.move_started = false;
        self.armed_at = None;
        if let Some(candidate) = self.candidate.take() {
            if self.valid(&candidate.window)
                && eligible(candidate.window.identity.handle as HWND, self.own_pid)
            {
                self.target = Some(candidate.window);
                self.hint = None;
                if let Err(error) = self.establish_joint() {
                    self.hint = Some(error);
                }
                self.follow();
                return;
            }
        }
        self.hint = Some("No live eligible target at release; binding remains detached".into());
    }

    fn follow(&mut self) {
        if self.follow_joint() {
            return;
        }
        let Some(target) = self.target.as_ref() else {
            return;
        };
        if !self.valid(target) {
            self.invalidate("Target lifecycle cannot be verified; binding was removed");
            return;
        }
        let target_hwnd = target.identity.handle as HWND;
        let own_minimized_or_maximized = unsafe { IsIconic(self.own.identity.handle as HWND) } != 0
            || unsafe { IsZoomed(self.own.identity.handle as HWND) } != 0;
        let current_title = window_title(target_hwnd);
        if let Some(target) = &mut self.target {
            target.info.title = current_title;
        }
        if unsafe { IsIconic(target_hwnd) } != 0 || !visible(target_hwnd) {
            self.layout = None;
            self.arrangement = None;
            if !own_minimized_or_maximized
                && !self.hidden_by_us
                && unsafe { IsWindowVisible(self.own.identity.handle as HWND) } != 0
                && unsafe { ShowWindowAsync(self.own.identity.handle as HWND, SW_HIDE) } != 0
            {
                self.hidden_by_us = true;
            }
            self.hint = Some("Target is minimized or hidden; companion waits for restore".into());
            return;
        }
        // Keep target liveness checks active while respecting the user's own
        // companion minimize/maximize state; never restore it implicitly.
        if own_minimized_or_maximized {
            self.layout = None;
            self.arrangement = None;
            return;
        }
        if !eligible(target_hwnd, self.own_pid) {
            self.invalidate("Target is no longer an eligible ordinary window");
            return;
        }
        if self.cancelled.is_some() {
            // Do not move the companion while a timed-out consent request is being compensated.
            return;
        }
        self.show_own();
        let Some(target_outer) = outer_bounds(target_hwnd) else {
            self.invalidate("Target bounds cannot be read");
            return;
        };
        let Some(own_outer) = outer_bounds(self.own.identity.handle as HWND) else {
            return;
        };
        if let Some(pending) = self.arrangement.take() {
            if Instant::now() >= pending.deadline {
                self.cancel_operation(CancelledOperation::Arrangement(pending));
                self.hint =
                    Some("Windows did not restore the target; arrangement was cancelled".into());
                return;
            }
            if unsafe { IsZoomed(target_hwnd) } != 0 {
                self.arrangement = Some(pending);
                return;
            }
            let unchanged = placement(target_hwnd)
                .is_some_and(|current| same_normal_placement(current, pending.original_target))
                && own_outer == pending.own_bounds;
            if !unchanged {
                self.hint = Some(
                    "Layout changed while restoring the target; arrangement was cancelled".into(),
                );
                return;
            }
            if let Err(error) = self.arrange_normal(pending.original_target, pending.original_own) {
                self.hint = Some(error);
            }
            return;
        }
        if let Some(pending) = &mut self.layout {
            if unsafe { IsZoomed(target_hwnd) } != 0 {
                self.layout = None;
            } else {
                pending
                    .layout
                    .observe(target_outer.tuple(), own_outer.tuple());
                if pending.layout.applied() {
                    self.hint = Some(
                        "Consent layout applied; detach restores only an unchanged layout".into(),
                    );
                } else if Instant::now() < pending.deadline {
                    self.hint = Some(
                        "Side-by-side layout requested; waiting for Windows confirmation".into(),
                    );
                    return;
                } else {
                    if let Some(pending) = self.layout.take() {
                        self.cancel_operation(CancelledOperation::Layout(pending));
                    }
                    self.hint = Some(
                        "Windows did not confirm the layout; waiting for safe cancellation".into(),
                    );
                    return;
                }
            }
        }
        let Some(target_visible) = visible_bounds(target_hwnd) else {
            self.invalidate("Target visible bounds cannot be read");
            return;
        };
        let Some(own_visible) = visible_bounds(self.own.identity.handle as HWND) else {
            return;
        };
        let Some(work) = work_area(target_hwnd) else {
            self.layout_required = true;
            self.hint = Some("Monitor work area is unavailable".into());
            return;
        };
        let gap = 0;
        let Some(desired) = dock_rect(
            target_visible,
            (own_visible.width(), own_visible.height()),
            work,
            gap,
        ) else {
            self.layout_required = true;
            self.hint =
                Some("No external space; side-by-side layout needs your explicit consent".into());
            return;
        };
        self.layout_required = false;
        // An applied consent layout is already adjacent; don't invalidate it by
        // changing its exact outer rectangles to DWM frame-based coordinates.
        if self.layout.as_ref().is_some_and(|pending| {
            pending
                .layout
                .can_restore(target_outer.tuple(), own_outer.tuple())
        }) {
            return;
        }
        if desired != own_visible {
            let Some(desired_outer) = outer_for_visible(own_outer, own_visible, desired) else {
                return;
            };
            if !self.valid(&self.own) || !self.valid(self.target.as_ref().unwrap()) {
                self.invalidate("Window lifecycle changed before positioning");
                return;
            }
            if let Err(error) = position(self.own.identity.handle as HWND, desired_outer) {
                self.hint = Some(error);
            }
        }
        if self.layout.is_none()
            && self.hint.as_deref().is_some_and(|hint| {
                hint.starts_with("Target is minimized") || hint.starts_with("No external space")
            })
        {
            self.hint = None;
        }
    }

    fn arrange(&mut self) -> Result<(), String> {
        if self.dragging {
            return Err("Finish dragging before requesting a layout".into());
        }
        let target = self.target.as_ref().ok_or("Bind a target window first")?;
        if !self.valid(target) || !self.valid(&self.own) {
            return Err("Window lifecycle is no longer verifiable".into());
        }
        let hwnd = target.identity.handle as HWND;
        if unsafe { IsIconic(hwnd) } != 0 || !eligible(hwnd, self.own_pid) {
            return Err("Restore a minimized target before arranging".into());
        }
        if self.layout.is_some() || self.arrangement.is_some() || self.cancelled.is_some() {
            return Err("Detach the current consent layout before requesting another".into());
        }
        let original_target = placement(hwnd).ok_or("Target placement unavailable")?;
        let original_own =
            placement(self.own.identity.handle as HWND).ok_or("Companion placement unavailable")?;
        if unsafe { IsIconic(self.own.identity.handle as HWND) } != 0
            || unsafe { IsZoomed(self.own.identity.handle as HWND) } != 0
        {
            return Err("Restore the companion to a normal window before arranging".into());
        }
        if unsafe { IsZoomed(hwnd) } != 0 {
            let own_bounds = outer_bounds(self.own.identity.handle as HWND)
                .ok_or("Companion bounds unavailable")?;
            if unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } == 0 {
                return Err(last_error("Cannot restore target after consent"));
            }
            self.arrangement = Some(PendingArrangement {
                original_target,
                original_own,
                own_bounds,
                deadline: Instant::now() + Duration::from_millis(750),
            });
            self.hint = Some("Restoring maximized target after explicit layout consent".into());
            return Ok(());
        }
        self.arrange_normal(original_target, original_own)
    }

    fn arrange_normal(
        &mut self,
        original_target: WINDOWPLACEMENT,
        original_own: WINDOWPLACEMENT,
    ) -> Result<(), String> {
        let target = self
            .target
            .as_ref()
            .ok_or("Target was detached before arrangement")?;
        let hwnd = target.identity.handle as HWND;
        let target_outer = outer_bounds(hwnd).ok_or("Target bounds unavailable")?;
        let own_outer =
            outer_bounds(self.own.identity.handle as HWND).ok_or("Companion bounds unavailable")?;
        let target_visible = visible_bounds(hwnd).ok_or("Target visible bounds unavailable")?;
        let own_visible = visible_bounds(self.own.identity.handle as HWND)
            .ok_or("Companion visible bounds unavailable")?;
        let work = work_area(hwnd).ok_or("Monitor work area unavailable")?;
        let (target_slot, own_slot) = side_by_side(
            work,
            own_visible.width(),
            scaled(hwnd, 320),
            scaled(hwnd, 280),
            0,
        )
        .ok_or("Monitor is too narrow for both minimum window widths")?;
        let target_expected = outer_for_visible(target_outer, target_visible, target_slot)
            .ok_or("Target frame geometry overflow")?;
        let own_expected = outer_for_visible(own_outer, own_visible, own_slot)
            .ok_or("Companion frame geometry overflow")?;
        if !self.valid(target) || !self.valid(&self.own) {
            return Err("Window lifecycle changed before arrangement".into());
        }
        // The only target mutation in this module; reached only through explicit consent.
        position(hwnd, target_expected)?;
        if let Err(error) = position(self.own.identity.handle as HWND, own_expected) {
            // Record the accepted target request even for a partial failure.
            // Restoration still requires observing its exact expected state
            // while the companion remains at its untouched original rectangle.
            self.layout = Some(PendingLayout {
                layout: Layout::new(
                    target_outer.tuple(),
                    own_outer.tuple(),
                    target_expected.tuple(),
                    own_outer.tuple(),
                ),
                deadline: Instant::now() + Duration::from_millis(750),
                original_target,
                original_own,
            });
            self.hint = Some("Target layout request accepted, but companion request failed; waiting to verify the partial layout".into());
            return Err(error);
        }
        self.layout = Some(PendingLayout {
            layout: Layout::new(
                target_outer.tuple(),
                own_outer.tuple(),
                target_expected.tuple(),
                own_expected.tuple(),
            ),
            deadline: Instant::now() + Duration::from_millis(750),
            original_target,
            original_own,
        });
        self.layout_required = false;
        self.hint = Some("Side-by-side layout requested; waiting for Windows confirmation".into());
        Ok(())
    }

    fn show_own(&mut self) {
        if self.hidden_by_us
            && self.valid(&self.own)
            && unsafe { ShowWindowAsync(self.own.identity.handle as HWND, SW_SHOWNOACTIVATE) } != 0
        {
            self.hidden_by_us = false;
        }
    }

    fn detach(&mut self, restore_layout: bool) {
        self.outline.hide();
        if !restore_layout {
            // A new user drag, lifecycle loss or exit must never queue old layout writes.
            self.cancelled = None;
        } else if let Some(target) = self.target.take() {
            let operation = self
                .layout
                .take()
                .map(CancelledOperation::Layout)
                .or_else(|| self.arrangement.take().map(CancelledOperation::Arrangement))
                .or_else(|| {
                    self.joint
                        .take()
                        .map(|group| CancelledOperation::Joint(Box::new(group)))
                });
            if let Some(operation) = operation {
                // An accepted User32 request cannot be recalled. Keep its identity and observe
                // it after detaching, rather than losing a late request's restoration ownership.
                self.cancelled = Some(CancelledLayout {
                    target,
                    operation,
                    deadline: Instant::now() + Duration::from_secs(3),
                });
            }
        }
        self.target = None;
        self.joint = None;
        self.pair_status = PairLayoutStatus::default();
        self.candidate = None;
        self.layout = None;
        self.arrangement = None;
        self.dragging = false;
        self.move_started = false;
        self.armed_at = None;
        self.layout_required = false;
        self.hint = None;
        self.show_own();
        self.restore_cancelled();
    }

    fn restore_cancelled(&mut self) {
        let Some(mut cancelled) = self.cancelled.take() else {
            return;
        };
        let target = &cancelled.target;
        let target_hwnd = target.identity.handle as HWND;
        let own_hwnd = self.own.identity.handle as HWND;
        if Instant::now() >= cancelled.deadline
            || !self.valid(target)
            || !self.valid(&self.own)
            || !eligible(target_hwnd, self.own_pid)
            || !visible(own_hwnd)
            || unsafe { IsIconic(target_hwnd) } != 0
            || unsafe { IsIconic(own_hwnd) } != 0
            || unsafe { IsZoomed(own_hwnd) } != 0
        {
            return;
        }
        let (Some(current_target), Some(current_own)) =
            (outer_bounds(target_hwnd), outer_bounds(own_hwnd))
        else {
            return;
        };
        let originals = match &mut cancelled.operation {
            CancelledOperation::Joint(group) => match self.joint_compensation(group, target) {
                JointCompensation::Waiting => {
                    self.cancelled = Some(cancelled);
                    return;
                }
                JointCompensation::Finished => return,
                JointCompensation::Restore(target, own, original) => {
                    if self.valid(&cancelled.target) {
                        if target.showCmd == SW_SHOWMAXIMIZED as u32 {
                            if let (Some(outer), Some(visible)) =
                                (outer_bounds(target_hwnd), visible_bounds(target_hwnd))
                            {
                                if let Some(desired) =
                                    outer_for_visible(outer, visible, original.target)
                                {
                                    if self.valid(&cancelled.target) {
                                        let _ = position(target_hwnd, desired);
                                    }
                                }
                            }
                        } else {
                            let _ = restore_placement(target_hwnd, target);
                        }
                    }
                    if self.valid(&self.own) {
                        let _ = restore_placement(own_hwnd, own);
                    }
                    self.cancelled = Some(cancelled);
                    return;
                }
            },
            CancelledOperation::Arrangement(pending) => {
                if current_own != pending.own_bounds
                    || !placement(target_hwnd).is_some_and(|current| {
                        same_normal_placement(current, pending.original_target)
                    })
                {
                    return;
                }
                if unsafe { IsZoomed(target_hwnd) } != 0 {
                    self.cancelled = Some(cancelled);
                    return;
                }
                (pending.original_target, None)
            }
            CancelledOperation::Layout(pending) => {
                if unsafe { IsZoomed(target_hwnd) } != 0 {
                    return;
                }
                // Detach may run before follow() has observed already applied rectangles.
                pending
                    .layout
                    .observe(current_target.tuple(), current_own.tuple());
                if pending
                    .layout
                    .restore_bounds(current_target.tuple(), current_own.tuple())
                    .is_some()
                {
                    (pending.original_target, Some(pending.original_own))
                } else if pending
                    .layout
                    .awaiting_expected(current_target.tuple(), current_own.tuple())
                {
                    self.cancelled = Some(cancelled);
                    return;
                } else {
                    return;
                }
            }
        };
        // Lifecycle and exact expected state have been re-read; never restore by PID or HWND alone.
        if self.valid(target) {
            let _ = restore_placement(target_hwnd, originals.0);
        }
        if let Some(original_own) = originals.1 {
            if self.valid(&self.own) {
                let _ = restore_placement(own_hwnd, original_own);
            }
        }
    }

    fn cancel_operation(&mut self, operation: CancelledOperation) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        if !self.valid(target) {
            return;
        }
        // Hold a separately owned read-only process handle while keeping the live binding.
        // Refuse any observed identity change, including delivered HWND destruction/reuse.
        let Ok(tracked) = track(target.identity.handle, target.identity.destruction_epoch) else {
            return;
        };
        if tracked.identity != target.identity || !self.valid(&tracked) {
            return;
        }
        self.cancelled = Some(CancelledLayout {
            target: tracked,
            operation,
            deadline: Instant::now() + Duration::from_secs(3),
        });
    }

    fn invalidate(&mut self, reason: &str) {
        // Lost lifecycle: no target layout restoration or target operation is allowed.
        self.detach(false);
        self.hint = Some(reason.into());
    }

    fn publish(&self) {
        let update = WindowUpdate {
            target: self.target.as_ref().map(|target| target.info.clone()),
            dock_hint: self.hint.clone(),
            layout_required: self.layout_required,
            pair_layout: self.pair_status.clone(),
        };
        let changed = if let Ok(mut state) = self.snapshot.lock() {
            let same_target = match (&state.target, &update.target) {
                (None, None) => true,
                (Some(old), Some(new)) => old.id == new.id && old.title == new.title,
                _ => false,
            };
            let changed = !same_target
                || state.dock_hint != update.dock_hint
                || state.layout_required != update.layout_required;
            let changed = changed || state.pair_layout != update.pair_layout;
            if changed {
                *state = update.clone();
            }
            changed
        } else {
            false
        };
        if changed {
            (self.emit)(update);
        }
    }
}

unsafe extern "system" fn enumerate(hwnd: HWND, parameter: isize) -> i32 {
    let windows = &mut *(parameter as *mut Vec<isize>);
    if windows.len() >= WINDOW_LIMIT {
        return 0;
    }
    windows.push(hwnd as isize);
    1
}

fn process_start(handle: HANDLE) -> Option<u64> {
    let mut created: FILETIME = unsafe { zeroed() };
    let mut exited: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return None;
    }
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn placement(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
    let mut value: WINDOWPLACEMENT = unsafe { zeroed() };
    value.length = size_of::<WINDOWPLACEMENT>() as u32;
    (unsafe { GetWindowPlacement(hwnd, &mut value) } != 0).then_some(value)
}

fn same_normal_placement(a: WINDOWPLACEMENT, b: WINDOWPLACEMENT) -> bool {
    let a = a.rcNormalPosition;
    let b = b.rcNormalPosition;
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

fn restore_placement(hwnd: HWND, mut original: WINDOWPLACEMENT) -> Result<(), String> {
    // These are workspace coordinates from GetWindowPlacement, never screen rectangles.
    original.flags |= WPF_ASYNCWINDOWPLACEMENT;
    if original.showCmd == SW_SHOWNORMAL as u32 {
        original.showCmd = SW_SHOWNOACTIVATE as u32;
    }
    if unsafe { SetWindowPlacement(hwnd, &original) } == 0 {
        return Err(last_error("Window placement restoration failed"));
    }
    Ok(())
}

fn track(handle: isize, destruction_epoch: u64) -> Result<TrackedWindow, String> {
    let hwnd = handle as HWND;
    if handle == 0 || unsafe { IsWindow(hwnd) } == 0 {
        return Err("Window no longer exists".into());
    }
    let mut pid = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || thread_id == 0 {
        return Err("Window process identity unavailable".into());
    }
    let raw_process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if raw_process.is_null() {
        return Err(last_error("Process lifecycle cannot be queried"));
    }
    let process = Process(raw_process);
    if unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT {
        return Err("Window process has exited".into());
    }
    let started_at = process_start(process.0).ok_or("Process creation time unavailable")?;
    // GetWindowText sends a synchronous WM_GETTEXT within the same process.
    // Never query our main thread while it may be waiting for controller startup.
    let title = if pid == unsafe { GetCurrentProcessId() } {
        "ArkDock".into()
    } else {
        window_title(hwnd)
    };
    let mut executable = vec![0u16; 32768];
    let mut executable_length = executable.len() as u32;
    let app = if unsafe {
        QueryFullProcessImageNameW(
            process.0,
            0,
            executable.as_mut_ptr(),
            &mut executable_length,
        )
    } != 0
    {
        let full = String::from_utf16_lossy(&executable[..executable_length as usize]);
        full.rsplit(['\\', '/'])
            .next()
            .unwrap_or("Unknown application")
            .to_string()
    } else {
        "Application identity unavailable".into()
    };
    // PID/thread are rechecked after metadata reads. Every operation also checks
    // retained process liveness, creation time and the delivered destroy epoch.
    let mut observed_pid = 0;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut observed_pid) } != thread_id
        || observed_pid != pid
    {
        return Err("Window changed during identity inspection".into());
    }
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    Ok(TrackedWindow {
        identity: Identity {
            handle,
            pid,
            started_at,
            thread_id,
            destruction_epoch,
        },
        process,
        info: TargetInfo {
            id: format!("window-{token}"),
            title,
            app,
            process: ProcessRef {
                pid,
                started_at: started_at.to_string(),
            },
        },
    })
}

fn visible(hwnd: HWND) -> bool {
    if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
        return false;
    }
    let mut cloaked = 0u32;
    // Failed DWM visibility inspection is excluded rather than guessed visible.
    let status = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        )
    };
    status >= 0 && cloaked == 0
}

fn eligible(hwnd: HWND, own_pid: u32) -> bool {
    if !visible(hwnd)
        || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || !(unsafe { GetWindow(hwnd, GW_OWNER) }).is_null()
    {
        return false;
    }
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    if pid == 0 || pid == own_pid || unsafe { GetDpiForWindow(hwnd) } == 0 {
        return false;
    }
    // Window style fields are 32-bit on every architecture. GetWindowLongW is
    // present on x86 too; the pinned bindings expose LongPtr only on 64-bit.
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    if style & WS_CHILD != 0 || extended & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 {
        return false;
    }
    let mut class = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if length <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&class[..length as usize]);
    !matches!(
        class.as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "#32768"
            | "tooltips_class32"
    )
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 1024];
    let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length > 0 {
        String::from_utf16_lossy(&buffer[..length as usize])
    } else {
        "Untitled window".into()
    }
}

fn higher_windows(target: HWND, own: isize, outline: &Outline) -> Option<(usize, Vec<Rect>)> {
    let mut cursor = target;
    let mut visited = HashSet::new();
    let mut rectangles = Vec::new();
    for index in 0..WINDOW_LIMIT {
        cursor = unsafe { GetWindow(cursor, GW_HWNDPREV) };
        if cursor.is_null() {
            return Some((index, rectangles));
        }
        if !visited.insert(cursor as isize) || unsafe { IsWindow(cursor) } == 0 {
            return None;
        }
        if cursor as isize != own && !outline.contains(cursor) && visible(cursor) {
            // A visible higher window with unreadable bounds is an unknown
            // occluder, so reject the candidate rather than assume it is clear.
            rectangles.push(visible_bounds(cursor)?);
        }
    }
    None
}

fn from_native(rect: RECT) -> Option<Rect> {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}
fn outer_bounds(hwnd: HWND) -> Option<Rect> {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    from_native(rect)
}

fn visible_bounds(hwnd: HWND) -> Option<Rect> {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut rect as *mut RECT as *mut _,
            size_of::<RECT>() as u32,
        )
    } < 0
    {
        return None;
    }
    let frame = from_native(rect)?;
    if unsafe { IsZoomed(hwnd) } != 0 {
        let outer = outer_bounds(hwnd)?;
        // Some custom-frame apps return the maximized invisible sizing border as
        // DWMWA_EXTENDED_FRAME_BOUNDS. Derive that border from real client geometry.
        if frame.left == outer.left && frame.right == outer.right {
            let mut client: RECT = unsafe { zeroed() };
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            if unsafe { GetClientRect(hwnd, &mut client) } != 0
                && unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin) }
                    != 0
            {
                let left = (origin.x - frame.left).max(0);
                let right = (frame.right - origin.x - client.right).max(0);
                let top = (origin.y - frame.top).max(0).min(left.max(right));
                let bottom = (frame.bottom - origin.y - client.bottom).max(0);
                return Rect::new(
                    frame.left + left,
                    frame.top + top,
                    frame.right - right,
                    frame.bottom - bottom,
                );
            }
        }
    }
    Some(frame)
}

fn work_area(hwnd: HWND) -> Option<Rect> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    from_native(info.rcWork)
}

fn scaled(hwnd: HWND, logical: i32) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    ((i64::from(logical) * i64::from(dpi) + 48) / 96).min(i64::from(i32::MAX)) as i32
}

fn outer_for_visible(outer: Rect, visible: Rect, desired: Rect) -> Option<Rect> {
    Rect::new(
        desired
            .left
            .checked_add(outer.left.checked_sub(visible.left)?)?,
        desired
            .top
            .checked_add(outer.top.checked_sub(visible.top)?)?,
        desired
            .right
            .checked_add(outer.right.checked_sub(visible.right)?)?,
        desired
            .bottom
            .checked_add(outer.bottom.checked_sub(visible.bottom)?)?,
    )
}

fn position(hwnd: HWND, rect: Rect) -> Result<(), String> {
    // Preserve existing Z order and owners. No activation, parenting or topmost
    // state changes; async prevents a hung foreign UI thread blocking cleanup.
    let flags = SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
    if unsafe {
        SetWindowPos(
            hwnd,
            null_mut(),
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            flags,
        )
    } == 0
    {
        return Err(last_error("Windows rejected window positioning"));
    }
    Ok(())
}

fn last_error(context: &str) -> String {
    format!("{context} (Win32 error {})", unsafe { GetLastError() })
}
