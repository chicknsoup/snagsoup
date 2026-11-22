//! Local Microsoft Office COM Automation (`Word`, `PowerPoint`, `Excel` via `IDispatch`)
//! and Windows Email sharing via dynamically loaded `mapi32.dll` (`MAPISendMailW`).

use crate::PlatformError;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance,
    CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO, IDispatch,
};
use windows::Win32::System::Ole::{DISPID_PROPERTYPUT, GetActiveObject};
use windows::core::{BSTR, GUID, HSTRING, Interface, PCWSTR, VARIANT};
use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// Target Microsoft Office desktop application for direct COM image insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeTarget {
    Word,
    PowerPoint,
    Excel,
}

impl OfficeTarget {
    pub const fn prog_id(self) -> &'static str {
        match self {
            Self::Word => "Word.Application",
            Self::PowerPoint => "PowerPoint.Application",
            Self::Excel => "Excel.Application",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Word => "Microsoft Word",
            Self::PowerPoint => "Microsoft PowerPoint",
            Self::Excel => "Microsoft Excel",
        }
    }
}

/// Insert an exported image file directly into a running (or newly launched) Microsoft Word,
/// PowerPoint, or Excel instance via COM `IDispatch` Automation.
///
/// Automatically sets `Application.Visible = true` and creates a blank Document / Presentation+Slide /
/// Workbook when none is open yet, avoiding `DISP_E_EXCEPTION (0x80020009 / 2147614729)`.
pub fn send_image_to_office(target: OfficeTarget, image_path: &Path) -> Result<(), PlatformError> {
    if !image_path.exists() {
        return Err(PlatformError::ImageCodec(format!(
            "image path does not exist: {}",
            image_path.display()
        )));
    }
    let co_init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let res = (|| -> Result<(), PlatformError> {
        let prog_id = HSTRING::from(target.prog_id());
        let clsid = unsafe { CLSIDFromProgID(PCWSTR(prog_id.as_ptr())) }.map_err(|_| {
            PlatformError::ComAutomation(format!(
                "{} is not installed on this PC",
                target.display_name()
            ))
        })?;

        let mut active_unk = None;
        let app: IDispatch = if unsafe { GetActiveObject(&clsid, None, &mut active_unk) }.is_ok() {
            active_unk
                .ok_or_else(|| {
                    PlatformError::ComAutomation(format!(
                        "Failed to attach to running {}",
                        target.display_name()
                    ))
                })?
                .cast()
                .map_err(|e| {
                    PlatformError::ComAutomation(format!(
                        "{} COM interface cast failed (0x{:08X})",
                        target.display_name(),
                        e.code().0 as u32
                    ))
                })?
        } else {
            unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) }.map_err(|e| {
                PlatformError::ComAutomation(format!(
                    "Could not launch {} (0x{:08X})",
                    target.display_name(),
                    e.code().0 as u32
                ))
            })?
        };

        // Always make the Office application window visible first!
        let _ = dispatch_put_bool(&app, "Visible", true);

        let img_str = image_path.to_string_lossy().to_string();
        match target {
            OfficeTarget::Word => {
                // Ensure at least one Document is open before accessing Application.Selection
                let docs = dispatch_get_obj(&app, "Documents")?;
                if dispatch_get_i32(&docs, "Count").unwrap_or(0) == 0 {
                    let _ = dispatch_call(&docs, "Add", &mut [])?;
                }
                let sel = dispatch_get_obj(&app, "Selection")?;
                let shapes = dispatch_get_obj(&sel, "InlineShapes")?;
                let mut args = [VARIANT::from(BSTR::from(img_str))];
                let _ = dispatch_call(&shapes, "AddPicture", &mut args)?;
            }
            OfficeTarget::PowerPoint => {
                // Ensure a Presentation and at least one Slide exist before inserting picture
                let pres_col = dispatch_get_obj(&app, "Presentations")?;
                let pres = if dispatch_get_i32(&pres_col, "Count").unwrap_or(0) == 0 {
                    let added = dispatch_call(&pres_col, "Add", &mut [])?;
                    IDispatch::try_from(&added)
                        .or_else(|_| dispatch_get_obj(&app, "ActivePresentation"))?
                } else {
                    dispatch_get_obj(&app, "ActivePresentation")?
                };

                let slides = dispatch_get_obj(&pres, "Slides")?;
                let slide = if dispatch_get_i32(&slides, "Count").unwrap_or(0) == 0 {
                    // Slides.Add(Index = 1, Layout = ppLayoutBlank (12)) -> reversed COM args
                    let mut add_args = [VARIANT::from(12i32), VARIANT::from(1i32)];
                    let added_slide = dispatch_call(&slides, "Add", &mut add_args)?;
                    IDispatch::try_from(&added_slide).map_err(|e| {
                        PlatformError::ComAutomation(format!(
                            "PowerPoint Slides.Add returned invalid slide (0x{:08X})",
                            e.code().0 as u32
                        ))
                    })?
                } else if let Ok(slide_from_view) = dispatch_get_obj(&app, "ActiveWindow")
                    .and_then(|w| dispatch_get_obj(&w, "View"))
                    .and_then(|v| dispatch_get_obj(&v, "Slide"))
                {
                    slide_from_view
                } else {
                    let mut idx_arg = [VARIANT::from(1i32)];
                    let first_slide = dispatch_call(&slides, "Item", &mut idx_arg)?;
                    IDispatch::try_from(&first_slide).map_err(|e| {
                        PlatformError::ComAutomation(format!(
                            "PowerPoint Slides.Item(1) returned invalid slide (0x{:08X})",
                            e.code().0 as u32
                        ))
                    })?
                };

                let shapes = dispatch_get_obj(&slide, "Shapes")?;
                // Shapes.AddPicture(FileName, LinkToFile=0, SaveWithDocument=-1, Left=50, Top=50) -> reversed!
                let mut args = [
                    VARIANT::from(50.0f32),
                    VARIANT::from(50.0f32),
                    VARIANT::from(-1i32),
                    VARIANT::from(0i32),
                    VARIANT::from(BSTR::from(img_str)),
                ];
                let _ = dispatch_call(&shapes, "AddPicture", &mut args)?;
            }
            OfficeTarget::Excel => {
                // Ensure at least one Workbook is open before accessing ActiveSheet
                let books = dispatch_get_obj(&app, "Workbooks")?;
                if dispatch_get_i32(&books, "Count").unwrap_or(0) == 0 {
                    let _ = dispatch_call(&books, "Add", &mut [])?;
                }
                let sheet = dispatch_get_obj(&app, "ActiveSheet")?;
                // Prefer Shapes.AddPicture(FileName, LinkToFile=0, SaveWithDocument=-1, Left=20, Top=20, Width=-1, Height=-1)
                let added_via_shapes = dispatch_get_obj(&sheet, "Shapes").and_then(|shapes| {
                    let mut args = [
                        VARIANT::from(-1.0f32),
                        VARIANT::from(-1.0f32),
                        VARIANT::from(20.0f32),
                        VARIANT::from(20.0f32),
                        VARIANT::from(-1i32),
                        VARIANT::from(0i32),
                        VARIANT::from(BSTR::from(img_str.clone())),
                    ];
                    dispatch_call(&shapes, "AddPicture", &mut args)
                });
                if added_via_shapes.is_err() {
                    let pics = dispatch_get_obj(&sheet, "Pictures")?;
                    let mut arg = [VARIANT::from(BSTR::from(img_str))];
                    let _ = dispatch_call(&pics, "Insert", &mut arg)?;
                }
            }
        }
        Ok(())
    })();

    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
    res
}

fn format_invoke_error(context: &str, hr: windows::core::Error, excep: &EXCEPINFO) -> PlatformError {
    let desc = excep.bstrDescription.to_string();
    if !desc.trim().is_empty() {
        PlatformError::ComAutomation(format!("Office {context}: {}", desc.trim()))
    } else {
        let code = hr.code().0 as u32;
        PlatformError::ComAutomation(format!(
            "Office {context} failed (0x{code:08X})"
        ))
    }
}

fn dispatch_get_id(disp: &IDispatch, name: &str) -> Result<i32, PlatformError> {
    let wname = HSTRING::from(name);
    let mut ptr = PCWSTR(wname.as_ptr());
    let mut dispid = 0i32;
    unsafe { disp.GetIDsOfNames(&GUID::zeroed(), &mut ptr, 1, 0x0409, &mut dispid) }.map_err(
        |e| {
            PlatformError::ComAutomation(format!(
                "Office member `{name}` not found (0x{:08X})",
                e.code().0 as u32
            ))
        },
    )?;
    Ok(dispid)
}

fn dispatch_invoke_raw(
    disp: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    params: &DISPPARAMS,
) -> Result<VARIANT, PlatformError> {
    let dispid = dispatch_get_id(disp, name)?;
    let mut result = VARIANT::default();
    let mut excep = EXCEPINFO::default();
    unsafe {
        disp.Invoke(
            dispid,
            &GUID::zeroed(),
            0x0409,
            flags,
            params,
            Some(&mut result),
            Some(&mut excep),
            None,
        )
    }
    .map_err(|e| format_invoke_error(name, e, &excep))?;
    Ok(result)
}

fn dispatch_get_obj(disp: &IDispatch, prop: &str) -> Result<IDispatch, PlatformError> {
    let params = DISPPARAMS::default();
    let result = dispatch_invoke_raw(disp, prop, DISPATCH_PROPERTYGET, &params)?;
    IDispatch::try_from(&result).map_err(|e| {
        PlatformError::ComAutomation(format!(
            "Office `{prop}` returned null/non-object (0x{:08X})",
            e.code().0 as u32
        ))
    })
}

fn dispatch_get_i32(disp: &IDispatch, prop: &str) -> Result<i32, PlatformError> {
    let params = DISPPARAMS::default();
    let result = dispatch_invoke_raw(disp, prop, DISPATCH_PROPERTYGET, &params)?;
    i32::try_from(&result).map_err(|e| {
        PlatformError::ComAutomation(format!(
            "Office `{prop}` is not an integer (0x{:08X})",
            e.code().0 as u32
        ))
    })
}

fn dispatch_put_bool(disp: &IDispatch, prop: &str, val: bool) -> Result<(), PlatformError> {
    let mut arg = [VARIANT::from(val)];
    let mut named = [DISPID_PROPERTYPUT];
    let params = DISPPARAMS {
        rgvarg: arg.as_mut_ptr(),
        rgdispidNamedArgs: named.as_mut_ptr(),
        cArgs: 1,
        cNamedArgs: 1,
    };
    let _ = dispatch_invoke_raw(disp, prop, DISPATCH_PROPERTYPUT, &params)?;
    Ok(())
}

fn dispatch_call(
    disp: &IDispatch,
    method: &str,
    args_reversed: &mut [VARIANT],
) -> Result<VARIANT, PlatformError> {
    let params = DISPPARAMS {
        rgvarg: if args_reversed.is_empty() {
            null_mut()
        } else {
            args_reversed.as_mut_ptr()
        },
        rgdispidNamedArgs: null_mut(),
        cArgs: args_reversed.len() as u32,
        cNamedArgs: 0,
    };
    dispatch_invoke_raw(disp, method, DISPATCH_METHOD, &params)
}

#[repr(C)]
struct MapiFileDescW {
    ul_reserved: u32,
    fl_flags: u32,
    n_position: u32,
    lpsz_path_name: *const u16,
    lpsz_file_name: *const u16,
    lp_file_type: *const std::ffi::c_void,
}

#[repr(C)]
struct MapiMessageW {
    ul_reserved: u32,
    lpsz_subject: *const u16,
    lpsz_note_text: *const u16,
    lpsz_message_type: *const u16,
    lpsz_date_received: *const u16,
    lpsz_conversation_id: *const u16,
    fl_flags: u32,
    lp_originator: *const std::ffi::c_void,
    n_recip_count: u32,
    lp_recips: *const std::ffi::c_void,
    n_file_count: u32,
    lp_files: *const MapiFileDescW,
}

type FnMapiSendMailW = unsafe extern "system" fn(
    lh_session: usize,
    ul_ui_param: usize,
    lp_message: *const MapiMessageW,
    fl_flags: u32,
    ul_reserved: u32,
) -> u32;

const MAPI_DIALOG: u32 = 0x0000_0008;
const MAPI_LOGON_UI: u32 = 0x0000_0001;

/// Open the default Windows email client compose window with `file_path` attached via
/// dynamically loaded `mapi32.dll` (`MAPISendMailW`).
pub fn send_file_via_email_mapi(file_path: &Path, subject: &str) -> Result<(), PlatformError> {
    if !file_path.exists() {
        return Err(PlatformError::ImageCodec(format!(
            "attachment does not exist: {}",
            file_path.display()
        )));
    }
    let dll_name: Vec<u16> = "mapi32.dll\0".encode_utf16().collect();
    let hmod = unsafe { LoadLibraryW(dll_name.as_ptr()) };
    if hmod.is_null() {
        return Err(PlatformError::Win32 {
            api: "LoadLibraryW(mapi32.dll)",
            code: unsafe { GetLastError() },
        });
    }

    let proc_opt = unsafe { GetProcAddress(hmod, b"MAPISendMailW\0".as_ptr()) };
    let Some(proc_fn) = proc_opt else {
        let code = unsafe { GetLastError() };
        unsafe { FreeLibrary(hmod) };
        return Err(PlatformError::Win32 {
            api: "GetProcAddress(MAPISendMailW)",
            code,
        });
    };

    let mapi_send: FnMapiSendMailW = unsafe { std::mem::transmute(proc_fn) };
    let path_w: Vec<u16> = file_path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let fname_w: Vec<u16> = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "capture.png".into())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let subj_w: Vec<u16> = subject
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let file_desc = MapiFileDescW {
        ul_reserved: 0,
        fl_flags: 0,
        n_position: u32::MAX,
        lpsz_path_name: path_w.as_ptr(),
        lpsz_file_name: fname_w.as_ptr(),
        lp_file_type: null(),
    };
    let msg = MapiMessageW {
        ul_reserved: 0,
        lpsz_subject: subj_w.as_ptr(),
        lpsz_note_text: null(),
        lpsz_message_type: null(),
        lpsz_date_received: null(),
        lpsz_conversation_id: null(),
        fl_flags: 0,
        lp_originator: null(),
        n_recip_count: 0,
        lp_recips: null(),
        n_file_count: 1,
        lp_files: &file_desc,
    };

    let ret = unsafe { mapi_send(0, 0, &msg, MAPI_LOGON_UI | MAPI_DIALOG, 0) };
    unsafe { FreeLibrary(hmod) };
    if ret != 0 && ret != 1 {
        // 0 = SUCCESS_SUCCESS, 1 = MAPI_USER_ABORT
        return Err(PlatformError::Win32 {
            api: "MAPISendMailW",
            code: ret,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_prog_ids_and_missing_file_validation() {
        assert_eq!(OfficeTarget::Word.prog_id(), "Word.Application");
        assert_eq!(OfficeTarget::PowerPoint.prog_id(), "PowerPoint.Application");
        assert_eq!(OfficeTarget::Excel.prog_id(), "Excel.Application");

        let missing = Path::new(r"C:\nonexistent_snagsoup_image_xyz.png");
        assert!(send_image_to_office(OfficeTarget::Word, missing).is_err());
        assert!(send_file_via_email_mapi(missing, "Test").is_err());
    }
}
