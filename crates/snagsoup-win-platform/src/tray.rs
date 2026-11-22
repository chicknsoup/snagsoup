//! Lightweight Win32 System Tray icon (`Shell_NotifyIconW`) with right-click popup
//! context menu (`Restore`, `Capture Now`, `Open Editor`, `Capture Preferences...`, `Exit`),
//! double-click `OpenCaptureWindow` dispatch, and clean removal on `Drop`.

use crate::PlatformError;
use std::cell::RefCell;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DBLCLKS, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, IDI_APPLICATION,
    IMAGE_ICON, LR_SHARED, LoadIconW, LoadImageW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
    SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW,
};

pub const WM_TRAY_CALLBACK: u32 = WM_APP + 10;
const WM_TRAY_SHUTDOWN: u32 = WM_APP + 11;

const MENU_CAPTURE: usize = 1001;
const MENU_OPEN_EDITOR: usize = 1002;
const MENU_PRESETS: usize = 1003;
const MENU_PREFERENCES: usize = 1004;
const MENU_QUIT: usize = 1005;
const MENU_RESTORE_CAPTURE_WIN: usize = 1006;

thread_local! {
    static THREAD_TRAY_TX: RefCell<Option<mpsc::Sender<TrayCommand>>> = const { RefCell::new(None) };
}

/// Commands triggered from the System Tray icon or its context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Capture,
    OpenCaptureWindow,
    OpenEditor,
    Presets,
    Preferences,
    Quit,
}

/// Active Win32 System Tray icon running on a lightweight dedicated message thread.
pub struct TrayIcon {
    hwnd: isize,
    rx: Mutex<mpsc::Receiver<TrayCommand>>,
    thread: Option<JoinHandle<()>>,
}

unsafe impl Send for TrayIcon {}
unsafe impl Sync for TrayIcon {}

impl TrayIcon {
    /// Create and show a Win32 System Tray icon with the given tooltip.
    pub fn new(tooltip: &str) -> Result<Self, PlatformError> {
        let tooltip_owned = tooltip.to_string();
        let (cmd_tx, cmd_rx) = mpsc::channel::<TrayCommand>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<isize, PlatformError>>(1);

        let handle = thread::Builder::new()
            .name("snagsoup-tray-loop".to_string())
            .spawn(move || tray_thread_main(&tooltip_owned, cmd_tx, ready_tx))
            .map_err(|_| PlatformError::HotkeyThreadStopped)?;

        let hwnd = ready_rx
            .recv()
            .map_err(|_| PlatformError::HotkeyThreadStopped)??;

        Ok(Self {
            hwnd,
            rx: Mutex::new(cmd_rx),
            thread: Some(handle),
        })
    }

    /// Underlying message window `HWND` (exposed for unit testing callback dispatch).
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Non-blocking poll for the next tray menu command.
    pub fn try_recv(&self) -> Option<TrayCommand> {
        self.rx.lock().ok()?.try_recv().ok()
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_TRAY_SHUTDOWN, 0, 0);
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn tray_thread_main(
    tooltip: &str,
    cmd_tx: mpsc::Sender<TrayCommand>,
    ready_tx: mpsc::SyncSender<Result<isize, PlatformError>>,
) {
    THREAD_TRAY_TX.with(|cell| {
        *cell.borrow_mut() = Some(cmd_tx);
    });

    let class_name: Vec<u16> = "SnagSoupTrayWindow\0".encode_utf16().collect();
    let hinstance = unsafe { GetModuleHandleW(null()) };

    let wc = WNDCLASSW {
        style: CS_DBLCLKS,
        lpfnWndProc: Some(tray_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
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
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        )
    };
    if hwnd.is_null() {
        let code = unsafe { GetLastError() };
        THREAD_TRAY_TX.with(|cell| *cell.borrow_mut() = None);
        let _ = ready_tx.send(Err(PlatformError::Win32 {
            api: "CreateWindowExW",
            code,
        }));
        return;
    }

    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    let cx_sm = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let cy_sm = unsafe { GetSystemMetrics(SM_CYSMICON) }.max(16);
    let custom_icon =
        unsafe { LoadImageW(hinstance, 1 as *const u16, IMAGE_ICON, cx_sm, cy_sm, LR_SHARED) };
    nid.hIcon = if !custom_icon.is_null() {
        custom_icon
    } else {
        unsafe { LoadIconW(null_mut(), IDI_APPLICATION) }
    };

    for (dst, ch) in nid.szTip.iter_mut().zip(tooltip.encode_utf16().take(127)) {
        *dst = ch;
    }

    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) } != 0;
    let _ = ready_tx.send(Ok(hwnd as isize));

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        if msg.message == WM_TRAY_SHUTDOWN {
            if added {
                unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
            }
            unsafe {
                DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
        } else {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    THREAD_TRAY_TX.with(|cell| *cell.borrow_mut() = None);
}

fn show_tray_context_menu(hwnd: HWND) -> Option<TrayCommand> {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return None;
    }

    let append = |id: usize, label: &str| {
        let w: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { AppendMenuW(menu, MF_STRING, id, w.as_ptr()) };
    };
    append(MENU_RESTORE_CAPTURE_WIN, "Restore");
    append(MENU_CAPTURE, "Capture Now");
    append(MENU_OPEN_EDITOR, "Open Editor");
    append(MENU_PREFERENCES, "Capture Preferences...");
    unsafe {
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    }
    append(MENU_QUIT, "Exit");

    // Highlight "Restore" in bold as the default double-click action
    unsafe {
        let _ = SetMenuDefaultItem(menu, MENU_RESTORE_CAPTURE_WIN as u32, 0);
    }

    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
    }
    let selected = unsafe {
        TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        )
    } as usize;
    unsafe {
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }

    match selected {
        MENU_RESTORE_CAPTURE_WIN => Some(TrayCommand::OpenCaptureWindow),
        MENU_CAPTURE => Some(TrayCommand::Capture),
        MENU_OPEN_EDITOR => Some(TrayCommand::OpenEditor),
        MENU_PRESETS => Some(TrayCommand::Presets),
        MENU_PREFERENCES => Some(TrayCommand::Preferences),
        MENU_QUIT => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// Window procedure receiving `Shell_NotifyIconW` (`SendNotifyMessageW`) callbacks directly from `explorer.exe`.
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY_CALLBACK {
        let event = (lparam & 0xFFFF) as u32;
        if event == WM_LBUTTONDBLCLK {
            THREAD_TRAY_TX.with(|cell| {
                if let Some(ref tx) = *cell.borrow() {
                    let _ = tx.send(TrayCommand::OpenCaptureWindow);
                }
            });
            return 0;
        }
        if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
            if let Some(cmd) = show_tray_context_menu(hwnd) {
                THREAD_TRAY_TX.with(|cell| {
                    if let Some(ref tx) = *cell.borrow() {
                        let _ = tx.send(cmd);
                    }
                });
            }
            return 0;
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::SendNotifyMessageW;

    #[test]
    fn tray_icon_double_click_dispatches_open_capture_window_via_wndproc() {
        let tray = TrayIcon::new("SnagSoup Test").expect("tray icon thread should start");
        assert_eq!(tray.try_recv(), None);

        // Simulate `explorer.exe` delivering WM_LBUTTONDBLCLK via `SendNotifyMessageW` directly to `tray_wnd_proc`
        unsafe {
            let _ = SendNotifyMessageW(
                tray.hwnd() as HWND,
                WM_TRAY_CALLBACK,
                1,
                WM_LBUTTONDBLCLK as isize,
            );
        }

        let mut received = None;
        for _ in 0..25 {
            if let Some(cmd) = tray.try_recv() {
                received = Some(cmd);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(received, Some(TrayCommand::OpenCaptureWindow));
        drop(tray);
    }
}
