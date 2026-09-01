//! Raw low-level input capture (Windows).
//!
//! This is the "Rust for Windows" way of doing things: instead of relying on
//! winit/egui's abstractions, we call the Win32 API directly (via
//! `windows-sys`) from a dedicated background thread that runs its own
//! message loop:
//!
//! * **Mouse** — a global `WH_MOUSE_LL` hook forwards every mouse
//!   move / button / wheel event together with its global position, and raw
//!   input (`WM_INPUT`) delivers high-frequency relative deltas
//!   (`RAWINPUT`).
//! * **Pen (터치펜) / touch / trackpad** — raw input is registered for the
//!   HID digitizer usages (pen `0x0D/0x02`, touch screen `0x0D/0x04`, touch
//!   pad `0x0D/0x05`). Every `WM_INPUT` HID report is decoded best-effort
//!   (contact, x/y, pressure, tilt).
//!
//! All events are **debounced (~1 ms)**: the hook/raw-input handlers merge
//! them into a small accumulator (latest position/state, summed deltas) and a
//! dedicated thread flushes the coalesced events to the app at ~1 ms, so
//! high-rate HID devices (pen, touch, high-polling mice) never flood the
//! channel or the per-frame processing.
//!
//! Because the input thread has its own message queue, input keeps flowing
//! even while the overlay window is click-through (the pointer events go to
//! the app below the cursor, but we still observe everything).

use std::sync::mpsc::Receiver;

/// A single contact decoded from a HID digitizer report.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Contact {
    /// Tip switch / contact bit.
    pub down: bool,
    /// Pen in range / confidence bit.
    pub in_range: bool,
    /// Raw X (usually 0..=65535 device units).
    pub x: f64,
    /// Raw Y (usually 0..=65535 device units).
    pub y: f64,
    /// Normalized pressure 0.0..=1.0 (best-effort).
    pub pressure: f32,
    /// Tilt along X, degrees (best-effort, pen only).
    pub tilt_x: f32,
    /// Tilt along Y, degrees (best-effort, pen only).
    pub tilt_y: f32,
}

/// Events produced by the raw-input thread.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum InputEvent {
    /// Global mouse position (from the low-level hook), physical pixels.
    MouseMove { x: f64, y: f64 },
    /// Left/right button press/release (from the low-level hook).
    MouseButton { left: bool, right: bool, down: bool },
    /// Mouse wheel delta (multiple of 120).
    MouseWheel { delta: i32 },
    /// Raw input (`RAWINPUT`): relative motion + button transition flags.
    RawMouse {
        dx: i32,
        dy: i32,
        flags: u16,
        buttons: u32,
    },
    /// HID pen digitizer (0x0D/0x02).
    Pen { contact: Contact },
    /// HID touch screen (0x0D/0x04).
    Touch { contact: Contact },
    /// HID touch pad / precision trackpad (0x0D/0x05).
    Touchpad {
        contact: Contact,
        dx: i32,
        dy: i32,
    },
    /// Any other HID report with its raw bytes (device-specific).
    HidRaw {
        usage_page: u16,
        usage: u16,
        data: Vec<u8>,
    },
}

/// Latest decoded raw-input state, shown in the settings panel.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct InputSnapshot {
    pub mouse: (f64, f64),
    pub raw_delta: (i64, i64),
    pub buttons: u32,
    pub wheel: i32,
    pub pen: Option<Contact>,
    pub touch: Option<Contact>,
    pub touchpad: Option<Contact>,
    pub touchpad_delta: (i64, i64),
    pub hid_reports: u64,
    pub last_device: &'static str,
}

impl InputSnapshot {
    pub fn apply(&mut self, ev: &InputEvent) {
        match ev {
            InputEvent::MouseMove { x, y } => {
                self.mouse = (*x, *y);
                self.last_device = "mouse";
            }
            InputEvent::MouseButton { .. } => {
                self.last_device = "mouse";
            }
            InputEvent::MouseWheel { delta } => {
                self.wheel += delta;
                self.last_device = "mouse";
            }
            InputEvent::RawMouse {
                dx,
                dy,
                flags,
                buttons,
            } => {
                self.raw_delta.0 += *dx as i64;
                self.raw_delta.1 += *dy as i64;
                if *flags != 0 {
                    self.buttons = *buttons;
                }
                self.last_device = "mouse";
            }
            InputEvent::Pen { contact } => {
                self.pen = Some(*contact);
                self.last_device = "pen";
            }
            InputEvent::Touch { contact } => {
                self.touch = Some(*contact);
                self.last_device = "touch";
            }
            InputEvent::Touchpad {
                contact,
                dx,
                dy,
            } => {
                self.touchpad = Some(*contact);
                self.touchpad_delta.0 += *dx as i64;
                self.touchpad_delta.1 += *dy as i64;
                self.last_device = "touchpad";
            }
            InputEvent::HidRaw { .. } => {
                self.hid_reports += 1;
                self.last_device = "hid";
            }
        }
    }
}

/// Start the raw-input background threads. Returns a receiver the app drains
/// every frame, or `None` on platforms where raw input is not supported.
pub fn start() -> Option<Receiver<InputEvent>> {
    #[cfg(target_os = "windows")]
    {
        win::start()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Stop the raw-input threads / unhook (called on shutdown).
pub fn stop() {
    #[cfg(target_os = "windows")]
    win::stop();
    #[cfg(not(target_os = "windows"))]
    {}
}

/// Last global mouse position captured by the low-level hook, if any
/// (lower latency than polling `GetCursorPos` every frame).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn last_global_mouse_pos() -> Option<(f64, f64)> {
    #[cfg(target_os = "windows")]
    {
        win::last_global_mouse_pos()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Windows implementation (raw Win32 API via windows-sys)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::mem::size_of;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use windows_sys::Win32::UI::Input as kbm;
    use windows_sys::Win32::UI::WindowsAndMessaging as wam;

    static EVENT_TX: OnceLock<Sender<InputEvent>> = OnceLock::new();
    static HOOK: AtomicUsize = AtomicUsize::new(0);
    static LAST_POS: AtomicU64 = AtomicU64::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);
    /// Raw input events are merged here and flushed to the app at ~1 ms, so
    /// high-rate HID devices (pen / touch / high-polling mice) don't flood
    /// the channel or the per-frame processing.
    static COALESCER: Mutex<Coalescer> = Mutex::new(Coalescer::new());

    // HID usage page / usage values for the devices we subscribe to.
    const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
    const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
    const HID_USAGE_PAGE_DIGITIZER: u16 = 0x0D;
    const HID_USAGE_DIGITIZER_PEN: u16 = 0x02;
    const HID_USAGE_DIGITIZER_TOUCH_SCREEN: u16 = 0x04;
    const HID_USAGE_DIGITIZER_TOUCH_PAD: u16 = 0x05;

    pub fn start() -> Option<Receiver<InputEvent>> {
        let (tx, rx) = mpsc::channel();
        if EVENT_TX.set(tx).is_err() {
            // Already running (e.g. re-entrant start): still return a receiver.
            log::warn!("raw input thread already running");
            return Some(rx);
        }
        std::thread::Builder::new()
            .name("raw-input".into())
            .spawn(raw_input_thread)
            .ok()?;
        // ~1 ms debounced flusher: emits the coalesced events to the app.
        std::thread::Builder::new()
            .name("raw-flush".into())
            .spawn(flush_loop)
            .ok()?;
        Some(rx)
    }

    pub fn stop() {
        STOP.store(true, Ordering::SeqCst);
        let h = HOOK.swap(0, Ordering::SeqCst);
        if h != 0 {
            unsafe {
                wam::UnhookWindowsHookEx(h as *mut core::ffi::c_void);
            }
        }
    }

    pub fn last_global_mouse_pos() -> Option<(f64, f64)> {
        let v = LAST_POS.load(Ordering::Relaxed);
        if v == 0 {
            None
        } else {
            Some((
                (v & 0xFFFF_FFFF) as u32 as f64,
                ((v >> 32) & 0xFFFF_FFFF) as u32 as f64,
            ))
        }
    }

    fn set_last_pos(x: f64, y: f64) {
        let packed = ((y as u32 as u64) << 32) | (x as u32 as u64);
        LAST_POS.store(packed, Ordering::Relaxed);
    }

    fn send(ev: InputEvent) {
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(ev);
        }
    }

    /// Debounced accumulator that merges raw input events arriving faster
    /// than the flush rate: keeps the latest state, sums deltas, so a ~1 ms
    /// flush only ever emits a handful of coalesced events.
    struct Coalescer {
        mouse_pos: Option<(f64, f64)>,
        raw_delta: (i32, i32),
        raw_flags: u16,
        raw_buttons: u32,
        wheel: i32,
        button: Option<(bool, bool, bool)>,
        pen: Option<Contact>,
        touch: Option<Contact>,
        touchpad: Option<(Contact, i32, i32)>,
        hid_reports: u64,
    }

    impl Coalescer {
        const fn new() -> Self {
            Self {
                mouse_pos: None,
                raw_delta: (0, 0),
                raw_flags: 0,
                raw_buttons: 0,
                wheel: 0,
                button: None,
                pen: None,
                touch: None,
                touchpad: None,
                hid_reports: 0,
            }
        }

        fn mouse_move(&mut self, x: f64, y: f64) {
            self.mouse_pos = Some((x, y));
        }
        fn mouse_button(&mut self, left: bool, right: bool, down: bool) {
            self.button = Some((left, right, down));
        }
        fn wheel(&mut self, delta: i32) {
            self.wheel += delta;
        }
        fn raw_mouse(&mut self, dx: i32, dy: i32, flags: u16, buttons: u32) {
            self.raw_delta.0 += dx;
            self.raw_delta.1 += dy;
            self.raw_flags |= flags;
            self.raw_buttons = buttons;
        }
        fn pen(&mut self, c: Contact) {
            self.pen = Some(c);
        }
        fn touch(&mut self, c: Contact) {
            self.touch = Some(c);
        }
        fn touchpad(&mut self, c: Contact, dx: i32, dy: i32) {
            self.touchpad = Some((c, dx, dy));
        }
        fn hid(&mut self) {
            self.hid_reports += 1;
        }

        /// Build the coalesced event list and reset the accumulator.
        fn drain(&mut self) -> Vec<InputEvent> {
            let mut out = Vec::with_capacity(8);
            if let Some((x, y)) = self.mouse_pos.take() {
                out.push(InputEvent::MouseMove { x, y });
            }
            if let Some((left, right, down)) = self.button.take() {
                out.push(InputEvent::MouseButton { left, right, down });
            }
            let (dx, dy) = self.raw_delta;
            if dx != 0 || dy != 0 || self.raw_flags != 0 {
                out.push(InputEvent::RawMouse {
                    dx,
                    dy,
                    flags: self.raw_flags,
                    buttons: self.raw_buttons,
                });
            }
            self.raw_delta = (0, 0);
            self.raw_flags = 0;
            if self.wheel != 0 {
                out.push(InputEvent::MouseWheel { delta: self.wheel });
                self.wheel = 0;
            }
            if let Some(c) = self.pen.take() {
                out.push(InputEvent::Pen { contact: c });
            }
            if let Some(c) = self.touch.take() {
                out.push(InputEvent::Touch { contact: c });
            }
            if let Some((c, dx, dy)) = self.touchpad.take() {
                out.push(InputEvent::Touchpad { contact: c, dx, dy });
            }
            if self.hid_reports > 0 {
                // Raw HID reports are counted (not forwarded byte-for-byte).
                out.push(InputEvent::HidRaw {
                    usage_page: 0,
                    usage: 0,
                    data: Vec::new(),
                });
                self.hid_reports = 0;
            }
            out
        }
    }

    fn coalesce() -> std::sync::MutexGuard<'static, Coalescer> {
        COALESCER.lock().unwrap()
    }

    /// Debounced flusher: every ~1 ms, drain the coalesced events into the
    /// channel. On Windows the actual sleep granularity is ~1–16 ms, which is
    /// fine because the app only renders at ~60 Hz — the point is to coalesce
    /// bursts, not to add perceptible latency.
    fn flush_loop() {
        let period = std::time::Duration::from_millis(1);
        while !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(period);
            let events = {
                let mut c = COALESCER.lock().unwrap();
                c.drain()
            };
            for ev in events {
                send(ev);
            }
        }
    }

    /// Global low-level mouse hook: merges every mouse event (position,
    /// buttons, wheel) into the debounced coalescer and tracks the latest
    /// global position for the low-latency position fallback.
    unsafe extern "system" fn mouse_ll_hook(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            let m = &*(lparam as *const wam::MSLLHOOKSTRUCT);
            let msg = wparam as u32;
            match msg {
                wam::WM_MOUSEMOVE => {
                    set_last_pos(m.pt.x as f64, m.pt.y as f64);
                    coalesce().mouse_move(m.pt.x as f64, m.pt.y as f64);
                }
                wam::WM_LBUTTONDOWN => coalesce().mouse_button(true, false, true),
                wam::WM_LBUTTONUP => coalesce().mouse_button(true, false, false),
                wam::WM_RBUTTONDOWN => coalesce().mouse_button(false, true, true),
                wam::WM_RBUTTONUP => coalesce().mouse_button(false, true, false),
                wam::WM_MOUSEWHEEL => {
                    let delta = (m.mouseData >> 16) as u16 as i16 as i32;
                    coalesce().wheel(delta);
                }
                _ => {}
            }
        }
        wam::CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Enumerate raw input devices and build a map `hDevice -> (usagePage,
    /// usage)` so `WM_INPUT` HID reports can be classified as pen / touch /
    /// touchpad / other.
    fn enumerate_devices() -> std::collections::HashMap<usize, (u16, u16)> {
        let mut map = std::collections::HashMap::new();
        unsafe {
            let mut count: u32 = 0;
            let header = size_of::<kbm::RAWINPUTDEVICELIST>() as u32;
            kbm::GetRawInputDeviceList(std::ptr::null_mut(), &mut count, header);
            if count == 0 {
                return map;
            }
            let mut list = vec![std::mem::zeroed::<kbm::RAWINPUTDEVICELIST>(); count as usize];
            let n = kbm::GetRawInputDeviceList(list.as_mut_ptr(), &mut count, header);
            for item in list.iter().take(n as usize) {
                let mut size: u32 = 0;
                kbm::GetRawInputDeviceInfoW(
                    item.hDevice,
                    kbm::RIDI_DEVICEINFO,
                    std::ptr::null_mut(),
                    &mut size,
                );
                if size == 0 {
                    continue;
                }
                let mut info: kbm::RID_DEVICE_INFO = std::mem::zeroed();
                let cb = kbm::GetRawInputDeviceInfoW(
                    item.hDevice,
                    kbm::RIDI_DEVICEINFO,
                    &mut info as *mut kbm::RID_DEVICE_INFO as *mut core::ffi::c_void,
                    &mut size,
                );
                if cb == 0 {
                    continue;
                }
                if info.dwType == kbm::RIM_TYPEHID {
                    let (page, usage) = (info.Anonymous.hid.usUsagePage, info.Anonymous.hid.usUsage);
                    map.insert(item.hDevice as usize, (page, usage));
                }
            }
        }
        map
    }

    /// The raw-input thread: installs the mouse hook, registers HID
    /// digitizer devices and runs its own message loop.
    fn raw_input_thread() {
        unsafe {
            let devices = [
                kbm::RAWINPUTDEVICE {
                    usUsagePage: HID_USAGE_PAGE_GENERIC,
                    usUsage: HID_USAGE_GENERIC_MOUSE,
                    dwFlags: kbm::RIDEV_INPUTSINK,
                    hwndTarget: std::ptr::null_mut(),
                },
                kbm::RAWINPUTDEVICE {
                    usUsagePage: HID_USAGE_PAGE_DIGITIZER,
                    usUsage: HID_USAGE_DIGITIZER_PEN,
                    dwFlags: kbm::RIDEV_INPUTSINK,
                    hwndTarget: std::ptr::null_mut(),
                },
                kbm::RAWINPUTDEVICE {
                    usUsagePage: HID_USAGE_PAGE_DIGITIZER,
                    usUsage: HID_USAGE_DIGITIZER_TOUCH_SCREEN,
                    dwFlags: kbm::RIDEV_INPUTSINK,
                    hwndTarget: std::ptr::null_mut(),
                },
                kbm::RAWINPUTDEVICE {
                    usUsagePage: HID_USAGE_PAGE_DIGITIZER,
                    usUsage: HID_USAGE_DIGITIZER_TOUCH_PAD,
                    dwFlags: kbm::RIDEV_INPUTSINK,
                    hwndTarget: std::ptr::null_mut(),
                },
            ];
            let cb = size_of::<kbm::RAWINPUTDEVICE>() as u32;
            if kbm::RegisterRawInputDevices(devices.as_ptr(), devices.len() as u32, cb) == 0 {
                log::warn!(
                    "RegisterRawInputDevices failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let usage_map = enumerate_devices();

            // WH_MOUSE_LL may live in the current process (no DLL needed).
            let hook = wam::SetWindowsHookExW(
                wam::WH_MOUSE_LL,
                Some(mouse_ll_hook),
                std::ptr::null_mut(),
                0,
            );
            if hook.is_null() {
                log::warn!(
                    "SetWindowsHookExW(WH_MOUSE_LL) failed: {}",
                    std::io::Error::last_os_error()
                );
            } else {
                HOOK.store(hook as usize, Ordering::SeqCst);
            }

            let mut msg = std::mem::zeroed::<wam::MSG>();
            while wam::GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message == wam::WM_INPUT {
                    handle_raw_input(msg.lParam as isize, &usage_map);
                } else {
                    wam::TranslateMessage(&msg);
                    wam::DispatchMessageW(&msg);
                }
            }

            if !hook.is_null() {
                wam::UnhookWindowsHookEx(hook);
            }
            HOOK.store(0, Ordering::SeqCst);
        }
    }

    /// Parse a `WM_INPUT` `RAWINPUT` structure and merge it into the debounced
    /// coalescer.
    unsafe fn handle_raw_input(
        lparam: isize,
        usage_map: &std::collections::HashMap<usize, (u16, u16)>,
    ) {
        let mut size: u32 = 0;
        let header = size_of::<kbm::RAWINPUTHEADER>() as u32;
        kbm::GetRawInputData(
            lparam as *mut core::ffi::c_void,
            kbm::RID_INPUT,
            std::ptr::null_mut(),
            &mut size,
            header,
        );
        if size == 0 {
            return;
        }
        let mut buf = vec![0u8; size as usize];
        let read = kbm::GetRawInputData(
            lparam as *mut core::ffi::c_void,
            kbm::RID_INPUT,
            buf.as_mut_ptr().cast(),
            &mut size,
            header,
        );
        if read == 0 {
            return;
        }
        let raw = &*(buf.as_ptr() as *const kbm::RAWINPUT);
        match raw.header.dwType {
            kbm::RIM_TYPEMOUSE => {
                let m = raw.data.mouse;
                let btns = m.Anonymous.Anonymous;
                let (dx, dy) = if m.usFlags & kbm::MOUSE_MOVE_ABSOLUTE != 0 {
                    (0, 0)
                } else {
                    (m.lLastX, m.lLastY)
                };
                let mut c = COALESCER.lock().unwrap();
                c.raw_mouse(dx, dy, btns.usButtonFlags, m.ulRawButtons);
                if (btns.usButtonFlags as u32) & wam::RI_MOUSE_WHEEL != 0 {
                    c.wheel(btns.usButtonData as i16 as i32);
                }
            }
            kbm::RIM_TYPEHID => {
                let hid = raw.data.hid;
                let n = (hid.dwSizeHid as usize).saturating_mul(hid.dwCount as usize);
                let bytes = std::slice::from_raw_parts(hid.bRawData.as_ptr(), n.min(256));
                let Some((page, usage)) = usage_map.get(&(raw.header.hDevice as usize)).copied()
                else {
                    coalesce().hid();
                    return;
                };
                if page == HID_USAGE_PAGE_DIGITIZER {
                    match usage {
                        HID_USAGE_DIGITIZER_PEN => {
                            if let Some(c) = decode_contact(bytes) {
                                coalesce().pen(c);
                            }
                        }
                        HID_USAGE_DIGITIZER_TOUCH_SCREEN => {
                            if let Some(c) = decode_contact(bytes) {
                                coalesce().touch(c);
                            }
                        }
                        HID_USAGE_DIGITIZER_TOUCH_PAD => {
                            if let Some(c) = decode_contact(bytes) {
                                // Touch pads report absolute contact positions;
                                // relative motion arrives as RawMouse deltas.
                                coalesce().touchpad(c, 0, 0);
                            }
                        }
                        _ => coalesce().hid(),
                    }
                } else {
                    coalesce().hid();
                }
            }
            _ => {}
        }
    }

    /// Best-effort decode of a common Windows HID digitizer report:
    ///
    /// ```text
    ///   [0]     report ID (1..=255, or 0 if absent)
    ///   [1]     flags: bit0 contact/tip, bit1 in-range/confidence
    ///   [2..4]  X (16-bit LE)
    ///   [4..6]  Y (16-bit LE)
    ///   [6..8]  pressure (16-bit LE)
    ///   [8]     tilt X (8-bit signed), [9] tilt Y (8-bit signed)
    /// ```
    ///
    /// The layout differs between devices; the debounced pipeline counts
    /// unrecognized reports (`HidRaw`) instead of forwarding every raw byte.
    fn decode_contact(data: &[u8]) -> Option<Contact> {
        if data.len() < 6 {
            return None;
        }
        let base = if data[0] == 0 { 0 } else { 1 }; // skip report ID
        if data.len() < base + 6 {
            return None;
        }
        let flags = data[base + 1];
        let x = u16::from_le_bytes([data[base + 2], data[base + 3]]) as f64;
        let y = u16::from_le_bytes([data[base + 4], data[base + 5]]) as f64;
        let pressure = if data.len() >= base + 8 {
            u16::from_le_bytes([data[base + 6], data[base + 7]]) as f32 / 4096.0
        } else {
            0.0
        };
        let tilt_x = if data.len() >= base + 9 {
            data[base + 8] as i8 as f32
        } else {
            0.0
        };
        let tilt_y = if data.len() >= base + 10 {
            data[base + 9] as i8 as f32
        } else {
            0.0
        };
        Some(Contact {
            down: flags & 0x01 != 0,
            in_range: flags & 0x02 != 0,
            x,
            y,
            pressure: pressure.clamp(0.0, 1.0),
            tilt_x,
            tilt_y,
        })
    }
}
