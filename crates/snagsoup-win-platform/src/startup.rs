//! Windows "Run at Startup" integration via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use crate::PlatformError;
use std::ptr::{null, null_mut};

const HKEY_CURRENT_USER: isize = -2147483647i32 as isize; // 0x80000001
const KEY_READ: u32 = 0x20019;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_VALUE_NAME: &str = "SnagSoup";

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        hkey: isize,
        lpsubkey: *const u16,
        uloptions: u32,
        samdesired: u32,
        phkresult: *mut isize,
    ) -> u32;

    fn RegQueryValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        lpreserved: *const u32,
        lptype: *mut u32,
        lpdata: *mut u8,
        lpcbdata: *mut u32,
    ) -> u32;

    fn RegSetValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        reserved: u32,
        dwtype: u32,
        lpdata: *const u8,
        cbdata: u32,
    ) -> u32;

    fn RegDeleteValueW(hkey: isize, lpvaluename: *const u16) -> u32;

    fn RegCloseKey(hkey: isize) -> u32;
}

fn to_wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Check whether `value_name` is present in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub fn is_run_value_present(value_name: &str) -> bool {
    let subkey = to_wide_z(RUN_SUBKEY);
    let val = to_wide_z(value_name);
    let mut hkey: isize = 0;
    let open_rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if open_rc != ERROR_SUCCESS {
        return false;
    }
    let mut reg_type: u32 = 0;
    let mut data_len: u32 = 0;
    let query_rc = unsafe {
        RegQueryValueExW(
            hkey,
            val.as_ptr(),
            null(),
            &mut reg_type,
            null_mut(),
            &mut data_len,
        )
    };
    unsafe { RegCloseKey(hkey) };
    query_rc == ERROR_SUCCESS && data_len > 2
}

/// Write or remove a startup command under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub fn set_run_value_named(
    value_name: &str,
    enabled: bool,
    command_override: Option<&str>,
) -> Result<(), PlatformError> {
    let subkey = to_wide_z(RUN_SUBKEY);
    let val = to_wide_z(value_name);
    let mut hkey: isize = 0;
    let open_rc =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) };
    if open_rc != ERROR_SUCCESS {
        return Err(PlatformError::Win32 {
            api: "RegOpenKeyExW",
            code: open_rc,
        });
    }

    let result = if enabled {
        let cmd = match command_override {
            Some(c) => c.to_string(),
            None => {
                let exe = std::env::current_exe().unwrap_or_else(|_| "snagsoup.exe".into());
                format!("\"{}\" --tray", exe.display())
            }
        };
        let cmd_wide = to_wide_z(&cmd);
        let byte_len = (cmd_wide.len() * 2) as u32;
        let rc = unsafe {
            RegSetValueExW(
                hkey,
                val.as_ptr(),
                0,
                REG_SZ,
                cmd_wide.as_ptr() as *const u8,
                byte_len,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(PlatformError::Win32 {
                api: "RegSetValueExW",
                code: rc,
            })
        }
    } else {
        let rc = unsafe { RegDeleteValueW(hkey, val.as_ptr()) };
        if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(PlatformError::Win32 {
                api: "RegDeleteValueW",
                code: rc,
            })
        }
    };

    unsafe { RegCloseKey(hkey) };
    result
}

/// Check if SnagSoup is registered to launch automatically when Windows starts.
pub fn is_run_at_windows_startup_enabled() -> bool {
    is_run_value_present(APP_VALUE_NAME)
}

/// Enable or disable `"SnagSoup"` in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub fn set_run_at_windows_startup(enabled: bool) -> Result<(), PlatformError> {
    set_run_value_named(APP_VALUE_NAME, enabled, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_registry_roundtrip_on_isolated_key() {
        let test_name = "SnagSoup_UnitTest_TempCheck";
        let _ = set_run_value_named(test_name, false, None);
        assert!(!is_run_value_present(test_name));

        set_run_value_named(test_name, true, Some("\"C:\\Temp\\snagsoup.exe\" --tray"))
            .expect("set_run_value_named enable");
        assert!(is_run_value_present(test_name));

        set_run_value_named(test_name, false, None).expect("set_run_value_named disable");
        assert!(!is_run_value_present(test_name));
    }
}
