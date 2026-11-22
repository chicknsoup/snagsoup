//! Zero-CC dynamic loader for Windows' built-in `C:\Windows\System32\winsqlite3.dll` (v3.53+ with FTS5).

use crate::error::{ExportError, Result};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

pub const SQLITE_OK: c_int = 0;
pub const SQLITE_ROW: c_int = 100;
pub const SQLITE_DONE: c_int = 101;
const SQLITE_TRANSIENT: isize = -1;

type FnOpen = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int;
type FnClose = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnExec = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>,
    *mut c_void,
    *mut *mut c_char,
) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnPrepareV2 = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    c_int,
    *mut *mut c_void,
    *mut *const c_char,
) -> c_int;
type FnBindText = unsafe extern "C" fn(
    *mut c_void,
    c_int,
    *const c_char,
    c_int,
    isize,
) -> c_int;
type FnBindInt64 = unsafe extern "C" fn(*mut c_void, c_int, i64) -> c_int;
type FnStep = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnReset = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnClearBindings = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnFinalize = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnColumnInt64 = unsafe extern "C" fn(*mut c_void, c_int) -> i64;
type FnColumnText = unsafe extern "C" fn(*mut c_void, c_int) -> *const u8;
type FnColumnBytes = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type FnErrmsg = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type FnChanges = unsafe extern "C" fn(*mut c_void) -> c_int;

struct SqliteApi {
    open: FnOpen,
    close: FnClose,
    exec: FnExec,
    free: FnFree,
    prepare_v2: FnPrepareV2,
    bind_text: FnBindText,
    bind_int64: FnBindInt64,
    step: FnStep,
    reset: FnReset,
    clear_bindings: FnClearBindings,
    finalize: FnFinalize,
    column_int64: FnColumnInt64,
    column_text: FnColumnText,
    column_bytes: FnColumnBytes,
    errmsg: FnErrmsg,
    changes: FnChanges,
}

static API: OnceLock<std::result::Result<SqliteApi, String>> = OnceLock::new();

fn api() -> Result<&'static SqliteApi> {
    let res = API.get_or_init(|| unsafe {
        let hmod = LoadLibraryA(b"winsqlite3.dll\0".as_ptr());
        if hmod.is_null() {
            return Err("Failed to load Windows built-in winsqlite3.dll".to_string());
        }
        macro_rules! load_sym {
            ($name:literal, $ty:ty) => {{
                let p = GetProcAddress(hmod, concat!($name, "\0").as_ptr());
                match p {
                    Some(f) => std::mem::transmute::<_, $ty>(f),
                    None => return Err(format!("Missing symbol {} in winsqlite3.dll", $name)),
                }
            }};
        }
        Ok(SqliteApi {
            open: load_sym!("sqlite3_open", FnOpen),
            close: load_sym!("sqlite3_close", FnClose),
            exec: load_sym!("sqlite3_exec", FnExec),
            free: load_sym!("sqlite3_free", FnFree),
            prepare_v2: load_sym!("sqlite3_prepare_v2", FnPrepareV2),
            bind_text: load_sym!("sqlite3_bind_text", FnBindText),
            bind_int64: load_sym!("sqlite3_bind_int64", FnBindInt64),
            step: load_sym!("sqlite3_step", FnStep),
            reset: load_sym!("sqlite3_reset", FnReset),
            clear_bindings: load_sym!("sqlite3_clear_bindings", FnClearBindings),
            finalize: load_sym!("sqlite3_finalize", FnFinalize),
            column_int64: load_sym!("sqlite3_column_int64", FnColumnInt64),
            column_text: load_sym!("sqlite3_column_text", FnColumnText),
            column_bytes: load_sym!("sqlite3_column_bytes", FnColumnBytes),
            errmsg: load_sym!("sqlite3_errmsg", FnErrmsg),
            changes: load_sym!("sqlite3_changes", FnChanges),
        })
    });
    res.as_ref().map_err(|e| ExportError::Sqlite(e.clone()))
}

pub struct SqliteConn {
    db: *mut c_void,
}

// SAFETY: SQLite connection is only accessed through `&self` / `&mut self` with serialized ownership.
unsafe impl Send for SqliteConn {}

impl SqliteConn {
    pub fn open(path: &str) -> Result<Self> {
        let a = api()?;
        let c_path = CString::new(path).map_err(|e| ExportError::Sqlite(e.to_string()))?;
        let mut db: *mut c_void = ptr::null_mut();
        let rc = unsafe { (a.open)(c_path.as_ptr(), &mut db) };
        if rc != SQLITE_OK {
            let msg = if !db.is_null() {
                let s = unsafe { CStr::from_ptr((a.errmsg)(db)).to_string_lossy().into_owned() };
                unsafe { (a.close)(db) };
                s
            } else {
                format!("sqlite3_open failed with code {rc}")
            };
            return Err(ExportError::Sqlite(msg));
        }
        Ok(Self { db })
    }

    pub fn last_error(&self) -> String {
        let Ok(a) = api() else {
            return "winsqlite3 unavailable".into();
        };
        unsafe { CStr::from_ptr((a.errmsg)(self.db)).to_string_lossy().into_owned() }
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let a = api()?;
        let c_sql = CString::new(sql).map_err(|e| ExportError::Sqlite(e.to_string()))?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        let rc = unsafe { (a.exec)(self.db, c_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
        if rc != SQLITE_OK {
            let msg = if !errmsg.is_null() {
                let s = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
                unsafe { (a.free)(errmsg as *mut c_void) };
                s
            } else {
                self.last_error()
            };
            return Err(ExportError::Sqlite(msg));
        }
        Ok(())
    }

    pub fn prepare(&self, sql: &str) -> Result<SqliteStmt<'_>> {
        let a = api()?;
        let mut stmt: *mut c_void = ptr::null_mut();
        let rc = unsafe {
            (a.prepare_v2)(
                self.db,
                sql.as_ptr() as *const c_char,
                sql.len() as c_int,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(ExportError::Sqlite(self.last_error()));
        }
        Ok(SqliteStmt { conn: self, stmt })
    }

    pub fn changes(&self) -> usize {
        api().map(|a| unsafe { (a.changes)(self.db) as usize }).unwrap_or(0)
    }
}

impl Drop for SqliteConn {
    fn drop(&mut self) {
        if !self.db.is_null() {
            if let Ok(a) = api() {
                unsafe { (a.close)(self.db) };
            }
            self.db = ptr::null_mut();
        }
    }
}

pub struct SqliteStmt<'a> {
    conn: &'a SqliteConn,
    stmt: *mut c_void,
}

impl<'a> SqliteStmt<'a> {
    pub fn bind_text(&mut self, idx: i32, val: &str) -> Result<()> {
        let a = api()?;
        let rc = unsafe {
            (a.bind_text)(
                self.stmt,
                idx,
                val.as_ptr() as *const c_char,
                val.len() as c_int,
                SQLITE_TRANSIENT,
            )
        };
        if rc != SQLITE_OK {
            return Err(ExportError::Sqlite(self.conn.last_error()));
        }
        Ok(())
    }

    pub fn bind_int64(&mut self, idx: i32, val: i64) -> Result<()> {
        let a = api()?;
        let rc = unsafe { (a.bind_int64)(self.stmt, idx, val) };
        if rc != SQLITE_OK {
            return Err(ExportError::Sqlite(self.conn.last_error()));
        }
        Ok(())
    }

    pub fn step(&mut self) -> Result<c_int> {
        let a = api()?;
        let rc = unsafe { (a.step)(self.stmt) };
        if rc != SQLITE_ROW && rc != SQLITE_DONE && rc != SQLITE_OK {
            return Err(ExportError::Sqlite(self.conn.last_error()));
        }
        Ok(rc)
    }

    pub fn reset(&mut self) -> Result<()> {
        let a = api()?;
        unsafe {
            (a.reset)(self.stmt);
            (a.clear_bindings)(self.stmt);
        }
        Ok(())
    }

    pub fn col_i64(&self, col: i32) -> i64 {
        api().map(|a| unsafe { (a.column_int64)(self.stmt, col) }).unwrap_or(0)
    }

    pub fn col_text(&self, col: i32) -> String {
        let Ok(a) = api() else {
            return String::new();
        };
        unsafe {
            let ptr = (a.column_text)(self.stmt, col);
            let len = (a.column_bytes)(self.stmt, col) as usize;
            if ptr.is_null() || len == 0 {
                return String::new();
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            String::from_utf8_lossy(slice).into_owned()
        }
    }
}

impl Drop for SqliteStmt<'_> {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            if let Ok(a) = api() {
                unsafe { (a.finalize)(self.stmt) };
            }
            self.stmt = ptr::null_mut();
        }
    }
}
