//! Custom COM `IDataObject` + `IDropSource` OLE Drag-and-Drop advertising BOTH
//! `CF_HDROP` (`DROPFILES` UTF-16) and registered `"PNG"` memory stream simultaneously
//! for dragging captures from the Recent Tray directly into Explorer, Slack, Chrome, Word, etc.

use crate::PlatformError;
use std::path::PathBuf;
use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
    E_NOTIMPL, HGLOBAL, S_OK,
};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumSTATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl,
    OleInitialize, OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::core::{Error, HRESULT, Result as WinResult, implement};
use windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW;

fn cf_png_format() -> u16 {
    let name: Vec<u16> = "PNG\0".encode_utf16().collect();
    unsafe { RegisterClipboardFormatW(name.as_ptr()) as u16 }
}

/// Build a valid Win32 `DROPFILES` (`CF_HDROP`) memory buffer containing UTF-16LE paths
/// terminated by a double NUL.
pub fn build_dropfiles_hdrop(paths: &[PathBuf]) -> Vec<u8> {
    // DROPFILES header is 20 bytes: pFiles(u32), pt.x(i32), pt.y(i32), fNC(i32), fWide(i32)
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles = 20
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0i32.to_le_bytes()); // fNC
    out.extend_from_slice(&1i32.to_le_bytes()); // fWide = TRUE (UTF-16)

    for p in paths {
        for unit in p.as_os_str().to_string_lossy().encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    // Final double-NUL terminator
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn alloc_hglobal(bytes: &[u8]) -> WinResult<HGLOBAL> {
    let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }?;
    let ptr = unsafe { GlobalLock(hglobal) } as *mut u8;
    if ptr.is_null() {
        return Err(Error::from_win32());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(hglobal);
    }
    Ok(hglobal)
}

#[implement(IDataObject)]
struct SnagDataObject {
    hdrop_bytes: Vec<u8>,
    png_bytes: Option<Vec<u8>>,
    cf_png: u16,
}

impl SnagDataObject {
    fn supported_formats(&self) -> Vec<FORMATETC> {
        let mut fmts = Vec::new();
        if !self.hdrop_bytes.is_empty() {
            fmts.push(FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            });
        }
        if self.png_bytes.is_some() && self.cf_png != 0 {
            fmts.push(FORMATETC {
                cfFormat: self.cf_png,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            });
        }
        fmts
    }
}

impl IDataObject_Impl for SnagDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> WinResult<STGMEDIUM> {
        let fmt = unsafe { &*pformatetcin };
        if (fmt.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
            return Err(Error::from(DV_E_FORMATETC));
        }

        let data: &[u8] = if fmt.cfFormat == CF_HDROP.0 && !self.hdrop_bytes.is_empty() {
            &self.hdrop_bytes
        } else if fmt.cfFormat == self.cf_png && self.cf_png != 0 {
            match self.png_bytes.as_deref() {
                Some(b) => b,
                None => return Err(Error::from(DV_E_FORMATETC)),
            }
        } else {
            return Err(Error::from(DV_E_FORMATETC));
        };

        let hglobal = alloc_hglobal(data)?;
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(&self, _pformatetc: *const FORMATETC, _pmedium: *mut STGMEDIUM) -> WinResult<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let fmt = unsafe { &*pformatetc };
        if (fmt.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
            return DV_E_FORMATETC;
        }
        if fmt.cfFormat == CF_HDROP.0 && !self.hdrop_bytes.is_empty() {
            S_OK
        } else if fmt.cfFormat == self.cf_png && self.cf_png != 0 && self.png_bytes.is_some() {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        _pformatectin: *const FORMATETC,
        pformatetcout: *mut FORMATETC,
    ) -> HRESULT {
        if !pformatetcout.is_null() {
            unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
        }
        windows::Win32::Foundation::DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _frelease: BOOL,
    ) -> WinResult<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn EnumFormatEtc(&self, _dwdirection: u32) -> WinResult<IEnumFORMATETC> {
        let fmts = self.supported_formats();
        unsafe { SHCreateStdEnumFmtEtc(&fmts) }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: Option<&IAdviseSink>,
    ) -> WinResult<u32> {
        Err(Error::from(E_NOTIMPL))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> WinResult<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
        Err(Error::from(E_NOTIMPL))
    }
}

#[implement(IDropSource)]
struct SnagDropSource;

impl IDropSource_Impl for SnagDropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Create a COM `IDataObject` advertising both `CF_HDROP` (for `file_paths`) and `"PNG"`
/// (when `png_bytes` is `Some`).
pub fn create_drag_data_object(
    file_paths: Vec<PathBuf>,
    png_bytes: Option<Vec<u8>>,
) -> IDataObject {
    let hdrop_bytes = if file_paths.is_empty() {
        Vec::new()
    } else {
        build_dropfiles_hdrop(&file_paths)
    };
    SnagDataObject {
        hdrop_bytes,
        png_bytes,
        cf_png: cf_png_format(),
    }
    .into()
}

/// Initiate a synchronous OLE Drag-and-Drop (`DoDragDrop`) from the UI thread.
pub fn start_ole_drag(
    file_paths: Vec<PathBuf>,
    png_bytes: Option<Vec<u8>>,
) -> Result<u32, PlatformError> {
    let ole_init = unsafe { OleInitialize(None) };
    let data_obj = create_drag_data_object(file_paths, png_bytes);
    let drop_src: IDropSource = SnagDropSource.into();
    let mut effect = DROPEFFECT(0);
    let hr = unsafe { DoDragDrop(&data_obj, &drop_src, DROPEFFECT_COPY, &mut effect as *mut DROPEFFECT) };
    if ole_init.is_ok() {
        unsafe { OleUninitialize() };
    }
    if hr.is_err() && hr != DRAGDROP_S_DROP && hr != DRAGDROP_S_CANCEL {
        return Err(PlatformError::Win32 {
            api: "DoDragDrop",
            code: hr.0 as u32,
        });
    }
    Ok(effect.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::GlobalFree;
    use windows::Win32::System::Memory::GlobalSize;

    #[test]
    fn data_object_serves_hdrop_and_png_formats() {
        let paths = vec![PathBuf::from(r"C:\Temp\capture_01.png")];
        let fake_png = vec![137u8, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3, 4];
        let obj = create_drag_data_object(paths.clone(), Some(fake_png.clone()));

        // 1. Verify CF_HDROP
        let hdrop_fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        assert_eq!(unsafe { obj.QueryGetData(&hdrop_fmt) }, S_OK);
        let med = unsafe { obj.GetData(&hdrop_fmt) }.expect("GetData(CF_HDROP)");
        let hglobal = unsafe { med.u.hGlobal };
        let size = unsafe { GlobalSize(hglobal) };
        let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
        let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
        unsafe {
            let _ = GlobalUnlock(hglobal);
            let _ = GlobalFree(hglobal);
        }
        assert_eq!(bytes, build_dropfiles_hdrop(&paths));

        // 2. Verify "PNG"
        let png_fmt = FORMATETC {
            cfFormat: cf_png_format(),
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        assert_eq!(unsafe { obj.QueryGetData(&png_fmt) }, S_OK);
        let med_png = unsafe { obj.GetData(&png_fmt) }.expect("GetData(PNG)");
        let hg_png = unsafe { med_png.u.hGlobal };
        let sz_png = unsafe { GlobalSize(hg_png) };
        let ptr_png = unsafe { GlobalLock(hg_png) } as *const u8;
        let out_png = unsafe { std::slice::from_raw_parts(ptr_png, sz_png) }.to_vec();
        unsafe {
            let _ = GlobalUnlock(hg_png);
            let _ = GlobalFree(hg_png);
        }
        assert_eq!(out_png, fake_png);

        // 3. Verify EnumFormatEtc lists both formats
        let enum_fmt = unsafe { obj.EnumFormatEtc(1) }.expect("EnumFormatEtc");
        let mut out = [FORMATETC::default(); 4];
        let mut fetched = 0u32;
        let _ = unsafe { enum_fmt.Next(&mut out, Some(&mut fetched)) };
        assert_eq!(fetched, 2);
    }
}
