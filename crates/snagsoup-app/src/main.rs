//! SnagSoup single-binary entry point (`snagsoup.exe`).
//!
//! Uses `windows_subsystem = "windows"` so double-clicking `dist\snagsoup.exe` in Explorer
//! opens no console window, while CLI flags (`--check-mem`, `--help`, `--version`) attach
//! cleanly to the parent console when invoked from a terminal.
//!
//! Supported CLI modes:
//! - (default): Opens the GPUI Editor window
//! - `--tray`: Starts lightweight Tier 0 Win32 tray + hotkey daemon (`< 14 MB` RAM) + crash recovery check
//! - `--editor [path.snagx]`: Opens the GPUI Editor shell window directly
//! - `--capture`: Opens the 3-tab GPUI Capture window
//! - `--oneclick`: Opens the OneClick top-edge floating capture widget
//! - `--scroll-helper <hwnd> <dy>`: Elevated scroll-wheel pulse helper for Admin windows
//! - `--check-mem`: Initializes Tier 0 daemon, reports Working Set & Private Usage via `K32GetProcessMemoryInfo`, and exits

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod daemon;
mod memory;

use std::path::PathBuf;

use snagsoup_ui::theme::ThemeMode;

use crate::daemon::AppCoordinator;

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchMode {
    Tray,
    Editor(Option<PathBuf>),
    Capture,
    CaptureNow,
    OneClick,
    ScrollHelper { hwnd: isize, dy: i32 },
    CheckMem,
    Version,
    Help,
}

fn attach_parent_console_if_available() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: `AttachConsole` is safe to call on Windows; returns 0 if already attached or redirected.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn parse_args() -> (LaunchMode, ThemeMode) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return (LaunchMode::Capture, ThemeMode::Light);
    }

    let mut mode: Option<LaunchMode> = None;
    let mut theme = ThemeMode::Light;
    let mut pending_file: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tray" => mode = Some(LaunchMode::Tray),
            "--editor" => mode = Some(LaunchMode::Editor(None)),
            "--capture" => mode = Some(LaunchMode::Capture),
            "--capture-now" => mode = Some(LaunchMode::CaptureNow),
            "--oneclick" => mode = Some(LaunchMode::OneClick),
            "--check-mem" => mode = Some(LaunchMode::CheckMem),
            "--dark" => theme = ThemeMode::Dark,
            "--light" => theme = ThemeMode::Light,
            "-V" | "--version" => mode = Some(LaunchMode::Version),
            "-h" | "--help" => mode = Some(LaunchMode::Help),
            "--scroll-helper" => {
                let hwnd = args.get(i + 1).and_then(|s| s.parse::<isize>().ok()).unwrap_or(0);
                let dy = args.get(i + 2).and_then(|s| s.parse::<i32>().ok()).unwrap_or(-120);
                mode = Some(LaunchMode::ScrollHelper { hwnd, dy });
                i += 2;
            }
            other if !other.starts_with('-') => {
                pending_file = Some(PathBuf::from(other));
            }
            other => {
                attach_parent_console_if_available();
                eprintln!("Unknown argument: {other}");
                mode = Some(LaunchMode::Help);
            }
        }
        i += 1;
    }

    let resolved_mode = match mode {
        Some(LaunchMode::Editor(_)) => LaunchMode::Editor(pending_file),
        Some(m) => m,
        None => {
            if pending_file.is_some() {
                LaunchMode::Editor(pending_file)
            } else {
                LaunchMode::Capture
            }
        }
    };

    (resolved_mode, theme)
}

fn print_help() {
    println!("{} — High-Performance Screen Capture & Editor", snagsoup_ui::APP_TITLE);
    println!("Usage: snagsoup [OPTIONS] [FILE.snagx]");
    println!("  (default)                      Launch SnagSoup Capture App window");
    println!("  --tray                         Run Tier 0 lightweight Win32 tray + hotkey daemon (< 14 MB)");
    println!("  --editor [FILE.snagx]          Launch GPUI Editor window");
    println!("  --capture                      Launch 3-tab GPUI Capture window");
    println!("  --oneclick                     Launch OneClick top-edge floating capture widget");
    println!("  --scroll-helper <hwnd> <dy>    Send single scroll pulse to elevated window and exit");
    println!("  --check-mem                    Measure Tier 0 idle memory via K32GetProcessMemoryInfo and exit");
    println!("  --light / --dark               Select Snagit 2022 Light (default) or modern Dark theme");
    println!("  --version / --help             Print version or usage");
}

fn main() {
    let (mode, theme) = parse_args();
    let coordinator = AppCoordinator::new(theme);

    match mode {
        LaunchMode::Help => {
            attach_parent_console_if_available();
            print_help();
        }
        LaunchMode::Version => {
            attach_parent_console_if_available();
            println!("{}", snagsoup_ui::APP_TITLE);
        }
        LaunchMode::CheckMem => {
            attach_parent_console_if_available();
            if let Err(err) = coordinator.run_check_mem() {
                eprintln!("Error: {err}");
                std::process::exit(1);
            }
        }
        LaunchMode::ScrollHelper { hwnd, dy } => {
            let _ = snagsoup_win_platform::scroll::send_scroll_wheel(dy, 0);
            let _ = hwnd;
        }
        LaunchMode::Editor(file) => {
            coordinator.run_editor(file);
        }
        LaunchMode::Capture => {
            coordinator.run_capture_window();
        }
        LaunchMode::CaptureNow => {
            coordinator.run_instant_capture();
        }
        LaunchMode::OneClick => {
            coordinator.run_oneclick_window();
        }
        LaunchMode::Tray => {
            if let Err(err) = coordinator.run_tray_daemon() {
                attach_parent_console_if_available();
                eprintln!("Error: {err}");
                std::process::exit(1);
            }
        }
    }
}
