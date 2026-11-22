//! Thread-safe global hotkey manager backed by a dedicated lightweight
//! `HWND_MESSAGE` Win32 message loop thread.

use crate::PlatformError;
use std::collections::{HashSet, VecDeque};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use windows_sys::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_SNAPSHOT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    HHOOK, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
};

static RECORDING_PAUSED: AtomicBool = AtomicBool::new(false);
static LL_HOOK_SPECS: Mutex<Vec<(isize, HotkeySpec, mpsc::Sender<HotkeyEvent>)>> =
    Mutex::new(Vec::new());

/// Action detected while polling Win32 keyboard state for interactive Hotkey Recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedKeyAction {
    Cancel,
    ClearToNone,
    Shortcut(String),
}

const CANDIDATE_RECORD_VKS: &[(i32, &str)] = &[
    (0x2C, "Print Screen"),
    (0x70, "F1"),
    (0x71, "F2"),
    (0x72, "F3"),
    (0x73, "F4"),
    (0x74, "F5"),
    (0x75, "F6"),
    (0x76, "F7"),
    (0x77, "F8"),
    (0x78, "F9"),
    (0x79, "F10"),
    (0x7A, "F11"),
    (0x7B, "F12"),
    (0x20, "Space"),
    (0x09, "Tab"),
    (0x2D, "Insert"),
    (0x24, "Home"),
    (0x23, "End"),
    (0x21, "PageUp"),
    (0x22, "PageDown"),
];

/// Pause global hotkey firing and flush stale `GetAsyncKeyState` transition bits before recording.
pub fn begin_hotkey_recording() {
    RECORDING_PAUSED.store(true, Ordering::SeqCst);
    unsafe {
        let _ = GetAsyncKeyState(0x1B); // Esc
        let _ = GetAsyncKeyState(0x08); // Backspace
        let _ = GetAsyncKeyState(0x2E); // Delete
        for &(vk, _) in CANDIDATE_RECORD_VKS {
            let _ = GetAsyncKeyState(vk);
        }
        for vk in 0x30..=0x5A {
            let _ = GetAsyncKeyState(vk);
        }
    }
}

/// Resume global hotkey firing after interactive recording ends.
pub fn end_hotkey_recording() {
    RECORDING_PAUSED.store(false, Ordering::SeqCst);
}

/// Poll Win32 hardware key state (`GetAsyncKeyState`) to capture any pressed shortcut
/// (including `Print Screen`, `Alt+...`, `F1..F12`, `Ctrl+Shift+...`) regardless of UI focus.
pub fn poll_hotkey_recording_input() -> Option<RecordedKeyAction> {
    unsafe {
        if (GetAsyncKeyState(0x1B) as u16) & 0x8001 != 0 {
            return Some(RecordedKeyAction::Cancel);
        }
        let ctrl = ((GetAsyncKeyState(0x11) as u16) & 0x8000) != 0;
        let alt = ((GetAsyncKeyState(0x12) as u16) & 0x8000) != 0;
        let shift = ((GetAsyncKeyState(0x10) as u16) & 0x8000) != 0;

        if !ctrl
            && !alt
            && !shift
            && (((GetAsyncKeyState(0x08) as u16) & 0x8001 != 0)
                || ((GetAsyncKeyState(0x2E) as u16) & 0x8001 != 0))
        {
            return Some(RecordedKeyAction::ClearToNone);
        }

        let mut detected_label: Option<String> = None;
        for &(vk, name) in CANDIDATE_RECORD_VKS {
            let st = GetAsyncKeyState(vk) as u16;
            // `VK_SNAPSHOT` (0x2C) often pulses only `0x0001` because Windows releases it immediately!
            let hit = if vk == 0x2C {
                st & 0x8001 != 0
            } else {
                st & 0x8000 != 0
            };
            if hit {
                detected_label = Some(name.to_string());
                break;
            }
        }

        if detected_label.is_none() {
            for vk in b'0'..=b'9' {
                if ((GetAsyncKeyState(vk as i32) as u16) & 0x8000) != 0 {
                    detected_label = Some((vk as char).to_string());
                    break;
                }
            }
        }
        if detected_label.is_none() {
            for vk in b'A'..=b'Z' {
                if ((GetAsyncKeyState(vk as i32) as u16) & 0x8000) != 0 {
                    detected_label = Some((vk as char).to_string());
                    break;
                }
            }
        }

        let main_key = detected_label?;
        let mut parts: Vec<String> = Vec::new();
        if ctrl {
            parts.push("Ctrl".to_string());
        }
        if alt {
            parts.push("Alt".to_string());
        }
        if shift {
            parts.push("Shift".to_string());
        }
        parts.push(main_key);
        Some(RecordedKeyAction::Shortcut(parts.join("+")))
    }
}

const WM_APP_COMMAND: u32 = WM_APP + 1;
const WM_APP_SHUTDOWN: u32 = WM_APP + 2;

/// Modifier flags for a global Win32 hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotkeyModifiers(pub u32);

impl HotkeyModifiers {
    pub const NONE: Self = Self(0);
    pub const ALT: Self = Self(MOD_ALT);
    pub const CONTROL: Self = Self(MOD_CONTROL);
    pub const SHIFT: Self = Self(MOD_SHIFT);
    pub const WIN: Self = Self(MOD_WIN);
    pub const NOREPEAT: Self = Self(MOD_NOREPEAT);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Specification of a global hotkey to register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySpec {
    pub id: i32,
    pub modifiers: HotkeyModifiers,
    pub vk: u32,
}

impl HotkeySpec {
    pub const fn new(id: i32, modifiers: HotkeyModifiers, vk: u32) -> Self {
        Self { id, modifiers, vk }
    }

    /// Convenience constructor for the global `PrintScreen` (`VK_SNAPSHOT`) key.
    pub const fn print_screen(id: i32) -> Self {
        Self {
            id,
            modifiers: HotkeyModifiers::NOREPEAT,
            vk: VK_SNAPSHOT as u32,
        }
    }

    /// Parse a human-readable Snagit hotkey string (e.g. `"Print Screen"`, `"Ctrl+Shift+S"`,
    /// `"Shift+F9"`, `"Alt+Print Screen"`, `"F12"`) into a [`HotkeySpec`].
    /// Returns `None` for `"None"` or unrecognized keys.
    pub fn from_display_str(id: i32, text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            return None;
        }
        let mut mods = HotkeyModifiers::NOREPEAT;
        let mut vk_opt: Option<u32> = None;

        for raw_part in trimmed.split('+') {
            let part = raw_part.trim();
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => mods = mods.union(HotkeyModifiers::CONTROL),
                "shift" => mods = mods.union(HotkeyModifiers::SHIFT),
                "alt" => mods = mods.union(HotkeyModifiers::ALT),
                "win" | "super" => mods = mods.union(HotkeyModifiers::WIN),
                "print screen" | "printscreen" | "prtscr" | "snapshot" => {
                    vk_opt = Some(VK_SNAPSHOT as u32);
                }
                "space" => vk_opt = Some(0x20),
                "tab" => vk_opt = Some(0x09),
                "insert" => vk_opt = Some(0x2D),
                "home" => vk_opt = Some(0x24),
                "end" => vk_opt = Some(0x23),
                "pageup" => vk_opt = Some(0x21),
                "pagedown" => vk_opt = Some(0x22),
                "enter" | "return" => vk_opt = Some(0x0D),
                "esc" | "escape" => vk_opt = Some(0x1B),
                s if s.starts_with('f') && s.len() >= 2 => {
                    if let Ok(fn_num) = s[1..].parse::<u32>() {
                        if (1..=24).contains(&fn_num) {
                            vk_opt = Some(0x70 + (fn_num - 1));
                        }
                    }
                }
                s if s.len() == 1 => {
                    let b = s.as_bytes()[0];
                    if b.is_ascii_alphabetic() {
                        vk_opt = Some(b.to_ascii_uppercase() as u32);
                    } else if b.is_ascii_digit() {
                        vk_opt = Some(b as u32);
                    }
                }
                _ => {}
            }
        }

        vk_opt.map(|vk| Self::new(id, mods, vk))
    }
}

/// Outcome of attempting to register a global hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAvailability {
    /// Successfully registered with Win32.
    Registered,
    /// `PrintScreen` is already claimed by Windows Snipping Tool or another OS handler.
    ConflictOsSnippingTool,
    /// Another application has already registered this hotkey combination.
    AlreadyRegistered,
    /// Registration failed with another Win32 error code.
    Failed(u32),
}

/// Event emitted when a registered global hotkey is pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyEvent {
    pub id: i32,
}

enum Command {
    Register {
        spec: HotkeySpec,
        reply: mpsc::SyncSender<HotkeyAvailability>,
    },
    Unregister {
        id: i32,
        reply: mpsc::SyncSender<bool>,
    },
    EnableLlFallback {
        enabled: bool,
        reply: mpsc::SyncSender<bool>,
    },
}

/// Thread-safe manager for Win32 global hotkeys.
///
/// Runs a dedicated `HWND_MESSAGE` loop thread and cleanly unregisters all hotkeys
/// and exits via `WM_QUIT` on `Drop`.
pub struct HotkeyManager {
    hwnd: isize,
    commands: Arc<Mutex<VecDeque<Command>>>,
    events_rx: Mutex<mpsc::Receiver<HotkeyEvent>>,
    thread: Option<JoinHandle<()>>,
}

// SAFETY: `hwnd` is only used with thread-safe `PostMessageW`; `commands` and `events_rx`
// are protected by `Mutex`.
unsafe impl Send for HotkeyManager {}
unsafe impl Sync for HotkeyManager {}

impl HotkeyManager {
    /// Spawn the background `HWND_MESSAGE` thread and return a ready `HotkeyManager`.
    pub fn new() -> Result<Self, PlatformError> {
        let commands = Arc::new(Mutex::new(VecDeque::<Command>::new()));
        let commands_bg = Arc::clone(&commands);
        let (events_tx, events_rx) = mpsc::channel::<HotkeyEvent>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<isize, PlatformError>>(1);

        let handle = thread::Builder::new()
            .name("snagsoup-hotkey-loop".to_string())
            .spawn(move || {
                hotkey_thread_main(commands_bg, events_tx, ready_tx);
            })
            .map_err(|_| PlatformError::HotkeyThreadStopped)?;

        let hwnd = ready_rx
            .recv()
            .map_err(|_| PlatformError::HotkeyThreadStopped)??;

        Ok(Self {
            hwnd,
            commands,
            events_rx: Mutex::new(events_rx),
            thread: Some(handle),
        })
    }

    /// Dynamically register a global hotkey.
    pub fn register(&self, spec: HotkeySpec) -> Result<HotkeyAvailability, PlatformError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        {
            let mut q = self.commands.lock().unwrap();
            q.push_back(Command::Register {
                spec,
                reply: reply_tx,
            });
        }
        self.wake(WM_APP_COMMAND)?;
        reply_rx
            .recv()
            .map_err(|_| PlatformError::HotkeyThreadStopped)
    }

    /// Enable or disable the `WH_KEYBOARD_LL` low-level hook fallback so `Print Screen`
    /// and global shortcuts work reliably even when Windows 11 Snipping Tool claims `VK_SNAPSHOT`.
    pub fn enable_ll_hook_fallback(&self, enabled: bool) -> Result<bool, PlatformError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        {
            let mut q = self.commands.lock().unwrap();
            q.push_back(Command::EnableLlFallback {
                enabled,
                reply: reply_tx,
            });
        }
        self.wake(WM_APP_COMMAND)?;
        reply_rx
            .recv()
            .map_err(|_| PlatformError::HotkeyThreadStopped)
    }

    /// Dynamically unregister a previously registered hotkey by ID.
    pub fn unregister(&self, id: i32) -> Result<bool, PlatformError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        {
            let mut q = self.commands.lock().unwrap();
            q.push_back(Command::Unregister {
                id,
                reply: reply_tx,
            });
        }
        self.wake(WM_APP_COMMAND)?;
        reply_rx
            .recv()
            .map_err(|_| PlatformError::HotkeyThreadStopped)
    }

    /// Non-blocking poll for the next triggered hotkey event.
    pub fn try_recv_event(&self) -> Option<HotkeyEvent> {
        self.events_rx.lock().ok()?.try_recv().ok()
    }

    fn wake(&self, msg: u32) -> Result<(), PlatformError> {
        let ok = unsafe { PostMessageW(self.hwnd as HWND, msg, 0, 0) };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            return Err(PlatformError::Win32 {
                api: "PostMessageW",
                code,
            });
        }
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.wake(WM_APP_SHUTDOWN);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn hotkey_thread_main(
    commands: Arc<Mutex<VecDeque<Command>>>,
    events_tx: mpsc::Sender<HotkeyEvent>,
    ready_tx: mpsc::SyncSender<Result<isize, PlatformError>>,
) {
    let class_name: Vec<u16> = "SnagSoupHotkeyMessageWindow\0".encode_utf16().collect();
    let hinstance = unsafe { GetModuleHandleW(null()) };

    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(hotkey_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    // Ignore ERROR_CLASS_ALREADY_EXISTS if another HotkeyManager was created in the same process.
    unsafe { RegisterClassW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            hinstance,
            null(),
        )
    };

    if hwnd.is_null() {
        let code = unsafe { GetLastError() };
        let _ = ready_tx.send(Err(PlatformError::Win32 {
            api: "CreateWindowExW",
            code,
        }));
        return;
    }

    let _ = ready_tx.send(Ok(hwnd as isize));

    let owner_key = hwnd as isize;
    let mut active_ids = HashSet::<i32>::new();
    let mut ll_hook: HHOOK = null_mut();
    let mut ll_enabled = false;
    let mut msg: MSG = unsafe { std::mem::zeroed() };

    loop {
        let ret = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }

        match msg.message {
            WM_HOTKEY => {
                if !RECORDING_PAUSED.load(Ordering::Relaxed) {
                    let _ = events_tx.send(HotkeyEvent {
                        id: msg.wParam as i32,
                    });
                }
            }
            WM_APP_COMMAND => {
                while let Some(cmd) = commands.lock().unwrap().pop_front() {
                    match cmd {
                        Command::EnableLlFallback { enabled, reply } => {
                            ll_enabled = enabled;
                            if enabled && ll_hook.is_null() {
                                ll_hook = unsafe {
                                    SetWindowsHookExW(
                                        WH_KEYBOARD_LL,
                                        Some(ll_keyboard_proc),
                                        null_mut(),
                                        0,
                                    )
                                };
                            } else if !enabled && !ll_hook.is_null() {
                                unsafe { UnhookWindowsHookEx(ll_hook) };
                                ll_hook = null_mut();
                                if let Ok(mut list) = LL_HOOK_SPECS.lock() {
                                    list.retain(|(owner, _, _)| *owner != owner_key);
                                }
                            }
                            let _ = reply.send(!ll_hook.is_null() || !enabled);
                        }
                        Command::Register { spec, reply } => {
                            if let Ok(mut list) = LL_HOOK_SPECS.lock() {
                                list.retain(|(owner, s, _)| !(owner == &owner_key && s.id == spec.id));
                            }
                            let ok = unsafe {
                                RegisterHotKey(hwnd, spec.id, spec.modifiers.bits(), spec.vk)
                            };
                            let status = if ok != 0 {
                                active_ids.insert(spec.id);
                                HotkeyAvailability::Registered
                            } else {
                                let err = unsafe { GetLastError() };
                                if ll_enabled {
                                    if let Ok(mut list) = LL_HOOK_SPECS.lock() {
                                        list.push((owner_key, spec, events_tx.clone()));
                                    }
                                }
                                if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                                    if spec.vk == VK_SNAPSHOT as u32 {
                                        HotkeyAvailability::ConflictOsSnippingTool
                                    } else {
                                        HotkeyAvailability::AlreadyRegistered
                                    }
                                } else {
                                    HotkeyAvailability::Failed(err)
                                }
                            };
                            let _ = reply.send(status);
                        }
                        Command::Unregister { id, reply } => {
                            let mut removed_ll = false;
                            if let Ok(mut list) = LL_HOOK_SPECS.lock() {
                                let before = list.len();
                                list.retain(|(owner, s, _)| !(owner == &owner_key && s.id == id));
                                removed_ll = list.len() < before;
                            }
                            let ok = unsafe { UnregisterHotKey(hwnd, id) } != 0;
                            if ok {
                                active_ids.remove(&id);
                            }
                            let _ = reply.send(ok || removed_ll);
                        }
                    }
                }
            }
            WM_APP_SHUTDOWN => {
                if !ll_hook.is_null() {
                    unsafe { UnhookWindowsHookEx(ll_hook) };
                    ll_hook = null_mut();
                }
                if let Ok(mut list) = LL_HOOK_SPECS.lock() {
                    list.retain(|(owner, _, _)| *owner != owner_key);
                }
                for id in active_ids.drain() {
                    unsafe {
                        UnregisterHotKey(hwnd, id);
                    }
                }
                unsafe {
                    DestroyWindow(hwnd);
                    PostQuitMessage(0);
                }
            }
            _ => unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
}

static SNAPSHOT_DOWN_SEEN: AtomicBool = AtomicBool::new(false);
static LAST_HELD_LL_VK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "system" fn ll_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && !RECORDING_PAUSED.load(Ordering::Relaxed) && lparam != 0 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let vk = kb.vkCode;
        let msg = wparam as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        if is_up && LAST_HELD_LL_VK.load(Ordering::Relaxed) == vk {
            LAST_HELD_LL_VK.store(0, Ordering::Relaxed);
        }

        let should_eval = if vk == VK_SNAPSHOT as u32 {
            if is_down {
                SNAPSHOT_DOWN_SEEN.store(true, Ordering::SeqCst);
                true
            } else if is_up {
                !SNAPSHOT_DOWN_SEEN.swap(false, Ordering::SeqCst)
            } else {
                false
            }
        } else if is_down {
            LAST_HELD_LL_VK.swap(vk, Ordering::SeqCst) != vk
        } else {
            false
        };

        if should_eval {
            let ctrl = unsafe { ((GetAsyncKeyState(0x11) as u16) & 0x8000) != 0 };
            let alt = unsafe { ((GetAsyncKeyState(0x12) as u16) & 0x8000) != 0 };
            let shift = unsafe { ((GetAsyncKeyState(0x10) as u16) & 0x8000) != 0 };

            if let Ok(list) = LL_HOOK_SPECS.lock() {
                for (_, spec, tx) in list.iter() {
                    if spec.vk == vk {
                        let bits = spec.modifiers.bits();
                        let want_ctrl = (bits & MOD_CONTROL) != 0;
                        let want_alt = (bits & MOD_ALT) != 0;
                        let want_shift = (bits & MOD_SHIFT) != 0;
                        if ctrl == want_ctrl && alt == want_alt && shift == want_shift {
                            let _ = tx.send(HotkeyEvent { id: spec.id });
                            return 1; // Swallow key so OS Snipping Tool doesn't hijack Print Screen!
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn hotkey_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_register_unregister_and_clean_drop() {
        let mgr = HotkeyManager::new().expect("HotkeyManager should start");

        // Ctrl+Alt+Shift+F20 (VK_F20 = 0x83) is virtually guaranteed unused on Windows.
        let spec = HotkeySpec::new(
            1001,
            HotkeyModifiers::CONTROL
                .union(HotkeyModifiers::ALT)
                .union(HotkeyModifiers::SHIFT)
                .union(HotkeyModifiers::NOREPEAT),
            0x83,
        );
        let status = mgr.register(spec).expect("register IPC should succeed");
        assert_eq!(status, HotkeyAvailability::Registered);

        // Registering the same combo again with a different ID should report AlreadyRegistered.
        let dup = HotkeySpec::new(1002, spec.modifiers, spec.vk);
        let dup_status = mgr.register(dup).expect("dup register IPC should succeed");
        assert_eq!(dup_status, HotkeyAvailability::AlreadyRegistered);

        let unregistered = mgr.unregister(1001).expect("unregister IPC should succeed");
        assert!(unregistered);

        // Drop runs WM_APP_SHUTDOWN -> DestroyWindow -> PostQuitMessage(0) and joins thread.
        drop(mgr);
    }

    #[test]
    fn print_screen_hotkey_reports_valid_availability() {
        let mgr = HotkeyManager::new().expect("HotkeyManager should start");
        let status = mgr
            .register(HotkeySpec::print_screen(2001))
            .expect("PrintScreen register IPC should succeed");
        assert!(
            matches!(
                status,
                HotkeyAvailability::Registered | HotkeyAvailability::ConflictOsSnippingTool
            ),
            "unexpected PrintScreen status: {status:?}"
        );
        if status == HotkeyAvailability::Registered {
            assert!(mgr.unregister(2001).expect("unregister should succeed"));
        }
    }
}
