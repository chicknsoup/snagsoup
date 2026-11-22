//! Single-instance enforcement via a fixed Application GUID (`CreateMutexW` in `Local\`
//! session namespace) plus `WM_COPYDATA` IPC forwarding (`HWND_MESSAGE` host window) so
//! launching a second copy — even if `snagsoup.exe` is renamed or copied elsewhere —
//! activates the running instance and exits cleanly.

use crate::PlatformError;
use std::cell::RefCell;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowExW, GetMessageW, GetWindowThreadProcessId, HWND_MESSAGE, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, TranslateMessage,
    WM_APP, WM_COPYDATA, WNDCLASSW,
};

/// Canonical Application GUID uniquely identifying SnagSoup across the user's Windows session,
/// independent of executable filename or installation directory.
pub const APP_INSTANCE_GUID: &str = "8F4E2D9A-7C31-4B6E-A1D5-9E0F3C82B741";

const SNAGSOUP_IPC_MAGIC: usize = 0x534E_4147; // "SNAG"
const WM_IPC_SHUTDOWN: u32 = WM_APP + 42;

thread_local! {
    static THREAD_IPC_TX: RefCell<Option<mpsc::Sender<InstanceCommand>>> = const { RefCell::new(None) };
}

/// Command forwarded from a secondary launch attempt to the primary running instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceCommand {
    /// Restore and focus the Capture App Window.
    ActivateCaptureWindow,
    /// Open or focus the Editor Window (optionally loading a `.snagx` / `.mp4` file).
    OpenEditor(Option<PathBuf>),
    /// Immediately trigger screen capture on the primary instance.
    CaptureNow,
    /// Open or focus the OneClick floating bar.
    ActivateOneClick,
}

impl InstanceCommand {
    /// Encode into a compact UTF-8 wire string for `WM_COPYDATA`.
    pub fn encode_wire(&self) -> String {
        match self {
            Self::ActivateCaptureWindow => "CAPTURE_WIN".to_string(),
            Self::CaptureNow => "CAPTURE_NOW".to_string(),
            Self::ActivateOneClick => "ONECLICK".to_string(),
            Self::OpenEditor(None) => "EDITOR:".to_string(),
            Self::OpenEditor(Some(path)) => format!("EDITOR:{}", path.to_string_lossy()),
        }
    }

    /// Decode a UTF-8 wire string received from `WM_COPYDATA`.
    pub fn decode_wire(s: &str) -> Option<Self> {
        let trimmed = s.trim_matches('\0');
        if trimmed == "CAPTURE_WIN" {
            Some(Self::ActivateCaptureWindow)
        } else if trimmed == "CAPTURE_NOW" {
            Some(Self::CaptureNow)
        } else if trimmed == "ONECLICK" {
            Some(Self::ActivateOneClick)
        } else if let Some(rest) = trimmed.strip_prefix("EDITOR:") {
            if rest.is_empty() {
                Some(Self::OpenEditor(None))
            } else {
                Some(Self::OpenEditor(Some(PathBuf::from(rest))))
            }
        } else {
            None
        }
    }
}

/// Result of attempting to acquire the GUID single-instance lock.
pub enum AcquireOutcome {
    /// Current process is the sole primary instance and now owns the GUID Mutex + IPC host.
    Primary(SingleInstanceGuard),
    /// Another SnagSoup instance (even under a different `.exe` name) is already running.
    AlreadyRunning,
}

/// RAII guard holding the OS kernel Named Mutex (`Local\SnagSoup_Instance_<GUID>`) and
/// background `HWND_MESSAGE` IPC receiver for the lifetime of the primary process.
pub struct SingleInstanceGuard {
    mutex_handle: isize,
    ipc_hwnd: isize,
    rx: Mutex<mpsc::Receiver<InstanceCommand>>,
    thread: Option<JoinHandle<()>>,
}

unsafe impl Send for SingleInstanceGuard {}
unsafe impl Sync for SingleInstanceGuard {}

impl SingleInstanceGuard {
    /// Acquire the session-wide SnagSoup single-instance lock using [`APP_INSTANCE_GUID`].
    pub fn try_acquire() -> Result<AcquireOutcome, PlatformError> {
        Self::try_acquire_with_guid(APP_INSTANCE_GUID)
    }

    /// Acquire a single-instance lock keyed by `guid` (configurable for isolated unit tests).
    pub fn try_acquire_with_guid(guid: &str) -> Result<AcquireOutcome, PlatformError> {
        let mutex_name = to_wide_null(&mutex_object_name(guid));
        let handle = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
        if handle.is_null() {
            let code = unsafe { GetLastError() };
            return Err(PlatformError::Win32 {
                api: "CreateMutexW",
                code,
            });
        }

        let last_err = unsafe { GetLastError() };
        if last_err == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Ok(AcquireOutcome::AlreadyRunning);
        }

        let ipc_name = ipc_window_name(guid);
        let (cmd_tx, cmd_rx) = mpsc::channel::<InstanceCommand>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<isize, PlatformError>>(1);

        let thread = match thread::Builder::new()
            .name("snagsoup-single-instance-ipc".to_string())
            .spawn(move || ipc_thread_main(&ipc_name, cmd_tx, ready_tx))
        {
            Ok(t) => t,
            Err(_) => {
                unsafe {
                    CloseHandle(handle);
                }
                return Err(PlatformError::HotkeyThreadStopped);
            }
        };

        let ipc_hwnd = match ready_rx.recv() {
            Ok(Ok(hwnd)) => hwnd,
            Ok(Err(e)) => {
                unsafe {
                    CloseHandle(handle);
                }
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                unsafe {
                    CloseHandle(handle);
                }
                let _ = thread.join();
                return Err(PlatformError::HotkeyThreadStopped);
            }
        };

        Ok(AcquireOutcome::Primary(Self {
            mutex_handle: handle as isize,
            ipc_hwnd,
            rx: Mutex::new(cmd_rx),
            thread: Some(thread),
        }))
    }

    /// Non-blocking poll for activation commands sent by secondary launch attempts.
    pub fn try_recv(&self) -> Option<InstanceCommand> {
        self.rx.lock().ok()?.try_recv().ok()
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if self.ipc_hwnd != 0 {
            unsafe {
                PostMessageW(self.ipc_hwnd as HWND, WM_IPC_SHUTDOWN, 0, 0);
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        if self.mutex_handle != 0 {
            unsafe {
                CloseHandle(self.mutex_handle as HANDLE);
            }
            self.mutex_handle = 0;
        }
    }
}

/// Forward `cmd` to the already-running SnagSoup instance identified by [`APP_INSTANCE_GUID`],
/// granting it `AllowSetForegroundWindow` permission so it can raise its window cleanly.
pub fn notify_existing_instance(cmd: &InstanceCommand) -> bool {
    notify_existing_instance_with_guid(APP_INSTANCE_GUID, cmd)
}

/// Forward `cmd` to an existing instance keyed by `guid`.
pub fn notify_existing_instance_with_guid(guid: &str, cmd: &InstanceCommand) -> bool {
    let win_name = to_wide_null(&ipc_window_name(guid));

    // Retry briefly in case two launches raced within milliseconds and the primary's
    // message-only IPC window is still finishing creation.
    let mut target_hwnd: HWND = null_mut();
    for _ in 0..8 {
        target_hwnd =
            unsafe { FindWindowExW(HWND_MESSAGE, null_mut(), win_name.as_ptr(), win_name.as_ptr()) };
        if !target_hwnd.is_null() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    if target_hwnd.is_null() {
        return false;
    }

    unsafe {
        let mut primary_pid: u32 = 0;
        GetWindowThreadProcessId(target_hwnd, &mut primary_pid);
        if primary_pid != 0 {
            let _ = AllowSetForegroundWindow(primary_pid);
        }

        let payload = cmd.encode_wire();
        let bytes = payload.as_bytes();
        let cds = COPYDATASTRUCT {
            dwData: SNAGSOUP_IPC_MAGIC,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut core::ffi::c_void,
        };
        let mut result: usize = 0;
        let ok = SendMessageTimeoutW(
            target_hwnd,
            WM_COPYDATA,
            0,
            (&cds as *const COPYDATASTRUCT) as LPARAM,
            SMTO_ABORTIFHUNG,
            1500,
            &mut result,
        );
        ok != 0 && result != 0
    }
}

fn mutex_object_name(guid: &str) -> String {
    format!("Local\\SnagSoup_Instance_{guid}")
}

fn ipc_window_name(guid: &str) -> String {
    format!("SnagSoupIPC_{guid}")
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ipc_thread_main(
    ipc_name: &str,
    cmd_tx: mpsc::Sender<InstanceCommand>,
    ready_tx: mpsc::SyncSender<Result<isize, PlatformError>>,
) {
    THREAD_IPC_TX.with(|cell| {
        *cell.borrow_mut() = Some(cmd_tx);
    });

    let wide_name = to_wide_null(ipc_name);
    let hinstance = unsafe { GetModuleHandleW(null()) };

    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(ipc_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: wide_name.as_ptr(),
    };
    unsafe {
        RegisterClassW(&wc);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            wide_name.as_ptr(),
            wide_name.as_ptr(),
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
            api: "CreateWindowExW(SnagSoupIPC)",
            code,
        }));
        return;
    }

    let _ = ready_tx.send(Ok(hwnd as isize));

    let mut msg = unsafe { std::mem::zeroed::<MSG>() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        if msg.message == WM_IPC_SHUTDOWN {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        DestroyWindow(hwnd);
    }
    debug_assert!(size_of::<COPYDATASTRUCT>() > 0);
}

unsafe extern "system" fn ipc_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COPYDATA if lparam != 0 => {
            let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
            if cds.dwData == SNAGSOUP_IPC_MAGIC && !cds.lpData.is_null() && cds.cbData <= 16_384 {
                let slice =
                    unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
                if let Ok(text) = std::str::from_utf8(slice) {
                    if let Some(cmd) = InstanceCommand::decode_wire(text) {
                        THREAD_IPC_TX.with(|cell| {
                            if let Some(tx) = cell.borrow().as_ref() {
                                let _ = tx.send(cmd);
                            }
                        });
                        return 1;
                    }
                }
            }
            0
        }
        WM_IPC_SHUTDOWN => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_encoding_roundtrips_all_commands() {
        let cmds = [
            InstanceCommand::ActivateCaptureWindow,
            InstanceCommand::CaptureNow,
            InstanceCommand::ActivateOneClick,
            InstanceCommand::OpenEditor(None),
            InstanceCommand::OpenEditor(Some(PathBuf::from(r"F:\Captures\demo test.snagx"))),
        ];
        for cmd in cmds {
            let wire = cmd.encode_wire();
            assert_eq!(InstanceCommand::decode_wire(&wire).as_ref(), Some(&cmd));
        }
        assert_eq!(InstanceCommand::decode_wire("INVALID_CMD"), None);
    }

    #[test]
    fn guid_mutex_blocks_second_acquire_and_delivers_wm_copydata_ipc() {
        let test_guid = "TEST-GUID-9912A4B0-C3E1-4F77-8D10-55AA77BB99CC";

        let first =
            SingleInstanceGuard::try_acquire_with_guid(test_guid).expect("first acquire ok");
        let guard = match first {
            AcquireOutcome::Primary(g) => g,
            AcquireOutcome::AlreadyRunning => panic!("expected first instance to be Primary"),
        };

        // Second attempt with the exact same GUID must report AlreadyRunning regardless of exe name
        let second =
            SingleInstanceGuard::try_acquire_with_guid(test_guid).expect("second acquire ok");
        assert!(matches!(second, AcquireOutcome::AlreadyRunning));

        // Secondary instance forwards activation + Editor path via WM_COPYDATA
        assert!(notify_existing_instance_with_guid(
            test_guid,
            &InstanceCommand::ActivateCaptureWindow,
        ));
        let sample_path = PathBuf::from(r"C:\Users\demo\capture_01.snagx");
        assert!(notify_existing_instance_with_guid(
            test_guid,
            &InstanceCommand::OpenEditor(Some(sample_path.clone())),
        ));

        assert_eq!(guard.try_recv(), Some(InstanceCommand::ActivateCaptureWindow));
        assert_eq!(
            guard.try_recv(),
            Some(InstanceCommand::OpenEditor(Some(sample_path)))
        );
        assert_eq!(guard.try_recv(), None);

        // Dropping the primary guard releases the kernel mutex so a subsequent launch succeeds
        drop(guard);
        let after_drop =
            SingleInstanceGuard::try_acquire_with_guid(test_guid).expect("re-acquire after drop");
        assert!(matches!(after_drop, AcquireOutcome::Primary(_)));
    }
}
