//! SnagSoup Windows Explorer Shell Extension (`snagsoup_shellext.dll`):
//! Lightweight COM `IThumbnailProvider` + `IPropertyStore` + `IPropertyStoreCapabilities`
//! for `.snagx` files (Zero-GPUI, Zero-OpenCV, per-user HKCU registration).

#![allow(non_snake_case, non_camel_case_types, clippy::missing_safety_doc)]

use snagsoup_model::SnagxArchive;
use std::ffi::c_void;
use std::io::Cursor;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;
use windows_sys::core::HRESULT;
use windows_sys::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOINTERFACE,
    E_OUTOFMEMORY, E_POINTER, HMODULE, S_FALSE, S_OK,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::System::Com::CoTaskMemAlloc;
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, REG_SZ,
};

// ============================================================================
// COM GUIDs & Constants
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

/// SnagSoup Shell Extension CLSID: `{7A4E2F10-8C3B-4B91-9D4E-51C2A8E7F301}`
pub const CLSID_SNAGSOUP_SHELLEXT: Guid = Guid::new(
    0x7A4E2F10,
    0x8C3B,
    0x4B91,
    [0x9D, 0x4E, 0x51, 0xC2, 0xA8, 0xE7, 0xF3, 0x01],
);
const CLSID_STR: &str = "{7A4E2F10-8C3B-4B91-9D4E-51C2A8E7F301}";

pub const IID_IUNKNOWN: Guid = Guid::new(
    0x00000000,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_ICLASSFACTORY: Guid = Guid::new(
    0x00000001,
    0x0000,
    0x0000,
    [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
);
pub const IID_IINITIALIZEWITHSTREAM: Guid = Guid::new(
    0xB824B49D,
    0x22AC,
    0x4161,
    [0xAC, 0x8A, 0x99, 0x16, 0xE8, 0xFA, 0x3F, 0x7F],
);
pub const IID_ITHUMBNAILPROVIDER: Guid = Guid::new(
    0xE357FCCD,
    0xA995,
    0x4576,
    [0xB0, 0x1F, 0x23, 0x46, 0x30, 0x15, 0x4E, 0x96],
);
pub const IID_IPROPERTYSTORE: Guid = Guid::new(
    0x886D8EEB,
    0x8CF2,
    0x4446,
    [0x8D, 0x02, 0xCD, 0xBA, 0x1D, 0xBD, 0xCF, 0x99],
);
pub const IID_IPROPERTYSTORECAPABILITIES: Guid = Guid::new(
    0xC8E2D566,
    0x186E,
    0x4D49,
    [0xBF, 0x41, 0x69, 0x09, 0xEA, 0xD5, 0x6A, 0xCC],
);

// Official Snagit 2022 SnagxProperties.propdesc Property Keys + Standard Shell Keys
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyKey {
    pub fmtid: Guid,
    pub pid: u32,
}

pub const PKEY_OS_VERSION: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0x465360F7,
        0xFE1E,
        0x40B9,
        [0xA6, 0xE4, 0x46, 0x43, 0x51, 0x9E, 0xBD, 0x26],
    ),
    pid: 105,
};
pub const PKEY_WEB_URL: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0x0FBC0597,
        0x06EF,
        0x459B,
        [0x95, 0xD0, 0x7C, 0x35, 0x9B, 0xEB, 0xBB, 0x1F],
    ),
    pid: 106,
};
pub const PKEY_WINDOW_NAME: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0x3596DD85,
        0x4F22,
        0x44F3,
        [0x85, 0x60, 0xFD, 0xFA, 0x78, 0xB4, 0x8B, 0x11],
    ),
    pid: 107,
};
pub const PKEY_APP_VERSION: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0xBCD6DCC3,
        0xCF97,
        0x42E0,
        [0xA6, 0xF2, 0xBD, 0x07, 0xB4, 0xAA, 0xA2, 0xB0],
    ),
    pid: 108,
};
pub const PKEY_APP_COMPANY: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0x4C3F20D7,
        0x31F9,
        0x41CC,
        [0x95, 0x3F, 0xA9, 0x5F, 0x85, 0x8C, 0xE1, 0x9B],
    ),
    pid: 109,
};
pub const PKEY_APP_NAME: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0xB34401FF,
        0x599F,
        0x421F,
        [0xB7, 0x15, 0xBE, 0x3E, 0xE3, 0x0C, 0x13, 0x77],
    ),
    pid: 110,
};
pub const PKEY_CALLOUT_TEXT: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0x44FEC733,
        0x8461,
        0x4A1E,
        [0x93, 0x79, 0x23, 0x78, 0x20, 0xBB, 0x93, 0xF1],
    ),
    pid: 112,
};
pub const PKEY_OCR_TEXT: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0xB589B430,
        0x9947,
        0x4480,
        [0x8F, 0x9A, 0x15, 0x83, 0x2F, 0x8B, 0x89, 0x6A],
    ),
    pid: 113,
};
pub const PKEY_SYSTEM_TITLE: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0xF29F85E0,
        0x4FF9,
        0x1068,
        [0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9],
    ),
    pid: 2,
};
pub const PKEY_SYSTEM_KEYWORDS: PropertyKey = PropertyKey {
    fmtid: Guid::new(
        0xF29F85E0,
        0x4FF9,
        0x1068,
        [0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9],
    ),
    pid: 5,
};

pub const WTSAT_ARGB: u32 = 2;
pub const VT_EMPTY: u16 = 0;
pub const VT_LPWSTR: u16 = 31;
const STG_E_ACCESSDENIED: HRESULT = 0x80030005u32 as HRESULT;

/// Standard 24-byte x64 `PROPVARIANT` layout (`vt` + 3 reserved u16 + union pointer/data).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PropVariant {
    pub vt: u16,
    pub reserved1: u16,
    pub reserved2: u16,
    pub reserved3: u16,
    pub data1: usize,
    pub data2: usize,
}

impl Default for PropVariant {
    fn default() -> Self {
        Self {
            vt: VT_EMPTY,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            data1: 0,
            data2: 0,
        }
    }
}

// ============================================================================
// COM Vtable Definitions
// ============================================================================

#[repr(C)]
pub struct IUnknownVtbl {
    pub QueryInterface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct IClassFactoryVtbl {
    pub base: IUnknownVtbl,
    pub CreateInstance:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HRESULT,
    pub LockServer: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

#[repr(C)]
pub struct IInitializeWithStreamVtbl {
    pub base: IUnknownVtbl,
    pub Initialize: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> HRESULT,
}

#[repr(C)]
pub struct IThumbnailProviderVtbl {
    pub base: IUnknownVtbl,
    pub GetThumbnail:
        unsafe extern "system" fn(*mut c_void, u32, *mut HBITMAP, *mut u32) -> HRESULT,
}

#[repr(C)]
pub struct IPropertyStoreVtbl {
    pub base: IUnknownVtbl,
    pub GetCount: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    pub GetAt: unsafe extern "system" fn(*mut c_void, u32, *mut PropertyKey) -> HRESULT,
    pub GetValue: unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> HRESULT,
    pub SetValue: unsafe extern "system" fn(*mut c_void, *const PropertyKey, *const PropVariant) -> HRESULT,
    pub Commit: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

#[repr(C)]
pub struct IPropertyStoreCapabilitiesVtbl {
    pub base: IUnknownVtbl,
    pub IsPropertyWritable: unsafe extern "system" fn(*mut c_void, *const PropertyKey) -> HRESULT,
}

#[repr(C)]
struct IStreamRawVtbl {
    _qi: usize,
    _addref: usize,
    _release: usize,
    read: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut u32) -> HRESULT,
}

#[repr(C)]
struct IStreamRaw {
    vtbl: *const IStreamRawVtbl,
}

// ============================================================================
// Global DLL State
// ============================================================================

static DLL_OBJ_COUNT: AtomicU32 = AtomicU32::new(0);
static DLL_LOCK_COUNT: AtomicU32 = AtomicU32::new(0);
static DLL_HMODULE: AtomicUsize = AtomicUsize::new(0);

// ============================================================================
// SnagxShellHandler COM Object (4 Interface Vtable Slots)
// ============================================================================

struct HandlerData {
    thumb_png: Option<Vec<u8>>,
    properties: Vec<(PropertyKey, String)>,
}

#[repr(C)]
struct SnagxShellHandler {
    vtbl_init: *const IInitializeWithStreamVtbl,
    vtbl_thumb: *const IThumbnailProviderVtbl,
    vtbl_prop: *const IPropertyStoreVtbl,
    vtbl_caps: *const IPropertyStoreCapabilitiesVtbl,
    ref_count: AtomicU32,
    data: Mutex<HandlerData>,
}

impl SnagxShellHandler {
    fn new_boxed() -> *mut Self {
        DLL_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
        Box::into_raw(Box::new(Self {
            vtbl_init: &INIT_VTBL,
            vtbl_thumb: &THUMB_VTBL,
            vtbl_prop: &PROP_VTBL,
            vtbl_caps: &CAPS_VTBL,
            ref_count: AtomicU32::new(1),
            data: Mutex::new(HandlerData {
                thumb_png: None,
                properties: Vec::new(),
            }),
        }))
    }

    unsafe fn from_iface(this: *mut c_void, slot: usize) -> *mut Self {
        (this as *mut usize).sub(slot) as *mut Self
    }

    unsafe fn qi(&self, riid: *const Guid, ppv: *mut *mut c_void) -> HRESULT {
        if ppv.is_null() {
            return E_POINTER;
        }
        *ppv = ptr::null_mut();
        if riid.is_null() {
            return E_INVALIDARG;
        }
        let iid = *riid;
        let base = self as *const Self as *mut usize;
        let ptr_out = if iid == IID_IUNKNOWN || iid == IID_IINITIALIZEWITHSTREAM {
            base as *mut c_void
        } else if iid == IID_ITHUMBNAILPROVIDER {
            base.add(1) as *mut c_void
        } else if iid == IID_IPROPERTYSTORE {
            base.add(2) as *mut c_void
        } else if iid == IID_IPROPERTYSTORECAPABILITIES {
            base.add(3) as *mut c_void
        } else {
            return E_NOINTERFACE;
        };
        self.add_ref();
        *ppv = ptr_out;
        S_OK
    }

    fn add_ref(&self) -> u32 {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe fn release(this: *mut Self) -> u32 {
        let prev = (*this).ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this));
            DLL_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
        prev - 1
    }
}

// --- Vtable Thunks ---
macro_rules! impl_iunknown_thunks {
    ($slot:expr, $qi:ident, $ar:ident, $rel:ident) => {
        unsafe extern "system" fn $qi(
            this: *mut c_void,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> HRESULT {
            let obj = SnagxShellHandler::from_iface(this, $slot);
            (*obj).qi(riid, ppv)
        }
        unsafe extern "system" fn $ar(this: *mut c_void) -> u32 {
            let obj = SnagxShellHandler::from_iface(this, $slot);
            (*obj).add_ref()
        }
        unsafe extern "system" fn $rel(this: *mut c_void) -> u32 {
            let obj = SnagxShellHandler::from_iface(this, $slot);
            SnagxShellHandler::release(obj)
        }
    };
}

impl_iunknown_thunks!(0, init_qi, init_addref, init_release);
impl_iunknown_thunks!(1, thumb_qi, thumb_addref, thumb_release);
impl_iunknown_thunks!(2, prop_qi, prop_addref, prop_release);
impl_iunknown_thunks!(3, caps_qi, caps_addref, caps_release);

unsafe extern "system" fn init_initialize(
    this: *mut c_void,
    pstream: *mut c_void,
    _grf_mode: u32,
) -> HRESULT {
    if pstream.is_null() {
        return E_INVALIDARG;
    }
    let obj = SnagxShellHandler::from_iface(this, 0);
    let stream = pstream as *mut IStreamRaw;
    let read_fn = (*(*stream).vtbl).read;

    // Read full IStream into memory (capped at 128 MB safety guard)
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let mut read_len = 0u32;
        let hr = read_fn(
            pstream,
            chunk.as_mut_ptr() as *mut c_void,
            chunk.len() as u32,
            &mut read_len,
        );
        if hr < 0 {
            return hr;
        }
        if read_len == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read_len as usize]);
        if bytes.len() > 128 * 1024 * 1024 {
            return E_OUTOFMEMORY;
        }
    }

    let Ok(archive) = SnagxArchive::from_bytes(&bytes) else {
        return E_FAIL;
    };

    let thumb_png = archive
        .thumbnail_png
        .clone()
        .or_else(|| archive.pages.first().and_then(|p| p.background_png.clone()));

    let mut props = Vec::new();
    let m = &archive.metadata;
    let mut push_opt = |key: PropertyKey, val: &Option<String>| {
        if let Some(ref s) = val {
            if !s.is_empty() {
                props.push((key, s.clone()));
            }
        }
    };
    push_opt(PKEY_OS_VERSION, &m.operating_system_version);
    push_opt(PKEY_WEB_URL, &m.web_url);
    push_opt(PKEY_WINDOW_NAME, &m.window_name);
    push_opt(PKEY_APP_VERSION, &m.app_version);
    push_opt(PKEY_APP_COMPANY, &m.app_company);
    push_opt(PKEY_APP_NAME, &m.app_name);
    push_opt(PKEY_CALLOUT_TEXT, &m.callout_text);
    push_opt(PKEY_OCR_TEXT, &m.ocr_text);
    push_opt(PKEY_SYSTEM_TITLE, &m.window_name);
    if let Some(ref tags) = m.tags {
        if !tags.is_empty() {
            props.push((PKEY_SYSTEM_KEYWORDS, tags.join("; ")));
        }
    }

    if let Ok(mut guard) = (*obj).data.lock() {
        guard.thumb_png = thumb_png;
        guard.properties = props;
        S_OK
    } else {
        E_FAIL
    }
}

unsafe extern "system" fn thumb_get_thumbnail(
    this: *mut c_void,
    cx: u32,
    phbmp: *mut HBITMAP,
    pdw_alpha: *mut u32,
) -> HRESULT {
    if phbmp.is_null() || pdw_alpha.is_null() {
        return E_POINTER;
    }
    *phbmp = ptr::null_mut();
    *pdw_alpha = WTSAT_ARGB;

    let obj = SnagxShellHandler::from_iface(this, 1);
    let png_bytes = {
        let Ok(guard) = (*obj).data.lock() else {
            return E_FAIL;
        };
        let Some(ref b) = guard.thumb_png else {
            return E_FAIL;
        };
        b.clone()
    };

    let Ok((src_w, src_h, rgba)) = decode_png_rgba(&png_bytes) else {
        return E_FAIL;
    };
    if src_w == 0 || src_h == 0 {
        return E_FAIL;
    }

    let max_dim = cx.max(1);
    let (dst_w, dst_h, pixels) = if src_w > max_dim || src_h > max_dim {
        let scale = max_dim as f32 / src_w.max(src_h) as f32;
        let nw = ((src_w as f32 * scale).round() as u32).max(1);
        let nh = ((src_h as f32 * scale).round() as u32).max(1);
        (nw, nh, downscale_rgba(&rgba, src_w, src_h, nw, nh))
    } else {
        (src_w, src_h, rgba)
    };

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: dst_w as i32,
            biHeight: -(dst_h as i32), // top-down DIB
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [std::mem::zeroed()],
    };

    let mut bits_ptr: *mut c_void = ptr::null_mut();
    let hbmp = CreateDIBSection(
        ptr::null_mut(),
        &bmi,
        DIB_RGB_COLORS,
        &mut bits_ptr,
        ptr::null_mut(),
        0,
    );
    if hbmp.is_null() || bits_ptr.is_null() {
        return E_OUTOFMEMORY;
    }

    // Write premultiplied BGRA8 for WTSAT_ARGB
    let dst = std::slice::from_raw_parts_mut(
        bits_ptr as *mut u8,
        (dst_w as usize) * (dst_h as usize) * 4,
    );
    for (s, d) in pixels.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let a = s[3] as u16;
        d[0] = ((s[2] as u16 * a + 127) / 255) as u8;
        d[1] = ((s[1] as u16 * a + 127) / 255) as u8;
        d[2] = ((s[0] as u16 * a + 127) / 255) as u8;
        d[3] = s[3];
    }

    *phbmp = hbmp;
    *pdw_alpha = WTSAT_ARGB;
    S_OK
}

unsafe extern "system" fn prop_get_count(this: *mut c_void, cprops: *mut u32) -> HRESULT {
    if cprops.is_null() {
        return E_POINTER;
    }
    let obj = SnagxShellHandler::from_iface(this, 2);
    if let Ok(guard) = (*obj).data.lock() {
        *cprops = guard.properties.len() as u32;
        S_OK
    } else {
        E_FAIL
    }
}

unsafe extern "system" fn prop_get_at(
    this: *mut c_void,
    iprop: u32,
    pkey: *mut PropertyKey,
) -> HRESULT {
    if pkey.is_null() {
        return E_POINTER;
    }
    let obj = SnagxShellHandler::from_iface(this, 2);
    if let Ok(guard) = (*obj).data.lock() {
        if let Some((k, _)) = guard.properties.get(iprop as usize) {
            *pkey = *k;
            S_OK
        } else {
            E_INVALIDARG
        }
    } else {
        E_FAIL
    }
}

unsafe extern "system" fn prop_get_value(
    this: *mut c_void,
    key: *const PropertyKey,
    pv: *mut PropVariant,
) -> HRESULT {
    if key.is_null() || pv.is_null() {
        return E_POINTER;
    }
    *pv = PropVariant::default();
    let target = *key;
    let obj = SnagxShellHandler::from_iface(this, 2);
    let val_opt = {
        let Ok(guard) = (*obj).data.lock() else {
            return E_FAIL;
        };
        guard
            .properties
            .iter()
            .find(|(k, _)| *k == target)
            .map(|(_, v)| v.clone())
    };

    if let Some(val) = val_opt {
        let wide: Vec<u16> = val.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = wide.len() * 2;
        let mem = CoTaskMemAlloc(byte_len) as *mut u16;
        if mem.is_null() {
            return E_OUTOFMEMORY;
        }
        ptr::copy_nonoverlapping(wide.as_ptr(), mem, wide.len());
        (*pv).vt = VT_LPWSTR;
        (*pv).data1 = mem as usize;
    }
    S_OK
}

unsafe extern "system" fn prop_set_value(
    _this: *mut c_void,
    _key: *const PropertyKey,
    _pv: *const PropVariant,
) -> HRESULT {
    STG_E_ACCESSDENIED
}

unsafe extern "system" fn prop_commit(_this: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn caps_is_writable(
    _this: *mut c_void,
    _key: *const PropertyKey,
) -> HRESULT {
    S_FALSE
}

static INIT_VTBL: IInitializeWithStreamVtbl = IInitializeWithStreamVtbl {
    base: IUnknownVtbl {
        QueryInterface: init_qi,
        AddRef: init_addref,
        Release: init_release,
    },
    Initialize: init_initialize,
};

static THUMB_VTBL: IThumbnailProviderVtbl = IThumbnailProviderVtbl {
    base: IUnknownVtbl {
        QueryInterface: thumb_qi,
        AddRef: thumb_addref,
        Release: thumb_release,
    },
    GetThumbnail: thumb_get_thumbnail,
};

static PROP_VTBL: IPropertyStoreVtbl = IPropertyStoreVtbl {
    base: IUnknownVtbl {
        QueryInterface: prop_qi,
        AddRef: prop_addref,
        Release: prop_release,
    },
    GetCount: prop_get_count,
    GetAt: prop_get_at,
    GetValue: prop_get_value,
    SetValue: prop_set_value,
    Commit: prop_commit,
};

static CAPS_VTBL: IPropertyStoreCapabilitiesVtbl = IPropertyStoreCapabilitiesVtbl {
    base: IUnknownVtbl {
        QueryInterface: caps_qi,
        AddRef: caps_addref,
        Release: caps_release,
    },
    IsPropertyWritable: caps_is_writable,
};

// ============================================================================
// IClassFactory Implementation
// ============================================================================

#[repr(C)]
struct ClassFactory {
    vtbl: *const IClassFactoryVtbl,
    ref_count: AtomicU32,
}

unsafe extern "system" fn cf_qi(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    *ppv = ptr::null_mut();
    if riid.is_null() {
        return E_INVALIDARG;
    }
    let iid = *riid;
    if iid == IID_IUNKNOWN || iid == IID_ICLASSFACTORY {
        cf_addref(this);
        *ppv = this;
        S_OK
    } else {
        E_NOINTERFACE
    }
}

unsafe extern "system" fn cf_addref(this: *mut c_void) -> u32 {
    let cf = this as *mut ClassFactory;
    (*cf).ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn cf_release(this: *mut c_void) -> u32 {
    let cf = this as *mut ClassFactory;
    let prev = (*cf).ref_count.fetch_sub(1, Ordering::Release);
    if prev == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(cf));
        DLL_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
    prev - 1
}

unsafe extern "system" fn cf_create_instance(
    _this: *mut c_void,
    punk_outer: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if !punk_outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let handler = SnagxShellHandler::new_boxed();
    let hr = (*handler).qi(riid, ppv);
    SnagxShellHandler::release(handler);
    hr
}

unsafe extern "system" fn cf_lock_server(_this: *mut c_void, flock: i32) -> HRESULT {
    if flock != 0 {
        DLL_LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    } else {
        DLL_LOCK_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
    S_OK
}

static CF_VTBL: IClassFactoryVtbl = IClassFactoryVtbl {
    base: IUnknownVtbl {
        QueryInterface: cf_qi,
        AddRef: cf_addref,
        Release: cf_release,
    },
    CreateInstance: cf_create_instance,
    LockServer: cf_lock_server,
};

// ============================================================================
// Standard COM DLL Entrypoints
// ============================================================================

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    hinst_dll: HMODULE,
    fdw_reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if fdw_reason == 1 {
        DLL_HMODULE.store(hinst_dll as usize, Ordering::SeqCst);
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const Guid,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    *ppv = ptr::null_mut();
    if rclsid.is_null() || *rclsid != CLSID_SNAGSOUP_SHELLEXT {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    DLL_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
    let cf = Box::into_raw(Box::new(ClassFactory {
        vtbl: &CF_VTBL,
        ref_count: AtomicU32::new(1),
    }));
    let hr = cf_qi(cf as *mut c_void, riid, ppv);
    cf_release(cf as *mut c_void);
    hr
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if DLL_OBJ_COUNT.load(Ordering::SeqCst) == 0 && DLL_LOCK_COUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Per-user `HKCU\Software\Classes` COM registration (no Admin UAC required!).
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    let mut path_buf = [0u16; 1024];
    let hmod = DLL_HMODULE.load(Ordering::SeqCst) as HMODULE;
    let len = GetModuleFileNameW(hmod, path_buf.as_mut_ptr(), path_buf.len() as u32);
    if len == 0 {
        return E_FAIL;
    }
    let dll_path = String::from_utf16_lossy(&path_buf[..len as usize]);

    let clsid_key = format!(r"Software\Classes\CLSID\{CLSID_STR}");
    let inproc_key = format!(r"Software\Classes\CLSID\{CLSID_STR}\InprocServer32");
    let thumb_key = r"Software\Classes\.snagx\ShellEx\{E357FCCD-A995-4576-B01F-234630154E96}";

    if set_hkcu_sz(&clsid_key, None, "SnagSoup .snagx Shell Extension") != 0
        || set_hkcu_sz(&inproc_key, None, &dll_path) != 0
        || set_hkcu_sz(&inproc_key, Some("ThreadingModel"), "Both") != 0
        || set_hkcu_sz(thumb_key, None, CLSID_STR) != 0
    {
        return E_FAIL;
    }
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    let clsid_key = format!(r"Software\Classes\CLSID\{CLSID_STR}");
    let thumb_key = r"Software\Classes\.snagx\ShellEx\{E357FCCD-A995-4576-B01F-234630154E96}";
    delete_hkcu_tree(&clsid_key);
    delete_hkcu_tree(thumb_key);
    S_OK
}

unsafe fn set_hkcu_sz(subkey: &str, value_name: Option<&str>, data: &str) -> u32 {
    let sub_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hkey: HKEY = ptr::null_mut();
    let st = RegCreateKeyW(HKEY_CURRENT_USER, sub_w.as_ptr(), &mut hkey);
    if st != 0 {
        return st;
    }
    let name_w: Option<Vec<u16>> =
        value_name.map(|n| n.encode_utf16().chain(std::iter::once(0)).collect());
    let data_w: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
    let res = RegSetValueExW(
        hkey,
        name_w.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
        0,
        REG_SZ,
        data_w.as_ptr() as *const u8,
        (data_w.len() * 2) as u32,
    );
    RegCloseKey(hkey);
    res
}

unsafe fn delete_hkcu_tree(subkey: &str) {
    let sub_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    RegDeleteTreeW(HKEY_CURRENT_USER, sub_w.as_ptr());
}

// ============================================================================
// Minimal PNG Decoder & Bilinear Downscaler
// ============================================================================

fn decode_png_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), png::DecodingError> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let raw = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => raw
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => raw
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => raw.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => raw.to_vec(),
    };
    Ok((info.width, info.height, rgba))
}

fn downscale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    let xr = sw as f32 / dw as f32;
    let yr = sh as f32 / dh as f32;
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * yr - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = sy.floor() as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = sy - y0 as f32;
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * xr - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = sx.floor() as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = sx - x0 as f32;
            let i00 = ((y0 * sw + x0) as usize) * 4;
            let i10 = ((y0 * sw + x1) as usize) * 4;
            let i01 = ((y1 * sw + x0) as usize) * 4;
            let i11 = ((y1 * sw + x1) as usize) * 4;
            let di = ((y * dw + x) as usize) * 4;
            for c in 0..4 {
                let top = src[i00 + c] as f32 * (1.0 - wx) + src[i10 + c] as f32 * wx;
                let bot = src[i01 + c] as f32 * (1.0 - wx) + src[i11 + c] as f32 * wx;
                out[di + c] = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHCreateMemStream;

    #[test]
    fn com_thumbnail_and_property_store_on_editor_overview_snagx() {
        let path = Path::new(r"C:\Program Files\TechSmith\Snagit 2022\en-US\EditorOverview.snagx");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(path).unwrap();

        unsafe {
            // 1. Obtain IClassFactory via DllGetClassObject
            let mut cf_ptr: *mut c_void = ptr::null_mut();
            assert_eq!(
                DllGetClassObject(&CLSID_SNAGSOUP_SHELLEXT, &IID_ICLASSFACTORY, &mut cf_ptr),
                S_OK
            );
            assert!(!cf_ptr.is_null());
            let cf_vtbl = *(cf_ptr as *mut *const IClassFactoryVtbl);

            // 2. Create handler requesting IInitializeWithStream
            let mut init_ptr: *mut c_void = ptr::null_mut();
            assert_eq!(
                ((*cf_vtbl).CreateInstance)(
                    cf_ptr,
                    ptr::null_mut(),
                    &IID_IINITIALIZEWITHSTREAM,
                    &mut init_ptr,
                ),
                S_OK
            );
            assert!(!init_ptr.is_null());

            // 3. Feed EditorOverview.snagx bytes via SHCreateMemStream
            let stream = SHCreateMemStream(bytes.as_ptr(), bytes.len() as u32);
            assert!(!stream.is_null());
            let init_vtbl = *(init_ptr as *mut *const IInitializeWithStreamVtbl);
            assert_eq!(((*init_vtbl).Initialize)(init_ptr, stream, 0), S_OK);
            // Release IStream
            let stream_vtbl = *(stream as *mut *const IUnknownVtbl);
            ((*stream_vtbl).Release)(stream);

            // 4. QueryInterface -> IThumbnailProvider & GetThumbnail(256)
            let mut thumb_ptr: *mut c_void = ptr::null_mut();
            assert_eq!(
                ((*init_vtbl).base.QueryInterface)(init_ptr, &IID_ITHUMBNAILPROVIDER, &mut thumb_ptr),
                S_OK
            );
            let thumb_vtbl = *(thumb_ptr as *mut *const IThumbnailProviderVtbl);
            let mut hbmp: HBITMAP = ptr::null_mut();
            let mut alpha: u32 = 0;
            assert_eq!(
                ((*thumb_vtbl).GetThumbnail)(thumb_ptr, 256, &mut hbmp, &mut alpha),
                S_OK
            );
            assert!(!hbmp.is_null());
            assert_eq!(alpha, WTSAT_ARGB);

            let mut bm: BITMAP = std::mem::zeroed();
            let got = GetObjectW(
                hbmp,
                std::mem::size_of::<BITMAP>() as i32,
                &mut bm as *mut _ as *mut c_void,
            );
            assert!(got > 0);
            assert!(bm.bmWidth > 0 && bm.bmWidth <= 256);
            assert!(bm.bmHeight > 0 && bm.bmHeight <= 256);
            assert_eq!(bm.bmBitsPixel, 32);
            DeleteObject(hbmp);

            // 5. QueryInterface -> IPropertyStore & inspect properties
            let mut prop_ptr: *mut c_void = ptr::null_mut();
            assert_eq!(
                ((*init_vtbl).base.QueryInterface)(init_ptr, &IID_IPROPERTYSTORE, &mut prop_ptr),
                S_OK
            );
            let prop_vtbl = *(prop_ptr as *mut *const IPropertyStoreVtbl);
            let mut count = 0u32;
            assert_eq!(((*prop_vtbl).GetCount)(prop_ptr, &mut count), S_OK);
            assert!(count >= 1, "Expected at least 1 metadata property in EditorOverview.snagx");

            let mut pv = PropVariant::default();
            assert_eq!(
                ((*prop_vtbl).GetValue)(prop_ptr, &PKEY_OS_VERSION, &mut pv),
                S_OK
            );
            assert_eq!(pv.vt, VT_LPWSTR);
            assert!(pv.data1 != 0);
            // Read UTF-16 string and verify "macOS"
            let mut len = 0usize;
            let p = pv.data1 as *const u16;
            while *p.add(len) != 0 {
                len += 1;
            }
            let os_ver = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            assert!(os_ver.contains("macOS"), "Unexpected OS version: {os_ver}");
            CoTaskMemFree(pv.data1 as *const c_void);

            // 6. Clean COM Release -> DllCanUnloadNow() == S_OK
            ((*prop_vtbl).base.Release)(prop_ptr);
            ((*thumb_vtbl).base.Release)(thumb_ptr);
            ((*init_vtbl).base.Release)(init_ptr);
            ((*cf_vtbl).base.Release)(cf_ptr);
            assert_eq!(DllCanUnloadNow(), S_OK);
        }
    }
}
