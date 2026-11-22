//! Tiered Single-Process Daemon coordinator (`AppCoordinator`).
//!
//! - **Tier 0 (`--tray`)**: Lightweight Win32 tray icon + `snagsoup-win-platform` global
//!   hotkey loop. Zero GPUI or Direct3D 11 allocation -> idle working set `< 12 MB`.
//! - **Tier 1/2 (On-Demand)**: Pressing `PrintScreen` / `Ctrl+Shift+S` or clicking Tray menu
//!   items spawns a fresh child `snagsoup.exe --capture-now` / `--capture` / `--editor` so the
//!   Tier 0 `--tray` daemon remains permanently alive at `< 12 MB` RAM and 100% of GPU/Editor
//!   memory is reclaimed by the OS the instant the child window closes!

use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use snagsoup_ui::theme::ThemeMode;
use snagsoup_win_platform::dpi;
use snagsoup_win_platform::hotkey::{HotkeyAvailability, HotkeyManager, HotkeyModifiers, HotkeySpec};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DBLCLKS, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, LoadIconW, LoadImageW, MF_SEPARATOR, MF_STRING, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
    SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK,
    WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use crate::memory::{query_process_memory, reclaim_process_memory};

const HOTKEY_CAPTURE_PRIMARY: i32 = 1;
const HOTKEY_CAPTURE_FALLBACK: i32 = 2;

const WM_TRAY_ICON: u32 = WM_APP + 10;
const WM_APP_HOTKEY_CAPTURE: u32 = WM_APP + 11;
const TRAY_ICON_ID: u32 = 1;

const CMD_CAPTURE_NOW: u32 = 1000;
const CMD_OPEN_CAPTURE_WIN: u32 = 1001;
const CMD_OPEN_EDITOR: u32 = 1002;
const CMD_TRIM_MEMORY: u32 = 1003;
const CMD_EXIT: u32 = 1004;

/// Central lifecycle coordinator for SnagSoup.
pub struct AppCoordinator {
    theme_mode: ThemeMode,
}

impl AppCoordinator {
    pub fn new(theme_mode: ThemeMode) -> Self {
        Self { theme_mode }
    }

    /// Initialize DPI awareness and start the Tier 0 lightweight hotkey daemon,
    /// honoring the user's configured `global_capture_hotkey` in `preferences.json`.
    pub fn start_tier0_hotkeys() -> Result<(HotkeyManager, HotkeyAvailability), String> {
        let _ = dpi::init_per_monitor_dpi_v2();
        let manager =
            HotkeyManager::new().map_err(|e| format!("failed to start HotkeyManager: {e}"))?;
        let _ = manager.enable_ll_hook_fallback(true);

        let prefs = snagsoup_ui::capture_shell::capture_window::CapturePreferences::load_or_default();
        let primary_spec = HotkeySpec::from_display_str(
            HOTKEY_CAPTURE_PRIMARY,
            &prefs.global_capture_hotkey,
        )
        .unwrap_or_else(|| HotkeySpec::print_screen(HOTKEY_CAPTURE_PRIMARY));

        let status = manager
            .register(primary_spec)
            .map_err(|e| format!("failed to register hotkey: {e}"))?;

        if status != HotkeyAvailability::Registered {
            // Register Ctrl+Shift+S fallback when OS Snipping Tool owns PrintScreen.
            let _ = manager.register(HotkeySpec::new(
                HOTKEY_CAPTURE_FALLBACK,
                HotkeyModifiers::CONTROL
                    .union(HotkeyModifiers::SHIFT)
                    .union(HotkeyModifiers::NOREPEAT),
                0x53, // 'S'
            ));
        }

        Ok((manager, status))
    }

    /// Self-test mode (`--check-mem`): initializes Tier 0 daemon (no GPUI/D3D11),
    /// measures memory via `K32GetProcessMemoryInfo`, prints report, and exits cleanly.
    pub fn run_check_mem(&self) -> Result<(), String> {
        let (_hotkeys, hotkey_status) = Self::start_tier0_hotkeys()?;
        let stats = query_process_memory()
            .map_err(|code| format!("K32GetProcessMemoryInfo failed with Win32 error {code}"))?;

        println!("SnagSoup Tier-0 Idle Memory Check");
        println!("  Hotkey status       : {hotkey_status:?}");
        println!(
            "  Working Set         : {:.2} MB ({} bytes)",
            stats.working_set_mb(),
            stats.working_set_bytes
        );
        println!(
            "  Private Usage       : {:.2} MB ({} bytes)",
            stats.private_usage_mb(),
            stats.private_usage_bytes
        );
        println!(
            "  Peak Working Set    : {:.2} MB ({} bytes)",
            stats.peak_working_set_mb(),
            stats.peak_working_set_bytes
        );
        println!(
            "  Target (< 15.00 MB) : {}",
            if stats.working_set_mb() < 15.0 {
                "PASS"
            } else {
                "OVER_BUDGET"
            }
        );

        Ok(())
    }

    /// Direct GPUI Editor window (`--editor [optional.snagx]`).
    pub fn run_editor(&self, initial_file: Option<std::path::PathBuf>) {
        let _ = dpi::init_per_monitor_dpi_v2();
        snagsoup_ui::run_editor_window(self.theme_mode, initial_file);
        let _ = reclaim_process_memory();
    }

    /// Direct GPUI 3-tab Capture window (`--capture`).
    pub fn run_capture_window(&self) {
        let _ = dpi::init_per_monitor_dpi_v2();
        snagsoup_ui::run_capture_window(self.theme_mode);
        let _ = reclaim_process_memory();
    }

    /// Direct GPUI OneClick floating widget (`--oneclick`).
    pub fn run_oneclick_window(&self) {
        let _ = dpi::init_per_monitor_dpi_v2();
        snagsoup_ui::run_oneclick_window(self.theme_mode);
        let _ = reclaim_process_memory();
    }

    /// Immediate fullscreen Selection Overlay -> Editor flow (`--capture-now`).
    pub fn run_instant_capture(&self) {
        let _ = dpi::init_per_monitor_dpi_v2();
        snagsoup_ui::run_instant_capture_flow(self.theme_mode);
        let _ = reclaim_process_memory();
    }

    /// Tier 0 Tray Daemon (`--tray`): sits in a lightweight Win32 tray loop
    /// without initializing GPUI until the user triggers a capture or opens the Editor.
    pub fn run_tray_daemon(&self) -> Result<(), String> {
        // Check for any unfinished crash-safe recordings on startup
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let recovery_dir = std::path::PathBuf::from(local_app_data)
                .join("SnagSoup")
                .join("Recovery");
            if recovery_dir.exists() {
                let _ = snagsoup_media::DisasterRecoveryManager::recover_unfinished_recordings(
                    &recovery_dir,
                );
            }
        }

        let (hotkeys, hotkey_status) = Self::start_tier0_hotkeys()?;
        if let Ok(stats) = query_process_memory() {
            eprintln!(
                "[snagsoup-tray] Tier 0 started (hotkey={hotkey_status:?}, working_set={:.2} MB, private={:.2} MB)",
                stats.working_set_mb(),
                stats.private_usage_mb()
            );
        }

        run_win32_tray_loop(hotkeys)?;
        let _ = reclaim_process_memory();
        Ok(())
    }
}

fn spawn_self_with_flag(flag: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(flag).spawn();
    }
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_win32_tray_loop(hotkeys: HotkeyManager) -> Result<(), String> {
    let class_name = to_wide_null("SnagSoupTrayWindowClass");
    let window_title = to_wide_null("SnagSoup Tray");

    unsafe {
        let hinstance = GetModuleHandleW(null());
        let app_big_icon = LoadImageW(
            hinstance,
            1 as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        );
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(tray_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: if !app_big_icon.is_null() {
                app_big_icon
            } else {
                LoadIconW(null_mut(), IDI_APPLICATION)
            },
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        );
        if hwnd.is_null() {
            return Err("failed to create Win32 tray host window".to_string());
        }

        // Spawn lightweight hotkey-bridge thread posting WM_APP_HOTKEY_CAPTURE to tray hwnd
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop_flag);
        let hwnd_raw = hwnd as isize;
        let hk_thread = std::thread::Builder::new()
            .name("snagsoup-tray-hotkey-bridge".into())
            .spawn(move || {
                while !stop_bg.load(Ordering::Relaxed) {
                    if let Some(_ev) = hotkeys.try_recv_event() {
                        PostMessageW(hwnd_raw as HWND, WM_APP_HOTKEY_CAPTURE, 0, 0);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            })
            .ok();

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY_ICON;
        let cx_sm = GetSystemMetrics(SM_CXSMICON).max(16);
        let cy_sm = GetSystemMetrics(SM_CYSMICON).max(16);
        let tray_sm_icon =
            LoadImageW(hinstance, 1 as *const u16, IMAGE_ICON, cx_sm, cy_sm, LR_SHARED);
        nid.hIcon = if !tray_sm_icon.is_null() {
            tray_sm_icon
        } else {
            LoadIconW(null_mut(), IDI_APPLICATION)
        };
        let tip = to_wide_null(&format!(
            "{} — Press PrintScreen / Ctrl+Shift+S to Capture",
            snagsoup_ui::APP_TITLE
        ));
        let copy_len = tip.len().min(nid.szTip.len());
        nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);

        Shell_NotifyIconW(NIM_ADD, &nid);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        stop_flag.store(true, Ordering::Relaxed);
        if let Some(t) = hk_thread {
            let _ = t.join();
        }

        Shell_NotifyIconW(NIM_DELETE, &nid);
        DestroyWindow(hwnd);
    }

    Ok(())
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_HOTKEY_CAPTURE => {
            spawn_self_with_flag("--capture-now");
            0
        }
        WM_TRAY_ICON => {
            let event = (lparam & 0xFFFF) as u32;
            if event == WM_LBUTTONDBLCLK {
                spawn_self_with_flag("--capture");
                return 0;
            }
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                unsafe {
                    let menu = CreatePopupMenu();
                    if !menu.is_null() {
                        let restore_lbl = to_wide_null("Restore");
                        let cap_now_lbl = to_wide_null("Capture Now");
                        let open_lbl = to_wide_null("Open Editor");
                        let trim_lbl = to_wide_null("Trim Memory");
                        let exit_lbl = to_wide_null("Exit");

                        AppendMenuW(menu, MF_STRING, CMD_OPEN_CAPTURE_WIN as usize, restore_lbl.as_ptr());
                        AppendMenuW(menu, MF_STRING, CMD_CAPTURE_NOW as usize, cap_now_lbl.as_ptr());
                        AppendMenuW(menu, MF_STRING, CMD_OPEN_EDITOR as usize, open_lbl.as_ptr());
                        AppendMenuW(menu, MF_SEPARATOR, 0, null());
                        AppendMenuW(menu, MF_STRING, CMD_TRIM_MEMORY as usize, trim_lbl.as_ptr());
                        AppendMenuW(menu, MF_SEPARATOR, 0, null());
                        AppendMenuW(menu, MF_STRING, CMD_EXIT as usize, exit_lbl.as_ptr());
                        let _ = SetMenuDefaultItem(menu, CMD_OPEN_CAPTURE_WIN, 0);

                        let mut pt = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        let cmd = TrackPopupMenu(
                            menu,
                            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
                            pt.x,
                            pt.y,
                            0,
                            hwnd,
                            null(),
                        ) as u32;
                        PostMessageW(hwnd, WM_NULL, 0, 0);
                        DestroyMenu(menu);

                        match cmd {
                            CMD_CAPTURE_NOW => spawn_self_with_flag("--capture-now"),
                            CMD_OPEN_CAPTURE_WIN => spawn_self_with_flag("--capture"),
                            CMD_OPEN_EDITOR => spawn_self_with_flag("--editor"),
                            CMD_TRIM_MEMORY => {
                                let _ = reclaim_process_memory();
                            }
                            CMD_EXIT => {
                                PostQuitMessage(0);
                            }
                            _ => {}
                        }
                    }
                }
                return 0;
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
