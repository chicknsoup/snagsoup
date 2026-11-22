//! SnagSoup GPUI User Interface crate.
//!
//! Unifies the Light/Dark design system (`theme`), embedded SVG vector icons (`icons`),
//! reusable lightweight GPUI components (`widgets`), Capture/Editor shells (`capture_shell`,
//! `editor_shell`), floating HUDs (`huds`), and Library/Templates/Share views (`library_views`).

pub mod capture_shell;
pub mod editor_shell;
pub mod icons;
pub mod theme;
pub mod widgets;

pub use snagsoup_ui_huds as huds;
pub use snagsoup_ui_library as library_views;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    App, AppContext, Application, AssetSource, Bounds, RenderImage, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions, px, size,
};

/// 4-component build version string (`x.x.x.x`).
pub const APP_VERSION_FULL: &str = "1.3.0.0";

/// Canonical application window title (`SnagSoup vx.x.x.x`).
pub const APP_TITLE: &str = "SnagSoup v1.3.0.0";

/// Standard native Windows 11 titlebar configuration displaying `[Emerald S Icon] SnagSoup v1.0.0.0`.
pub fn default_titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some(SharedString::from(APP_TITLE)),
        appears_transparent: false,
        ..Default::default()
    }
}

/// Convert an `RGBA8` pixel buffer into a GPUI [`RenderImage`].
///
/// GPUI's DX11/Metal `RenderImage` expects **BGRA8** byte order inside `image::RgbaImage`
/// (see `vendor/zed/crates/gpui/src/assets.rs:42`). Swapping `R <-> B` here prevents
/// red/blue channel inversion (e.g. blue Windows 11 wallpapers turning brown-orange).
pub fn rgba_to_bgra_render_image(width: u32, height: u32, rgba: &[u8]) -> Option<Arc<RenderImage>> {
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return None;
    }
    let mut bgra = rgba.to_vec();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(width, height, bgra)?;
    let frame = image::Frame::new(buf);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

/// Zero-copy move of a native **BGRA8** `Vec<u8>` directly into a GPUI [`RenderImage`]
/// (`0` allocations, `0` per-pixel swizzle loops!).
pub fn bgra_vec_to_render_image(
    width: u32,
    height: u32,
    bgra: Vec<u8>,
) -> Option<Arc<RenderImage>> {
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if width == 0 || height == 0 || bgra.len() != expected {
        return None;
    }
    let buf = image::RgbaImage::from_raw(width, height, bgra)?;
    let frame = image::Frame::new(buf);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
use snagsoup_canvas::TiledBitmap;
use snagsoup_capture::ScreenFreezer;
use snagsoup_model::{CaptureKind, CaptureObject, CapturePreset, RectF, SelectionMode};

use crate::capture_shell::capture_window::{
    ABOUT_DIALOG_HEIGHT, ABOUT_DIALOG_WIDTH, AboutDialogView, CAPTURE_WIN_COMPACT_HEIGHT,
    CAPTURE_WIN_WIDTH, CapturePreferences, CapturePreferencesView, PREFS_DIALOG_HEIGHT,
    PREFS_DIALOG_WIDTH, PreferencesTab,
};
use crate::capture_shell::overlay::{
    VideoRecordRequest, crop_frozen_screen, open_selection_overlay_window_full,
};
use crate::capture_shell::{
    CaptureWindowView, LiveRecordingHudView, OneClickWidget, open_live_recording_hud,
};
use crate::editor_shell::EditorShellView;
use crate::editor_shell::shell::{EditorShellState, WorkspaceView, capture_storage_dir};
use crate::icons::SnagIcon;
use crate::theme::{SnagTheme, ThemeMode};

thread_local! {
    static ACTIVE_EDITOR_WINDOW: RefCell<Option<WindowHandle<EditorShellView>>> = const { RefCell::new(None) };
    static ACTIVE_CAPTURE_WINDOW: RefCell<Option<WindowHandle<CaptureWindowView>>> = const { RefCell::new(None) };
    static ACTIVE_PREFS_WINDOW: RefCell<Option<WindowHandle<CapturePreferencesView>>> = const { RefCell::new(None) };
    static ACTIVE_ABOUT_WINDOW: RefCell<Option<WindowHandle<AboutDialogView>>> = const { RefCell::new(None) };
    static ACTIVE_RECORDING_HUD: RefCell<Option<WindowHandle<LiveRecordingHudView>>> = const { RefCell::new(None) };
    static LAST_CAPTURE_RECT: RefCell<Option<RectF>> = const { RefCell::new(None) };
    static LIVE_HOTKEY_MGR: RefCell<Option<Arc<snagsoup_win_platform::hotkey::HotkeyManager>>> = const { RefCell::new(None) };
    static LIVE_TRAY_ICON: RefCell<Option<Arc<snagsoup_win_platform::tray::TrayIcon>>> = const { RefCell::new(None) };
    static LIVE_INSTANCE_GUARD: RefCell<Option<Arc<snagsoup_win_platform::single_instance::SingleInstanceGuard>>> = const { RefCell::new(None) };
    static SINGLE_INSTANCE_POLLING: Cell<bool> = const { Cell::new(false) };
    static APP_EXITING: Cell<bool> = const { Cell::new(false) };
}

const HK_ID_GLOBAL_CAPTURE: i32 = 9001;
const HK_ID_REPEAT_CAPTURE: i32 = 9002;
const HK_ID_SHOW_HIDE_APP: i32 = 9003;
const HK_ID_VIDEO_REC_PAUSE: i32 = 9004;
const HK_ID_VIDEO_STOP: i32 = 9005;

/// Register the session-wide GUID [`snagsoup_win_platform::single_instance::SingleInstanceGuard`]
/// with the GPUI runtime so incoming secondary-launch commands (`WM_COPYDATA`) restore/focus
/// the running window cleanly.
pub fn register_single_instance_guard(
    guard: Arc<snagsoup_win_platform::single_instance::SingleInstanceGuard>,
) {
    LIVE_INSTANCE_GUARD.with(|cell| {
        *cell.borrow_mut() = Some(guard);
    });
}

fn ensure_single_instance_ipc_listener(cx: &mut App) {
    if SINGLE_INSTANCE_POLLING.with(|c| c.replace(true)) {
        return;
    }
    let maybe_guard = LIVE_INSTANCE_GUARD.with(|cell| cell.borrow().clone());
    let Some(poll_guard) = maybe_guard else {
        SINGLE_INSTANCE_POLLING.with(|c| c.set(false));
        return;
    };

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            if APP_EXITING.with(|c| c.get()) {
                break;
            }
            while let Some(cmd) = poll_guard.try_recv() {
                let _ = cx.update(|cx| {
                    use snagsoup_win_platform::single_instance::InstanceCommand;
                    match cmd {
                        InstanceCommand::ActivateCaptureWindow | InstanceCommand::ActivateOneClick => {
                            open_or_focus_capture_window(cx);
                        }
                        InstanceCommand::OpenEditor(file) => {
                            open_or_focus_editor_window(file, cx);
                        }
                        InstanceCommand::CaptureNow => {
                            trigger_active_or_default_capture(cx);
                        }
                    }
                });
            }
        }
    })
    .detach();
}

/// Cleanly exit SnagSoup, removing the System Tray icon and bypassing `close_to_tray` vetoes.
pub fn quit_snagsoup_application(cx: &mut App) {
    APP_EXITING.with(|c| c.set(true));
    LIVE_TRAY_ICON.with(|cell| {
        *cell.borrow_mut() = None;
    });
    LIVE_INSTANCE_GUARD.with(|cell| {
        *cell.borrow_mut() = None;
    });
    cx.quit();
}

/// Ensure the Emerald S System Tray icon is active and dispatching single-click / menu actions.
fn ensure_live_tray_icon(cx: &mut App) {
    let mut newly_created: Option<Arc<snagsoup_win_platform::tray::TrayIcon>> = None;
    LIVE_TRAY_ICON.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            if let Ok(tray) = snagsoup_win_platform::tray::TrayIcon::new(APP_TITLE) {
                let arc = Arc::new(tray);
                *borrow = Some(Arc::clone(&arc));
                newly_created = Some(arc);
            }
        }
    });

    if let Some(poll_tray) = newly_created {
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if APP_EXITING.with(|c| c.get()) {
                    break;
                }
                while let Some(cmd) = poll_tray.try_recv() {
                    let _ = cx.update(|cx| {
                        use snagsoup_win_platform::tray::TrayCommand;
                        match cmd {
                            TrayCommand::OpenCaptureWindow | TrayCommand::Presets => {
                                open_or_focus_capture_window(cx);
                            }
                            TrayCommand::Capture => {
                                trigger_active_or_default_capture(cx);
                            }
                            TrayCommand::OpenEditor => {
                                open_or_focus_editor_window(None, cx);
                            }
                            TrayCommand::Preferences => {
                                open_or_focus_preferences_window(PreferencesTab::General, cx);
                            }
                            TrayCommand::Quit => {
                                quit_snagsoup_application(cx);
                            }
                        }
                    });
                }
            }
        })
        .detach();
    }
}

fn trigger_active_or_default_capture(cx: &mut App) {
    let preset_and_hwnd = ACTIVE_CAPTURE_WINDOW.with(|cell| {
        cell.borrow().as_ref().and_then(|h| {
            h.update(cx, |view, window, _| {
                let hwnd = snagsoup_ui_huds::extract_hwnd(window)
                    .filter(|&hw| snagsoup_ui_huds::is_window_visible_hwnd(hw));
                (view.state.active.clone(), hwnd)
            })
            .ok()
        })
    });
    if let Some((preset, cap_hwnd)) = preset_and_hwnd {
        trigger_capture_preset_from_window(&preset, cap_hwnd, cx);
    } else {
        trigger_capture_preset(&CapturePreset::default(), cx);
    }
}

pub(crate) fn record_last_capture_rect(rect: RectF) {
    if rect.width >= 2.0 && rect.height >= 2.0 {
        LAST_CAPTURE_RECT.with(|cell| *cell.borrow_mut() = Some(rect));
    }
}

fn trigger_repeat_last_capture(cx: &mut App) {
    let mut preset = ACTIVE_CAPTURE_WINDOW
        .with(|cell| {
            cell.borrow().as_ref().and_then(|h| {
                h.update(cx, |view, _, _| view.state.active.clone()).ok()
            })
        })
        .unwrap_or_default();

    let prefs = CapturePreferences::load_or_default();
    let mut hidden_hwnds = Vec::new();
    if prefs.hide_snagsoup_before_capturing {
        if let Some(cap_hwnd) = ACTIVE_CAPTURE_WINDOW.with(|cell| {
            cell.borrow().as_ref().and_then(|h| {
                h.update(cx, |_, window, _| snagsoup_ui_huds::extract_hwnd(window))
                    .ok()
                    .flatten()
            })
        }) {
            if snagsoup_ui_huds::hide_window_hwnd(cap_hwnd) {
                hidden_hwnds.push(cap_hwnd);
            }
        }
        if let Some(ed_hwnd) = ACTIVE_EDITOR_WINDOW.with(|cell| {
            cell.borrow().as_ref().and_then(|h| {
                h.update(cx, |_, window, _| snagsoup_ui_huds::extract_hwnd(window))
                    .ok()
                    .flatten()
            })
        }) {
            if !hidden_hwnds.contains(&ed_hwnd) && snagsoup_ui_huds::hide_window_hwnd(ed_hwnd) {
                hidden_hwnds.push(ed_hwnd);
            }
        }
    }

    let mut freezer = ScreenFreezer::new();
    if let Ok(frozen) = freezer.freeze_virtual_desktop() {
        let rect = LAST_CAPTURE_RECT
            .with(|cell| *cell.borrow())
            .filter(|r| r.width >= 2.0 && r.height >= 2.0)
            .unwrap_or_else(|| {
                RectF::new(
                    0.0,
                    0.0,
                    frozen.frame.width as f64,
                    frozen.frame.height as f64,
                )
            });
        let (bmp, cursor_obj) = crop_frozen_screen(&frozen, rect, preset.capture_cursor);
        let copy_clip = preset.copy_to_clipboard || prefs.auto_copy_to_clipboard;
        if copy_clip {
            let img = snagsoup_win_platform::clipboard::RgbaImage {
                width: bmp.width(),
                height: bmp.height(),
                data: bmp.to_rgba(),
            };
            let _ = snagsoup_win_platform::clipboard::write_image_to_clipboard(&img);
        }
        if preset.preview_in_editor {
            open_editor_with_capture(bmp, cursor_obj, cx);
        } else {
            let mut silent_state = EditorShellState::default();
            silent_state.load_captured_bitmap(bmp, cursor_obj);
            for &hwnd in &hidden_hwnds {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
        }
    } else {
        preset.selection_mode = SelectionMode::FullScreen;
        run_preset_immediate(&preset, hidden_hwnds, cx);
    }
}

fn toggle_show_hide_app_window(cx: &mut App) {
    let toggled_existing = ACTIVE_CAPTURE_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            if let Ok(handled) = handle.update(cx, |_, window, cx| {
                if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                    if snagsoup_ui_huds::is_window_visible_hwnd(hwnd) {
                        ensure_live_tray_icon(cx);
                        let _ = snagsoup_ui_huds::hide_window_hwnd(hwnd);
                        return true;
                    }
                    let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    window.activate_window();
                    cx.notify();
                    return true;
                }
                false
            }) {
                return handled;
            }
        }
        false
    });
    if !toggled_existing {
        open_or_focus_capture_window(cx);
    }
}

fn sync_all_live_hotkeys(prefs: &CapturePreferences, cx: &mut App) {
    let mut newly_created_mgr: Option<Arc<snagsoup_win_platform::hotkey::HotkeyManager>> = None;
    let mgr_opt = LIVE_HOTKEY_MGR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            if let Ok(m) = snagsoup_win_platform::hotkey::HotkeyManager::new() {
                let _ = m.enable_ll_hook_fallback(true);
                let arc = Arc::new(m);
                *borrow = Some(Arc::clone(&arc));
                newly_created_mgr = Some(arc);
            }
        }
        borrow.clone()
    });

    if let Some(ref mgr) = mgr_opt {
        let _ = mgr.enable_ll_hook_fallback(true);
        let slots: &[(i32, &str)] = &[
            (HK_ID_GLOBAL_CAPTURE, &prefs.global_capture_hotkey),
            (HK_ID_REPEAT_CAPTURE, &prefs.repeat_last_capture_hotkey),
            (HK_ID_SHOW_HIDE_APP, &prefs.show_hide_hotkey),
            (HK_ID_VIDEO_REC_PAUSE, &prefs.video_record_pause_hotkey),
            (HK_ID_VIDEO_STOP, &prefs.video_stop_hotkey),
        ];
        for &(id, key_str) in slots {
            let _ = mgr.unregister(id);
            if let Some(spec) =
                snagsoup_win_platform::hotkey::HotkeySpec::from_display_str(id, key_str)
            {
                let _ = mgr.register(spec);
            }
        }
    }

    if let Some(poll_mgr) = newly_created_mgr {
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(35))
                    .await;
                if APP_EXITING.with(|c| c.get()) {
                    break;
                }
                while let Some(ev) = poll_mgr.try_recv_event() {
                    let _ = cx.update(|cx| match ev.id {
                        HK_ID_GLOBAL_CAPTURE => {
                            trigger_active_or_default_capture(cx);
                        }
                        HK_ID_REPEAT_CAPTURE => {
                            trigger_repeat_last_capture(cx);
                        }
                        HK_ID_SHOW_HIDE_APP => {
                            toggle_show_hide_app_window(cx);
                        }
                        HK_ID_VIDEO_REC_PAUSE => {
                            let toggled = ACTIVE_RECORDING_HUD.with(|cell| {
                                cell.borrow().as_ref().is_some_and(|h| {
                                    h.update(cx, |view, _, cx| {
                                        if view.recorder.is_some() {
                                            view.toggle_pause();
                                            cx.notify();
                                            true
                                        } else {
                                            false
                                        }
                                    })
                                    .unwrap_or(false)
                                })
                            });
                            if !toggled {
                                let mut vp = CapturePreset::default();
                                vp.kind = CaptureKind::Video;
                                vp.selection_mode = SelectionMode::Region;
                                trigger_capture_preset(&vp, cx);
                            }
                        }
                        HK_ID_VIDEO_STOP => {
                            ACTIVE_RECORDING_HUD.with(|cell| {
                                if let Some(h) = *cell.borrow() {
                                    let _ = h.update(cx, |view, window, cx| {
                                        view.finalize_and_open_editor(window, cx);
                                    });
                                }
                            });
                            ACTIVE_RECORDING_HUD.with(|cell| *cell.borrow_mut() = None);
                        }
                        _ => {}
                    });
                }
            }
        })
        .detach();
    }
}

/// Handle clicking the `✕` Close button on the Capture App Window:
/// - When `Keep running in System Tray on close` is enabled: hides the `HWND` completely
///   via `SW_HIDE` (removing it from the Windows Taskbar & Alt+Tab!) while keeping the
///   System Tray icon and global hotkeys alive.
/// - When disabled: closes the Capture Window (and exits if no Editor window is open).
pub fn handle_capture_window_close_action(window: &mut Window, cx: &mut App) {
    let close_to_tray = CapturePreferences::load_or_default().close_to_tray;
    if close_to_tray && !APP_EXITING.with(|c| c.get()) {
        ensure_live_tray_icon(cx);
        if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
            let _ = snagsoup_ui_huds::hide_window_hwnd(hwnd);
        }
        let _ = snagsoup_win_platform::memory::reclaim_process_memory(None);
    } else {
        ACTIVE_CAPTURE_WINDOW.with(|cell| *cell.borrow_mut() = None);
        window.remove_window();
        let editor_alive = ACTIVE_EDITOR_WINDOW.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|h| h.update(cx, |_, _, _| ()).is_ok())
        });
        if !editor_alive {
            quit_snagsoup_application(cx);
        }
    }
}

/// Open or focus the standalone redesigned `SnagSoup Preferences` dialog (`General | Capture | Hotkeys`).
pub fn open_or_focus_preferences_window(tab: PreferencesTab, cx: &mut App) {
    let existing_ok = ACTIVE_PREFS_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            if handle
                .update(cx, |view, window, cx| {
                    view.tab = tab;
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
            {
                return true;
            }
        }
        false
    });
    if existing_ok {
        cx.activate(true);
        return;
    }

    let bounds = Bounds::centered(
        None,
        size(px(PREFS_DIALOG_WIDTH), px(PREFS_DIALOG_HEIGHT)),
        cx,
    );
    if let Ok(handle) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("SnagSoup Preferences")),
                appears_transparent: false,
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        move |window, cx| {
            window.activate_window();
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
            cx.new(move |_| {
                CapturePreferencesView::new(tab).with_on_saved(|saved_prefs, cx| {
                    ACTIVE_CAPTURE_WINDOW.with(|cell| {
                        if let Some(cap_handle) = *cell.borrow() {
                            let _ = cap_handle.update(cx, |view, _, cx| {
                                view.state.apply_updated_preferences(saved_prefs.clone());
                                cx.notify();
                            });
                        }
                    });
                    sync_all_live_hotkeys(saved_prefs, cx);
                    if saved_prefs.close_to_tray {
                        ensure_live_tray_icon(cx);
                    }
                })
            })
        },
    ) {
        ACTIVE_PREFS_WINDOW.with(|cell| *cell.borrow_mut() = Some(handle));
    }
    cx.activate(true);
}

/// Open or focus the standalone `About SnagSoup` dialog (`Help > About`).
pub fn open_or_focus_about_window(cx: &mut App) {
    let existing_ok = ACTIVE_ABOUT_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            if handle
                .update(cx, |_, window, cx| {
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
            {
                return true;
            }
        }
        false
    });
    if existing_ok {
        cx.activate(true);
        return;
    }

    let bounds = Bounds::centered(
        None,
        size(px(ABOUT_DIALOG_WIDTH), px(ABOUT_DIALOG_HEIGHT)),
        cx,
    );
    if let Ok(handle) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("About SnagSoup")),
                appears_transparent: false,
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| {
            window.activate_window();
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
            cx.new(|_| AboutDialogView::new())
        },
    ) {
        ACTIVE_ABOUT_WINDOW.with(|cell| *cell.borrow_mut() = Some(handle));
    }
    cx.activate(true);
}

/// Zero-I/O embedded asset provider serving all vector SVG icons from static memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct SnagAssets;

impl AssetSource for SnagAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(SnagIcon::lookup_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = if path.is_empty() { "" } else { path };
        Ok(SnagIcon::ALL
            .iter()
            .map(|icon| icon.asset_path())
            .filter(|p| p.starts_with(prefix))
            .map(SharedString::from)
            .collect())
    }
}

/// Construct a GPUI `Application` backed by Windows DirectX 11 (`gpui_windows`)
/// with [`SnagAssets`] pre-registered.
pub fn create_application() -> Application {
    let platform = Rc::new(
        gpui_windows::WindowsPlatform::new(false)
            .expect("failed to initialize GPUI Windows DX11 platform"),
    );
    Application::with_platform(platform).with_assets(SnagAssets)
}

/// Open or focus the singleton GPUI Editor / Library Window, optionally loading `initial_file`.
pub fn open_or_focus_editor_window(initial_file: Option<PathBuf>, cx: &mut App) {
    let existing_ok = ACTIVE_EDITOR_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            let file_clone = initial_file.clone();
            if handle
                .update(cx, |view, window, cx| {
                    if let Some(ref path) = file_clone {
                        let is_mp4 = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
                        let _ = if is_mp4 {
                            view.state.open_video_file(path)
                        } else {
                            view.state.open_snagx(path)
                        };
                        view.state.workspace_view = WorkspaceView::Editor;
                    }
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
            {
                return true;
            }
        }
        false
    });
    if existing_ok {
        cx.activate(true);
        return;
    }

    let bounds = Bounds::centered(None, size(px(1240.0), px(780.0)), cx);
    if let Ok(handle) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(default_titlebar_options()),
            focus: true,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            window.activate_window();
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
            cx.new(move |_| EditorShellView::new(initial_file.clone()))
        },
    ) {
        ACTIVE_EDITOR_WINDOW.with(|cell| *cell.borrow_mut() = Some(handle));
    }
    cx.activate(true);
}

/// Open or focus the singleton Snagit-style compact Capture App Window (`520 × 268`, solid blue header).
pub fn open_or_focus_capture_window(cx: &mut App) {
    let existing_ok = ACTIVE_CAPTURE_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            if handle
                .update(cx, |_, window, cx| {
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
            {
                return true;
            }
        }
        false
    });
    if existing_ok {
        cx.activate(true);
        return;
    }

    let prefs = CapturePreferences::load_or_default();
    sync_all_live_hotkeys(&prefs, cx);
    ensure_live_tray_icon(cx);

    let bounds = Bounds::centered(
        None,
        size(px(CAPTURE_WIN_WIDTH), px(CAPTURE_WIN_COMPACT_HEIGHT)),
        cx,
    );
    if let Ok(handle) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(APP_TITLE)),
                appears_transparent: true,
                ..Default::default()
            }),
            focus: true,
            show: true,
            is_resizable: false,
            ..Default::default()
        },
        |window, cx| {
            window.activate_window();
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
            window.on_window_should_close(cx, |window, cx| {
                if APP_EXITING.with(|c| c.get()) {
                    return true;
                }
                let close_to_tray = CapturePreferences::load_or_default().close_to_tray;
                if close_to_tray {
                    ensure_live_tray_icon(cx);
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::hide_window_hwnd(hwnd);
                    }
                    let _ = snagsoup_win_platform::memory::reclaim_process_memory(None);
                    false
                } else {
                    ACTIVE_CAPTURE_WINDOW.with(|cell| *cell.borrow_mut() = None);
                    let editor_alive = ACTIVE_EDITOR_WINDOW.with(|cell| {
                        cell.borrow()
                            .as_ref()
                            .is_some_and(|h| h.update(cx, |_, _, _| ()).is_ok())
                    });
                    if !editor_alive {
                        quit_snagsoup_application(cx);
                    }
                    true
                }
            });
            cx.new(|_| {
                CaptureWindowView::new()
                    .with_on_capture(|preset, cap_win, cx| {
                        let cap_hwnd = snagsoup_ui_huds::extract_hwnd(cap_win);
                        trigger_capture_preset_from_window(preset, cap_hwnd, cx);
                    })
                    .with_on_open_editor(|_cap_win, cx| {
                        open_or_focus_editor_window(None, cx);
                    })
            })
        },
    ) {
        ACTIVE_CAPTURE_WINDOW.with(|cell| *cell.borrow_mut() = Some(handle));
    }
    cx.activate(true);
}

/// Open (or reuse) the Editor window initialized with a freshly captured bitmap & optional cursor object,
/// immediately raising and focusing it to the foreground.
pub fn open_editor_with_capture(
    bmp: TiledBitmap,
    cursor_obj: Option<CaptureObject>,
    cx: &mut App,
) {
    let mut bmp_slot = Some((bmp, cursor_obj));
    let updated_existing = ACTIVE_EDITOR_WINDOW.with(|cell| {
        if let Some(handle) = *cell.borrow() {
            if handle
                .update(cx, |view, window, cx| {
                    if let Some((b, c)) = bmp_slot.take() {
                        view.state.load_captured_bitmap(b, c);
                    }
                    if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                        let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                    }
                    window.activate_window();
                    cx.notify();
                })
                .is_ok()
            {
                return true;
            }
        }
        false
    });
    if updated_existing {
        cx.activate(true);
        return;
    }

    let Some((bmp, cursor_obj)) = bmp_slot else {
        return;
    };
    let bounds = Bounds::centered(None, size(px(1240.0), px(780.0)), cx);
    if let Ok(handle) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(default_titlebar_options()),
            focus: true,
            show: true,
            ..Default::default()
        },
        move |window, cx| {
            window.activate_window();
            if let Some(hwnd) = snagsoup_ui_huds::extract_hwnd(window) {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
            cx.new(move |_| {
                let mut view = EditorShellView::new(None);
                view.state.load_captured_bitmap(bmp, cursor_obj);
                view
            })
        },
    ) {
        ACTIVE_EDITOR_WINDOW.with(|cell| *cell.borrow_mut() = Some(handle));
    }
    cx.activate(true);
}

/// Open (or reuse) the Editor window initialized with a recorded `.mp4` video in Video Player mode,
/// immediately raising and focusing it to the foreground.
pub fn open_editor_with_video(mp4_path: PathBuf, cx: &mut App) {
    open_or_focus_editor_window(Some(mp4_path), cx);
}

/// Launch the Floating Mini Recording HUD and start live screen recording for `req`.
pub fn start_live_video_recording_flow(req: VideoRecordRequest, cx: &mut App) {
    let hud_handle = open_live_recording_hud(req, cx);
    ACTIVE_RECORDING_HUD.with(|cell| *cell.borrow_mut() = hud_handle);
}

/// Execute a capture preset: fullscreen immediate capture or interactive `SelectionOverlayView`,
/// optionally copying to Clipboard, dispatching Share destination, and opening the Editor window.
pub fn trigger_capture_preset(preset: &CapturePreset, cx: &mut App) {
    trigger_capture_preset_from_window(preset, None, cx);
}

/// Execute a capture preset while hiding open SnagSoup windows beforehand (both Capture App
/// and Editor/Library if visible, when `hide_snagsoup_before_capturing` is enabled) so SnagSoup
/// never photographs its own windows, restoring appropriately upon completion or cancel.
pub fn trigger_capture_preset_from_window(
    preset: &CapturePreset,
    caller_hwnd: Option<isize>,
    cx: &mut App,
) {
    let prefs = CapturePreferences::load_or_default();
    let mut hidden_hwnds: Vec<isize> = Vec::new();
    if prefs.hide_snagsoup_before_capturing {
        if let Some(hwnd) = caller_hwnd {
            if snagsoup_ui_huds::hide_window_hwnd(hwnd) {
                hidden_hwnds.push(hwnd);
            }
        }
        let ed_hwnd_opt = ACTIVE_EDITOR_WINDOW.with(|cell| {
            cell.borrow().as_ref().and_then(|h| {
                h.update(cx, |_, window, _| snagsoup_ui_huds::extract_hwnd(window))
                    .ok()
                    .flatten()
            })
        });
        if let Some(ed_hwnd) = ed_hwnd_opt {
            if !hidden_hwnds.contains(&ed_hwnd) && snagsoup_ui_huds::hide_window_hwnd(ed_hwnd) {
                hidden_hwnds.push(ed_hwnd);
            }
        }
    }

    if preset.timed_capture.enabled && preset.timed_capture.delay_seconds > 0 {
        let secs = preset.timed_capture.delay_seconds as u64;
        let mut immediate = preset.clone();
        immediate.timed_capture.enabled = false;
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(secs))
                .await;
            let _ = cx.update(|cx| {
                run_preset_immediate(&immediate, hidden_hwnds.clone(), cx);
            });
        })
        .detach();
    } else {
        run_preset_immediate(preset, hidden_hwnds, cx);
    }
}

fn run_preset_immediate(preset: &CapturePreset, hidden_hwnds: Vec<isize>, cx: &mut App) {
    let prefs = CapturePreferences::load_or_default();
    let copy_clip = preset.copy_to_clipboard || prefs.auto_copy_to_clipboard;
    let open_editor = preset.preview_in_editor;
    let include_cursor = preset.capture_cursor;
    let start_in_video_mode = preset.kind == CaptureKind::Video;
    let share_dest = preset.share_destination.clone();
    let hidden_for_deliver = hidden_hwnds.clone();
    let hidden_for_err = hidden_hwnds.clone();

    let deliver = move |bmp: TiledBitmap, cursor_obj: Option<CaptureObject>, cx: &mut App| {
        if copy_clip || matches!(share_dest.as_deref(), Some("Clipboard")) {
            let img = snagsoup_win_platform::clipboard::RgbaImage {
                width: bmp.width(),
                height: bmp.height(),
                data: bmp.to_rgba(),
            };
            let _ = snagsoup_win_platform::clipboard::write_image_to_clipboard(&img);
        }
        if let Some(ref dest) = share_dest {
            let target_opt = match dest.as_str() {
                "File (Auto-save)" => Some(snagsoup_ui_library::ShareTargetKind::FileAutoSave),
                "Word" => Some(snagsoup_ui_library::ShareTargetKind::OfficeWord),
                "PowerPoint" => Some(snagsoup_ui_library::ShareTargetKind::OfficePowerPoint),
                "Excel" => Some(snagsoup_ui_library::ShareTargetKind::OfficeExcel),
                "Email" => Some(snagsoup_ui_library::ShareTargetKind::EmailMapi),
                _ => None,
            };
            if let Some(target) = target_opt {
                let mut sm = snagsoup_ui_library::ShareMenuView::new(capture_storage_dir(), None);
                let _ = sm.execute_share(
                    target,
                    bmp.width(),
                    bmp.height(),
                    &bmp.to_rgba(),
                    "SnagSoup_Capture",
                );
            }
        }
        if open_editor {
            open_editor_with_capture(bmp, cursor_obj, cx);
        } else {
            // Persist silently into Library even when `Preview in Editor` is OFF, then restore Capture Window
            let mut silent_state = EditorShellState::default();
            silent_state.load_captured_bitmap(bmp, cursor_obj);
            if let Some(&first_hwnd) = hidden_for_deliver.first() {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(first_hwnd);
            }
        }
    };

    if preset.selection_mode == SelectionMode::FullScreen && !start_in_video_mode {
        let mut freezer = ScreenFreezer::new();
        if let Ok(frozen) = freezer.freeze_virtual_desktop() {
            let rect = RectF::new(
                0.0,
                0.0,
                frozen.frame.width as f64,
                frozen.frame.height as f64,
            );
            let (bmp, cursor_obj) = crop_frozen_screen(&frozen, rect, include_cursor);
            deliver(bmp, cursor_obj, cx);
        } else {
            for &hwnd in &hidden_for_err {
                let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
            }
        }
    } else {
        open_selection_overlay_window_full(
            cx,
            include_cursor,
            start_in_video_mode,
            deliver,
            |req, cx| {
                start_live_video_recording_flow(req, cx);
            },
            move |_cx| {
                for &hwnd in &hidden_hwnds {
                    let _ = snagsoup_ui_huds::restore_and_focus_hwnd(hwnd);
                }
            },
        );
    }
}

/// Launch the GPUI Editor Window Shell (`W3-05`..`W3-16`), optionally opening a `.snagx` file.
pub fn run_editor_window(initial_mode: ThemeMode, initial_file: Option<PathBuf>) {
    let _ = std::thread::Builder::new()
        .name("snagsoup-wgc-prewarm".into())
        .spawn(|| snagsoup_capture::prewarm_wgc_monitor_sessions());
    create_application().run(move |cx: &mut App| {
        let prefs = CapturePreferences::load_or_default();
        let mode = if prefs.dark_theme {
            ThemeMode::Dark
        } else {
            initial_mode
        };
        cx.set_global(SnagTheme::for_mode(mode));
        ensure_single_instance_ipc_listener(cx);
        ensure_live_tray_icon(cx);
        sync_all_live_hotkeys(&prefs, cx);
        open_or_focus_editor_window(initial_file.clone(), cx);
    });
}

/// Launch the Snagit-style GPUI Capture App Window (`W3-02`) wired to live screen capture & Editor handoff.
pub fn run_capture_window(initial_mode: ThemeMode) {
    let _ = std::thread::Builder::new()
        .name("snagsoup-wgc-prewarm".into())
        .spawn(|| snagsoup_capture::prewarm_wgc_monitor_sessions());
    create_application().run(move |cx: &mut App| {
        let prefs = CapturePreferences::load_or_default();
        let mode = if prefs.dark_theme {
            ThemeMode::Dark
        } else {
            initial_mode
        };
        cx.set_global(SnagTheme::for_mode(mode));
        ensure_single_instance_ipc_listener(cx);
        open_or_focus_capture_window(cx);
    });
}

/// Launch the OneClick top-edge floating capture widget (`W3-02`) wired to live capture & Editor.
pub fn run_oneclick_window(initial_mode: ThemeMode) {
    let _ = std::thread::Builder::new()
        .name("snagsoup-wgc-prewarm".into())
        .spawn(|| snagsoup_capture::prewarm_wgc_monitor_sessions());
    create_application().run(move |cx: &mut App| {
        let prefs = CapturePreferences::load_or_default();
        cx.set_global(SnagTheme::for_mode(initial_mode));
        ensure_single_instance_ipc_listener(cx);
        ensure_live_tray_icon(cx);
        sync_all_live_hotkeys(&prefs, cx);
        let bounds = Bounds::centered(None, size(px(260.0), px(56.0)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(default_titlebar_options()),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| {
                    OneClickWidget::new()
                        .with_on_capture(|window, cx| {
                            let hwnd = snagsoup_ui_huds::extract_hwnd(window);
                            trigger_capture_preset_from_window(&CapturePreset::default(), hwnd, cx);
                        })
                        .with_on_open_editor(|_window, cx| {
                            open_or_focus_editor_window(None, cx);
                        })
                })
            },
        );
        cx.activate(true);
    });
}

/// Immediately freeze the desktop, open the fullscreen Selection Overlay, and open the
/// captured image in the GPUI Editor upon completion (`--capture-now` from `--tray` hotkey).
pub fn run_instant_capture_flow(initial_mode: ThemeMode) {
    create_application().run(move |cx: &mut App| {
        let prefs = CapturePreferences::load_or_default();
        let mode = if prefs.dark_theme {
            ThemeMode::Dark
        } else {
            initial_mode
        };
        cx.set_global(SnagTheme::for_mode(mode));
        ensure_single_instance_ipc_listener(cx);
        ensure_live_tray_icon(cx);
        sync_all_live_hotkeys(&prefs, cx);
        trigger_capture_preset(&CapturePreset::default(), cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_bgra_render_image_swaps_red_and_blue() {
        // Pure Windows 11 blue pixel in RGBA8: R=0x10, G=0x70, B=0xE8, A=0xFF
        let rgba = [0x10u8, 0x70, 0xE8, 0xFF, 0xAA, 0xBB, 0xCC, 0x80];
        let img = rgba_to_bgra_render_image(2, 1, &rgba).expect("valid 2x1 RenderImage");
        let bytes = img.as_bytes(0).expect("frame 0 bytes");
        assert_eq!(bytes, &[0xE8, 0x70, 0x10, 0xFF, 0xCC, 0xBB, 0xAA, 0x80]);

        // Rejects mismatched lengths or zero dimensions
        assert!(rgba_to_bgra_render_image(0, 1, &[]).is_none());
        assert!(rgba_to_bgra_render_image(2, 1, &[0; 4]).is_none());
    }

    #[test]
    fn app_title_and_titlebar_options_format() {
        assert_eq!(APP_VERSION_FULL, "1.3.0.0");
        assert_eq!(APP_TITLE, "SnagSoup v1.3.0.0");
        let tb = default_titlebar_options();
        assert!(!tb.appears_transparent);
        assert_eq!(tb.title.as_deref(), Some("SnagSoup v1.3.0.0"));
    }
}
