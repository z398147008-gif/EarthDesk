//! Win32 glue. Declared by hand against user32 so the crate carries no
//! `windows` dependency; everything here is Windows-only and compiles to
//! no-ops elsewhere.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::Mutex;
    use tauri::WebviewWindow;

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, window: *const u16) -> isize;
        fn FindWindowExW(parent: isize, after: isize, class: *const u16, window: *const u16) -> isize;
        fn SendMessageTimeoutW(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> isize;
        fn SetParent(child: isize, parent: isize) -> isize;
        fn GetAncestor(hwnd: isize, flags: u32) -> isize;
        fn GetDesktopWindow() -> isize;
        fn IsWindow(hwnd: isize) -> i32;
        fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn GetSystemMetrics(index: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn GetWindow(hwnd: isize, cmd: u32) -> isize;
        fn GetClassNameW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
        fn GetTopWindow(hwnd: isize) -> isize;
        fn GetWindowTextLengthW(hwnd: isize) -> i32;
        fn GetForegroundWindow() -> isize;
        fn GetWindowRect(hwnd: isize, rect: *mut RawRect) -> i32;
        fn SetWinEventHook(
            event_min: u32,
            event_max: u32,
            module: isize,
            callback: Option<WinEventProc>,
            pid: u32,
            tid: u32,
            flags: u32,
        ) -> isize;
    }

    type WinEventProc = unsafe extern "system" fn(
        hook: isize,
        event: u32,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        thread: u32,
        time: u32,
    );

    #[repr(C)]
    #[derive(Default)]
    struct RawRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: isize, attr: u32, out: *mut u32, size: u32) -> i32;
    }

    const GWL_STYLE: i32 = -16;
    const WS_VISIBLE: isize = 0x1000_0000;
    const GW_HWNDNEXT: u32 = 2;
    const GW_HWNDPREV: u32 = 3;
    const GW_OWNER: u32 = 4;
    const WS_EX_TOPMOST: isize = 0x0000_0008;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const DWMWA_CLOAKED: u32 = 14;

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;

    const HWND_TOP: isize = 0;
    const HWND_BOTTOM: isize = 1;
    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;

    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    /// GetAncestor(GA_PARENT): the real parent window.
    ///
    /// Not GetParent. For a pop-up style window -- which is what a frameless
    /// Tauri window is -- GetParent answers with the *owner*, and ours have
    /// none, so it said "no parent" even while the window was sitting inside
    /// WorkerW. The layer watchdog took that at its word and re-parented the
    /// wallpaper on every tick, and each re-parent is a visible flash. That was
    /// the periodic flicker; the interval only ever changed how often it came.
    const GA_PARENT: u32 = 1;

    fn parent_of(hwnd: isize) -> isize {
        unsafe { GetAncestor(hwnd, GA_PARENT) }
    }

    const SM_CXSCREEN: i32 = 0;
    const SM_CYSCREEN: i32 = 1;
    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;

    /// Undocumented Progman message that asks the shell to spawn the WorkerW
    /// window which normally hosts the wallpaper bitmap.
    const WM_SPAWN_WORKER: u32 = 0x052C;

    /// The shell's wallpaper window, once we have found it.
    ///
    /// Caching matters for more than speed: poking Progman with 0x052C makes
    /// the shell tear down and rebuild its wallpaper surface, which the user
    /// sees as a flash. Doing that on a timer is the difference between a
    /// desktop and a strobe light, so the probe runs once and the cached
    /// handle is reused until the window actually goes away.
    static HOST: AtomicIsize = AtomicIsize::new(0);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn handle(win: &WebviewWindow) -> Option<isize> {
        // `.0` is a raw pointer on current windows-rs and an isize on older
        // ones; `as isize` is a valid cast from either, and never names the
        // type, so this crate needs no `windows` dependency of its own.
        win.hwnd().ok().map(|h| h.0 as isize)
    }

    /// Keep a widget out of Alt+Tab and stop it stealing focus.
    pub fn mark_as_widget(win: &WebviewWindow) {
        let Some(h) = handle(win) else { return };
        unsafe {
            let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
        }
    }

    pub fn set_topmost(win: &WebviewWindow, on: bool) {
        let Some(h) = handle(win) else { return };
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            SetWindowPos(h, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Bottom of the normal z-order: above the desktop, below every app window.
    pub fn sink_to_bottom(win: &WebviewWindow) {
        let Some(h) = handle(win) else { return };
        unsafe {
            SetWindowPos(h, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// The bounding box of all monitors, in physical pixels.
    pub fn virtual_screen() -> (i32, i32, i32, i32) {
        unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            )
        }
    }

    /// The primary monitor, which Windows always anchors at the origin.
    pub fn primary_screen() -> (i32, i32, i32, i32) {
        unsafe {
            (0, 0, GetSystemMetrics(SM_CXSCREEN).max(1), GetSystemMetrics(SM_CYSCREEN).max(1))
        }
    }

    /// Find the WorkerW that sits *behind* the desktop icons.
    ///
    /// Explorer keeps the icons in a `SHELLDLL_DefView` child. Poking Progman
    /// with 0x052C makes it split itself into two WorkerW siblings: the one
    /// holding `SHELLDLL_DefView`, and an empty one underneath it that the
    /// wallpaper is painted into. That empty sibling is what we want as a
    /// parent -- a child of it draws under the icons.
    unsafe fn probe_host() -> Option<isize> {
        let progman = FindWindowW(wide("Progman").as_ptr(), std::ptr::null());
        if progman == 0 {
            return None;
        }
        let mut ignored: usize = 0;
        SendMessageTimeoutW(progman, WM_SPAWN_WORKER, 0, 0, 0x0000, 1000, &mut ignored);

        let class = wide("WorkerW");
        let defview = wide("SHELLDLL_DefView");
        let mut after: isize = 0;
        loop {
            let worker = FindWindowExW(0, after, class.as_ptr(), std::ptr::null());
            if worker == 0 {
                break;
            }
            if FindWindowExW(worker, 0, defview.as_ptr(), std::ptr::null()) != 0 {
                let sibling = FindWindowExW(0, worker, class.as_ptr(), std::ptr::null());
                if sibling != 0 {
                    return Some(sibling);
                }
            }
            after = worker;
        }

        // Some Win11 builds keep DefView directly under Progman and never
        // spawn the second WorkerW. Parenting to Progman still puts us behind
        // the icons there.
        Some(progman)
    }

    unsafe fn wallpaper_host() -> Option<isize> {
        let cached = HOST.load(Ordering::Relaxed);
        if cached != 0 && IsWindow(cached) != 0 {
            return Some(cached);
        }
        let found = probe_host()?;
        HOST.store(found, Ordering::Relaxed);
        Some(found)
    }

    /// True when the window already lives in the shell's wallpaper layer, so
    /// callers can skip a re-parent that would cost a visible flash.
    pub fn is_in_wallpaper_layer(win: &WebviewWindow) -> bool {
        let Some(h) = handle(win) else { return false };
        let cached = HOST.load(Ordering::Relaxed);
        cached != 0 && parent_of(h) == cached
    }

    /// Re-parent the wallpaper window into the desktop and stretch it over the
    /// whole virtual screen. Returns false if the shell did not cooperate, in
    /// which case the caller should fall back to a plain bottom-of-z window.
    pub fn attach_wallpaper(win: &WebviewWindow) -> bool {
        let Some(h) = handle(win) else { return false };
        unsafe {
            let Some(host) = wallpaper_host() else { return false };
            if parent_of(h) == host {
                return true;
            }
            if SetParent(h, host) == 0 {
                return false;
            }
            // Child coordinates are relative to the host, whose origin is the
            // top-left of the virtual screen.
            let (_, _, w, h_px) = virtual_screen();
            SetWindowPos(h, 0, 0, 0, w, h_px, SWP_NOACTIVATE | SWP_NOZORDER);
            true
        }
    }

    /// Park a widget inside another of our windows -- in practice the
    /// wallpaper -- so it rides along in the desktop layer: above the
    /// wallpaper, below the desktop icons, and below every application window.
    pub fn attach_to(win: &WebviewWindow, host: &WebviewWindow) -> bool {
        let (Some(h), Some(host_h)) = (handle(win), handle(host)) else { return false };
        unsafe {
            if parent_of(h) == host_h {
                return true;
            }
            if SetParent(h, host_h) == 0 {
                return false;
            }
            // Top of the host's children, i.e. drawn over the globe.
            SetWindowPos(h, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            true
        }
    }

    /// Give a window back to the desktop, making it a normal top-level window
    /// again. Needed while the user is arranging widgets: a child of the
    /// wallpaper layer sits behind the icon view and never sees the mouse.
    pub fn detach(win: &WebviewWindow) {
        let Some(h) = handle(win) else { return };
        unsafe {
            if parent_of(h) != GetDesktopWindow() {
                SetParent(h, 0);
            }
        }
    }

    pub fn is_detached(win: &WebviewWindow) -> bool {
        let Some(h) = handle(win) else { return true };
        unsafe { parent_of(h) == GetDesktopWindow() }
    }

    fn cloaked(hwnd: isize) -> bool {
        let mut value: u32 = 0;
        // S_OK == 0. A window the DWM knows nothing about just reads as not cloaked.
        let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut value, 4) };
        hr == 0 && value != 0
    }

    fn class_of(hwnd: isize) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// Is any *visible* sibling stacked above `h` that is not one of ours?
    /// Inside the wallpaper window the only other child is the WebView2 host
    /// that paints the globe, and if that ends up on top it covers the widgets
    /// completely while every "is it visible?" question still answers yes.
    unsafe fn covered_by_stranger(h: isize, friends: &[isize]) -> bool {
        let mut above = GetWindow(h, GW_HWNDPREV);
        // The chain is finite, but never trust a window list another process
        // is rearranging while we walk it.
        for _ in 0..64 {
            if above == 0 {
                return false;
            }
            if !friends.contains(&above) && IsWindowVisible(above) != 0 {
                return true;
            }
            above = GetWindow(above, GW_HWNDPREV);
        }
        false
    }

    /// Everything that can make a parked widget vanish without the window
    /// itself going away. See `WidgetState`.
    pub fn widget_state(
        win: &WebviewWindow,
        host: &WebviewWindow,
        friends: &[&WebviewWindow],
    ) -> Option<super::WidgetState> {
        let h = handle(win)?;
        let host_h = handle(host)?;
        let mates: Vec<isize> = friends.iter().filter_map(|w| handle(w)).collect();
        unsafe {
            Some(super::WidgetState {
                visible: IsWindowVisible(h) != 0,
                own_visible: GetWindowLongPtrW(h, GWL_STYLE) & WS_VISIBLE != 0,
                iconic: IsIconic(h) != 0,
                cloaked: cloaked(h),
                in_host: parent_of(h) == host_h,
                covered: covered_by_stranger(h, &mates),
            })
        }
    }

    /// Show without activating -- a widget must never take focus.
    pub fn show_noactivate(win: &WebviewWindow) {
        let Some(h) = handle(win) else { return };
        unsafe {
            ShowWindow(h, SW_SHOWNOACTIVATE);
        }
    }

    /// Back to the top of the siblings inside the wallpaper window.
    pub fn raise(win: &WebviewWindow) {
        let Some(h) = handle(win) else { return };
        unsafe {
            SetWindowPos(h, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn is_desktop_class(hwnd: isize) -> bool {
        let c = class_of(hwnd);
        c == "Progman" || c == "WorkerW"
    }

    /// A top-level window the user would call an open application: the same
    /// test the taskbar and Alt+Tab apply. Cloaked ones matter -- a suspended
    /// UWP app keeps a visible, full-size frame that nobody can see.
    unsafe fn is_app_window(h: isize) -> bool {
        if IsWindowVisible(h) == 0 || IsIconic(h) != 0 || cloaked(h) {
            return false;
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        if ex & WS_EX_TOPMOST != 0 {
            return false;
        }
        let promoted = ex & WS_EX_APPWINDOW != 0;
        if ex & WS_EX_TOOLWINDOW != 0 && !promoted {
            return false;
        }
        if GetWindow(h, GW_OWNER) != 0 && !promoted {
            return false;
        }
        GetWindowTextLengthW(h) > 0
    }

    /// Is one of the shell's desktop windows stacked above this top-level
    /// window?
    ///
    /// Walked from the *top* of the z-order down to `h`, with no small cap.
    /// The first version walked up from `h` and gave up after 512 windows --
    /// but a widget parked with HWND_BOTTOM sits under every invisible
    /// top-level window on the system (tooltips, IME and message-only
    /// helpers, one or more per running program), and after a day of uptime
    /// with a browser, a download manager and a video player open there are
    /// easily more than 512 of those between it and the desktop. The walk then
    /// ran out before reaching the desktop, answered "no", and Show Desktop
    /// left the widgets buried (watchdog.log: desktop_showing=true,
    /// desktop_above=false, lifts never increasing). Going top-down, the
    /// desktop windows are near the start of the walk whenever they matter.
    unsafe fn desktop_above(h: isize) -> bool {
        let mut w = GetTopWindow(0);
        let mut desktop_seen = false;
        for _ in 0..65_536 {
            if w == 0 || w == h {
                return desktop_seen && w == h;
            }
            if !desktop_seen && IsWindowVisible(w) != 0 && is_desktop_class(w) {
                desktop_seen = true;
            }
            w = GetWindow(w, GW_HWNDNEXT);
        }
        false
    }

    /// State of a widget that is an ordinary top-level window sitting at the
    /// bottom of the z-order (the earth wallpaper is off, or `z_mode` is
    /// "bottom").
    pub fn loose_state(win: &WebviewWindow) -> Option<super::LooseState> {
        let h = handle(win)?;
        unsafe {
            let mut r = RawRect::default();
            GetWindowRect(h, &mut r);
            Some(super::LooseState {
                rect: (r.left, r.top, r.right, r.bottom),
                visible: IsWindowVisible(h) != 0,
                own_visible: GetWindowLongPtrW(h, GWL_STYLE) & WS_VISIBLE != 0,
                iconic: IsIconic(h) != 0,
                cloaked: cloaked(h),
                topmost: GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST != 0,
                desktop_above: desktop_above(h),
            })
        }
    }

    /// Walk the top-level windows from the front. If the shell's desktop turns
    /// up before any open application does, nothing is covering the desktop:
    /// either no app is open or Show Desktop is in effect.
    unsafe fn showing_raw(mates: &[isize]) -> bool {
        let mut w = GetTopWindow(0);
        for _ in 0..65_536 {
            if w == 0 {
                return false;
            }
            if IsWindowVisible(w) != 0 && is_desktop_class(w) {
                return true;
            }
            if !mates.contains(&w) && is_app_window(w) {
                return false;
            }
            w = GetWindow(w, GW_HWNDNEXT);
        }
        false
    }

    pub fn desktop_showing(ours: &[&WebviewWindow]) -> bool {
        let mates: Vec<isize> = ours.iter().filter_map(|w| handle(w)).collect();
        unsafe { showing_raw(&mates) }
    }

    // ---- Show Desktop guard --------------------------------------------
    //
    // Show Desktop (Win+D) raises the shell's desktop windows over every
    // ordinary window, so a widget parked at the bottom of the z-order ends
    // up underneath. The only way to stay visible is to be topmost while that
    // lasts -- and to be back at the bottom the moment applications return.
    //
    // Polling for this leaves a visible gap of up to a poll interval at each
    // end. The shell announces both edges as they happen (focus lands on the
    // desktop, a window minimises or restores, a desktop window is shown), so
    // the guard listens for those and reacts inside the same frame.

    /// The widgets the guard looks after; empty when it has nothing to do.
    static GUARD: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    static GUARD_ON: AtomicBool = AtomicBool::new(false);
    static LIFTS: AtomicU32 = AtomicU32::new(0);
    static LOWERS: AtomicU32 = AtomicU32::new(0);

    /// Tell the guard which windows to look after, or (`active` false) to
    /// stand down -- while the earth wallpaper hosts the widgets, in edit
    /// mode, or when the user asked for a fixed z-order.
    pub fn guard_set(wins: &[&WebviewWindow], active: bool) {
        let handles: Vec<isize> = if active {
            wins.iter().filter_map(|w| handle(w)).collect()
        } else {
            Vec::new()
        };
        let on = active && !handles.is_empty();
        if let Ok(mut g) = GUARD.lock() {
            *g = handles;
        }
        GUARD_ON.store(on, Ordering::Relaxed);
    }

    /// How many times the guard has lifted and lowered the widgets, for the log.
    pub fn guard_counts() -> (u32, u32) {
        (LIFTS.load(Ordering::Relaxed), LOWERS.load(Ordering::Relaxed))
    }

    /// Called with `true` just before the widgets are lifted back into view
    /// and `false` just after, so the page can veil itself and fade up rather
    /// than pop. Registered once by the app; without it the lift is immediate.
    static LIFT_NOTIFY: Mutex<Option<fn(bool)>> = Mutex::new(None);
    /// A delayed lift is in flight; more events must not start another.
    static LIFT_PENDING: AtomicBool = AtomicBool::new(false);

    /// How long the widgets stay veiled and out of sight before they are
    /// lifted, so the page has certainly painted its blank frame first.
    const VEIL_SETTLE_MS: u64 = 110;

    pub fn guard_on_lift(f: fn(bool)) {
        if let Ok(mut slot) = LIFT_NOTIFY.lock() {
            *slot = Some(f);
        }
    }

    unsafe fn lift_now(mates: &[isize]) {
        for &h in mates {
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        LIFTS.fetch_add(1, Ordering::Relaxed);
    }

    /// The desktop has just covered the widgets. With a notifier registered
    /// the page is veiled first (it is under the desktop, so nobody sees
    /// that), the lift follows a moment later, and the page is then told to
    /// fade in.
    unsafe fn begin_lift(mates: &[isize]) {
        let notify = LIFT_NOTIFY.lock().ok().and_then(|g| *g);
        let Some(notify) = notify else {
            lift_now(mates);
            return;
        };
        if LIFT_PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        notify(true);
        let mates = mates.to_vec();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(VEIL_SETTLE_MS));
            unsafe {
                // Only lift if the desktop is still on top and nothing else
                // (the tick, another event) got there first.
                let already = mates
                    .iter()
                    .any(|&h| GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST != 0);
                if !already && mates.iter().any(|&h| desktop_above(h)) {
                    lift_now(&mates);
                }
            }
            notify(false);
            LIFT_PENDING.store(false, Ordering::Release);
        });
    }

    /// Bring the widgets' stacking in line with what the desktop is doing.
    /// Cheap and idempotent, so it can run from an event and from a timer.
    unsafe fn reconcile(mates: &[isize]) {
        if mates.is_empty() {
            return;
        }
        let lifted = mates
            .iter()
            .any(|&h| GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST != 0);
        if !lifted {
            if mates.iter().any(|&h| desktop_above(h)) {
                begin_lift(mates);
            }
        } else if !showing_raw(mates) {
            for &h in mates {
                SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                SetWindowPos(h, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            LOWERS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The timer's safety net for the event path.
    pub fn guard_reconcile_now() {
        if !GUARD_ON.load(Ordering::Relaxed) {
            return;
        }
        let mates = match GUARD.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        unsafe { reconcile(&mates) }
    }

    const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
    const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
    const EVENT_OBJECT_SHOW: u32 = 0x8002;
    const EVENT_OBJECT_REORDER: u32 = 0x8004;
    const WINEVENT_OUTOFCONTEXT: u32 = 0;
    const WINEVENT_SKIPOWNPROCESS: u32 = 2;
    const OBJID_WINDOW: i32 = 0;

    unsafe extern "system" fn on_shell_event(
        _hook: isize,
        event: u32,
        hwnd: isize,
        id_object: i32,
        id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if !GUARD_ON.load(Ordering::Relaxed) || id_object != OBJID_WINDOW || id_child != 0 {
            return;
        }
        // Focus and minimise events are rare enough to always act on. Show and
        // reorder events fire for every menu and tooltip on the system, so
        // only the desktop's own windows are worth waking up for.
        let worth_it = match event {
            EVENT_SYSTEM_FOREGROUND | EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => true,
            _ => hwnd != 0 && is_desktop_class(hwnd),
        };
        if !worth_it {
            return;
        }
        let mates = match GUARD.try_lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        reconcile(&mates);
    }

    /// Start listening. Must run on a thread that pumps messages -- the
    /// app's main thread does -- because the callbacks are delivered there.
    pub fn guard_install() -> bool {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let mut ok = true;
        unsafe {
            for (lo, hi) in [
                (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
                (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
                (EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER),
            ] {
                if SetWinEventHook(lo, hi, 0, Some(on_shell_event), 0, 0, flags) == 0 {
                    ok = false;
                }
            }
        }
        ok
    }

    /// Class of whatever has the focus, and whether it is a desktop host (has
    /// the icon view inside it), for the log.
    pub fn foreground() -> String {
        unsafe {
            let h = GetForegroundWindow();
            if h == 0 {
                return "none".into();
            }
            let defview = wide("SHELLDLL_DefView");
            let has_icons = FindWindowExW(h, 0, defview.as_ptr(), std::ptr::null()) != 0;
            format!("{}#{:x} icons={}", class_of(h), h, has_icons)
        }
    }

    /// The front of the top-level z-order, top first, one window per entry:
    /// class, visible, minimised, cloaked, topmost, tool window, has-title.
    /// Windows that belong to the widgets are starred. This is what says what
    /// Show Desktop actually did to the stack.
    pub fn zorder_dump(ours: &[&WebviewWindow], limit: usize) -> String {
        let mates: Vec<isize> = ours.iter().filter_map(|w| handle(w)).collect();
        let mut out: Vec<String> = Vec::new();
        unsafe {
            let mut w = GetTopWindow(0);
            let mut seen = 0usize;
            for _ in 0..2048 {
                if w == 0 || out.len() >= limit {
                    break;
                }
                seen += 1;
                let ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
                let vis = IsWindowVisible(w) != 0;
                let ours = mates.contains(&w);
                // Invisible strays are noise unless they are desktop hosts.
                if vis || ours || is_desktop_class(w) {
                    out.push(format!(
                        "{}{}#{:x}(v={} m={} c={} top={} tool={} t={})",
                        if ours { "*" } else { "" },
                        class_of(w),
                        w,
                        vis as u8,
                        (IsIconic(w) != 0) as u8,
                        cloaked(w) as u8,
                        (ex & WS_EX_TOPMOST != 0) as u8,
                        (ex & WS_EX_TOOLWINDOW != 0) as u8,
                        (GetWindowTextLengthW(w) > 0) as u8,
                    ));
                }
                w = GetWindow(w, GW_HWNDNEXT);
            }
            let _ = seen;
        }
        out.join(" > ")
    }

    /// The window and its ancestors, for the watchdog log: class, visibility,
    /// minimised, cloaked. Enough to see *what* the shell did to us.
    pub fn chain(win: &WebviewWindow) -> String {
        let Some(mut h) = handle(win) else { return "no-hwnd".into() };
        let mut parts: Vec<String> = Vec::new();
        unsafe {
            for _ in 0..8 {
                parts.push(format!(
                    "{}#{:x}(vis={} min={} cloak={})",
                    class_of(h),
                    h,
                    IsWindowVisible(h) != 0,
                    IsIconic(h) != 0,
                    cloaked(h),
                ));
                let up = parent_of(h);
                if up == 0 || up == GetDesktopWindow() {
                    break;
                }
                h = up;
            }
        }
        parts.join(" < ")
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::WebviewWindow;
    pub fn mark_as_widget(_win: &WebviewWindow) {}
    pub fn set_topmost(_win: &WebviewWindow, _on: bool) {}
    pub fn sink_to_bottom(_win: &WebviewWindow) {}
    pub fn virtual_screen() -> (i32, i32, i32, i32) {
        (0, 0, 1920, 1080)
    }
    pub fn primary_screen() -> (i32, i32, i32, i32) {
        (0, 0, 1920, 1080)
    }
    pub fn is_in_wallpaper_layer(_win: &WebviewWindow) -> bool {
        false
    }
    pub fn attach_wallpaper(_win: &WebviewWindow) -> bool {
        false
    }
    pub fn attach_to(_win: &WebviewWindow, _host: &WebviewWindow) -> bool {
        false
    }
    pub fn detach(_win: &WebviewWindow) {}
    pub fn is_detached(_win: &WebviewWindow) -> bool {
        true
    }
    pub fn widget_state(
        _win: &WebviewWindow,
        _host: &WebviewWindow,
        _friends: &[&WebviewWindow],
    ) -> Option<super::WidgetState> {
        None
    }
    pub fn show_noactivate(_win: &WebviewWindow) {}
    pub fn raise(_win: &WebviewWindow) {}
    pub fn loose_state(_win: &WebviewWindow) -> Option<super::LooseState> {
        None
    }
    pub fn desktop_showing(_ours: &[&WebviewWindow]) -> bool {
        false
    }
    pub fn guard_set(_wins: &[&WebviewWindow], _active: bool) {}
    pub fn guard_counts() -> (u32, u32) {
        (0, 0)
    }
    pub fn guard_reconcile_now() {}
    pub fn guard_on_lift(_f: fn(bool)) {}
    pub fn guard_install() -> bool {
        true
    }
    pub fn foreground() -> String {
        String::new()
    }
    pub fn zorder_dump(_ours: &[&WebviewWindow], _limit: usize) -> String {
        String::new()
    }
    pub fn chain(_win: &WebviewWindow) -> String {
        String::new()
    }
}

/// What the shell can do to a widget that lives inside the wallpaper window
/// without ever destroying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetState {
    /// Effective visibility: the window and every ancestor have WS_VISIBLE.
    pub visible: bool,
    /// The window's own WS_VISIBLE bit, so "we were hidden" and "an ancestor
    /// was hidden" can be told apart in the log.
    pub own_visible: bool,
    pub iconic: bool,
    /// Hidden by the DWM. Reads as visible everywhere else. Cannot be
    /// undone from outside, so it is reported but never "healed".
    pub cloaked: bool,
    /// Still a child of the wallpaper window.
    pub in_host: bool,
    /// Something that is not one of our widgets is stacked above it.
    pub covered: bool,
}

impl WidgetState {
    /// Nothing the watchdog knows how to repair is wrong.
    pub fn healthy(&self) -> bool {
        self.visible && !self.iconic && self.in_host && !self.covered
    }

    pub fn describe(&self) -> String {
        format!(
            "vis={} own={} min={} cloak={} in_host={} covered={}",
            self.visible, self.own_visible, self.iconic, self.cloaked, self.in_host, self.covered
        )
    }
}

/// Same idea for a widget that is a plain top-level window at the bottom of
/// the z-order rather than a child of the wallpaper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseState {
    /// Screen rectangle: left, top, right, bottom.
    pub rect: (i32, i32, i32, i32),
    pub visible: bool,
    pub own_visible: bool,
    pub iconic: bool,
    pub cloaked: bool,
    pub topmost: bool,
    /// One of the shell's desktop windows (Progman / WorkerW) is stacked
    /// above this widget, so the desktop is hiding it. Show Desktop does this:
    /// it raises the desktop over everything, and a window parked at the
    /// bottom of the z-order ends up underneath.
    pub desktop_above: bool,
}

impl LooseState {
    pub fn healthy(&self) -> bool {
        self.visible && !self.iconic && !self.desktop_above
    }

    pub fn describe(&self) -> String {
        format!(
            "vis={} own={} min={} cloak={} topmost={} desktop_above={} rect={:?}",
            self.visible,
            self.own_visible,
            self.iconic,
            self.cloaked,
            self.topmost,
            self.desktop_above,
            self.rect
        )
    }
}

pub use imp::{
    attach_to, attach_wallpaper, chain, desktop_showing, detach, foreground, guard_counts,
    guard_install, guard_on_lift, guard_reconcile_now, guard_set, is_detached, is_in_wallpaper_layer, loose_state,
    mark_as_widget, primary_screen, raise, set_topmost, show_noactivate, sink_to_bottom,
    virtual_screen, widget_state, zorder_dump,
};
