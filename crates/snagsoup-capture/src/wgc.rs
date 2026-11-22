//! Windows.Graphics.Capture (`WGC`) monitor capture backend.
//!
//! DWM composes ALL display adapters (discrete GPU, integrated GPU, USB DisplayLink /
//! Indirect Display Drivers, and virtual displays), so `WGC` avoids the black-screen
//! failure mode of `GetDC(NULL)` `BitBlt` and single-adapter DXGI Desktop Duplication.

use crate::{CaptureBackend, CaptureError, CapturedFrame};
use snagsoup_win_platform::dpi::PhysicalRect;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{Interface, factory};

struct RoGuard(bool);

impl RoGuard {
    fn init() -> Self {
        let ok = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
        Self(ok)
    }
}

impl Drop for RoGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

struct PooledWgcDevice {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

// SAFETY: `ID3D11Device` and `CreateDirect3D11DeviceFromDXGIDevice` agile WinRT wrappers are
// free-threaded (`RO_INIT_MULTITHREADED`), and each `PooledWgcDevice` (including its
// `ID3D11DeviceContext`) is checked out exclusively to a single thread at a time via `Mutex`.
unsafe impl Send for PooledWgcDevice {}

static WGC_DEVICE_POOL: OnceLock<Mutex<Vec<PooledWgcDevice>>> = OnceLock::new();

fn device_pool() -> &'static Mutex<Vec<PooledWgcDevice>> {
    WGC_DEVICE_POOL.get_or_init(|| Mutex::new(Vec::with_capacity(4)))
}

fn create_pooled_wgc_device() -> Result<PooledWgcDevice, CaptureError> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let feature_levels = [D3D_FEATURE_LEVEL_11_0];

    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| CaptureError::Dxgi {
        api: "WGC::D3D11CreateDevice",
        hresult: e.code().0,
    })?;

    let device = device.ok_or(CaptureError::Dxgi {
        api: "WGC::D3D11CreateDevice",
        hresult: -1,
    })?;
    let context = context.ok_or(CaptureError::Dxgi {
        api: "WGC::D3D11CreateDevice",
        hresult: -1,
    })?;

    let dxgi_device: IDXGIDevice = device.cast().map_err(|e| CaptureError::Dxgi {
        api: "WGC::cast<IDXGIDevice>",
        hresult: e.code().0,
    })?;
    let inspectable =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }.map_err(|e| {
            CaptureError::Dxgi {
                api: "CreateDirect3D11DeviceFromDXGIDevice",
                hresult: e.code().0,
            }
        })?;
    let winrt_device: IDirect3DDevice = inspectable.cast().map_err(|e| CaptureError::Dxgi {
        api: "WGC::cast<IDirect3DDevice>",
        hresult: e.code().0,
    })?;

    Ok(PooledWgcDevice {
        device,
        context,
        winrt_device,
    })
}

fn checkout_wgc_device() -> Result<PooledWgcDevice, CaptureError> {
    if let Ok(mut pool) = device_pool().lock() {
        if let Some(dev) = pool.pop() {
            return Ok(dev);
        }
    }
    create_pooled_wgc_device()
}

fn checkin_wgc_device(dev: PooledWgcDevice) {
    if let Ok(mut pool) = device_pool().lock() {
        if pool.len() < 4 {
            pool.push(dev);
        }
    }
}

/// Pre-warm `count` reusable WGC `ID3D11Device` instances in the background so the very first
/// multi-monitor capture pays `0ms` driver initialization cost.
pub fn prewarm_wgc_device_pool(count: usize) {
    let target = count.clamp(1, 4);
    let _ = RoGuard::init();
    let mut created = Vec::with_capacity(target);
    for _ in 0..target {
        if let Ok(dev) = create_pooled_wgc_device() {
            created.push(dev);
        }
    }
    for dev in created {
        checkin_wgc_device(dev);
    }
}

/// Reusable continuous `Windows.Graphics.Capture` session for a single `HMONITOR`.
///
/// Can be used for both one-shot monitor freezes (`capture_monitor_wgc`) and continuous
/// live video recording (`LiveScreenRecorder`).
pub struct WgcMonitorSession {
    session: GraphicsCaptureSession,
    frame_pool: Direct3D11CaptureFramePool,
    _item: GraphicsCaptureItem,
    staging: Option<ID3D11Texture2D>,
    staging_desc: Option<(u32, u32, DXGI_FORMAT)>,
    pooled_dev: Option<PooledWgcDevice>,
    frame_signal: Arc<(Mutex<u64>, Condvar)>,
    last_seen_seq: u64,
    last_bgra: Option<(u32, u32, Vec<u8>)>,
    // MUST be the last field so `RoUninitialize()` runs AFTER all WinRT/COM objects drop!
    _ro_guard: RoGuard,
}

impl WgcMonitorSession {
    /// Start a free-threaded WGC capture session targeting `hmonitor`, reusing a pooled D3D11 device.
    pub fn start(hmonitor: isize, capture_cursor: bool) -> Result<Self, CaptureError> {
        if hmonitor == 0 {
            return Err(CaptureError::InvalidBounds);
        }

        let ro_guard = RoGuard::init();
        Self::start_inner(hmonitor, capture_cursor, ro_guard)
    }

    fn start_inner(
        hmonitor: isize,
        capture_cursor: bool,
        ro_guard: RoGuard,
    ) -> Result<Self, CaptureError> {
        let dev = checkout_wgc_device()?;

        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| {
            CaptureError::Dxgi {
                api: "factory<IGraphicsCaptureItemInterop>",
                hresult: e.code().0,
            }
        })?;

        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForMonitor(HMONITOR(hmonitor as *mut c_void)) }.map_err(
                |e| CaptureError::Dxgi {
                    api: "IGraphicsCaptureItemInterop::CreateForMonitor",
                    hresult: e.code().0,
                },
            )?;

        let size = item.Size().map_err(|e| CaptureError::Dxgi {
            api: "GraphicsCaptureItem::Size",
            hresult: e.code().0,
        })?;
        if size.Width <= 0 || size.Height <= 0 {
            checkin_wgc_device(dev);
            return Err(CaptureError::InvalidBounds);
        }

        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &dev.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| CaptureError::Dxgi {
            api: "Direct3D11CaptureFramePool::CreateFreeThreaded",
            hresult: e.code().0,
        })?;

        let session = match frame_pool.CreateCaptureSession(&item) {
            Ok(s) => s,
            Err(e) => {
                let _ = frame_pool.Close();
                checkin_wgc_device(dev);
                return Err(CaptureError::Dxgi {
                    api: "Direct3D11CaptureFramePool::CreateCaptureSession",
                    hresult: e.code().0,
                });
            }
        };

        // Best-effort hide the yellow capture border & configure cursor visibility.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);

        let frame_signal = Arc::new((Mutex::new(0u64), Condvar::new()));
        let signal_cb = Arc::clone(&frame_signal);
        let handler = TypedEventHandler::<
            Direct3D11CaptureFramePool,
            windows::core::IInspectable,
        >::new(move |_pool, _args| {
            let (lock, cvar) = &*signal_cb;
            if let Ok(mut seq) = lock.lock() {
                *seq = seq.wrapping_add(1);
                cvar.notify_all();
            }
            Ok(())
        });

        if let Err(e) = frame_pool.FrameArrived(&handler) {
            let _ = session.Close();
            let _ = frame_pool.Close();
            checkin_wgc_device(dev);
            return Err(CaptureError::Dxgi {
                api: "Direct3D11CaptureFramePool::FrameArrived",
                hresult: e.code().0,
            });
        }

        if let Err(e) = session.StartCapture() {
            let _ = session.Close();
            let _ = frame_pool.Close();
            checkin_wgc_device(dev);
            return Err(CaptureError::Dxgi {
                api: "GraphicsCaptureSession::StartCapture",
                hresult: e.code().0,
            });
        }

        Ok(Self {
            session,
            frame_pool,
            _item: item,
            staging: None,
            staging_desc: None,
            pooled_dev: Some(dev),
            frame_signal,
            last_seen_seq: 0,
            last_bgra: None,
            _ro_guard: ro_guard,
        })
    }

    /// Acquire the next WGC frame directly as **native `BGRA8`** (`memcpy` per scanline, zero
    /// per-pixel channel swizzle — ready to hand straight into GPUI DX11 `RenderImage`!).
    pub fn acquire_frame_bgra(
        &mut self,
        timeout_ms: u32,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        self.acquire_frame_internal(timeout_ms, false)
    }

    /// Wait up to `timeout_ms` for the next available WGC frame and return `(width, height, rgba8)`.
    pub fn acquire_frame_rgba(
        &mut self,
        timeout_ms: u32,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        self.acquire_frame_internal(timeout_ms, true)
    }

    fn acquire_frame_internal(
        &mut self,
        timeout_ms: u32,
        swizzle_to_rgba: bool,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);

        let mut latest_frame = self.drain_latest_queued_frame();
        while latest_frame.is_none() {
            let now = Instant::now();
            if now >= deadline {
                return Err(CaptureError::WaitTimeout);
            }
            let remaining = deadline.saturating_duration_since(now);
            let (lock, cvar) = &*self.frame_signal;
            if let Ok(guard) = lock.lock() {
                if *guard == self.last_seen_seq {
                    let (guard, _timed_out) = cvar
                        .wait_timeout(guard, remaining)
                        .unwrap_or_else(|e| e.into_inner());
                    self.last_seen_seq = *guard;
                } else {
                    self.last_seen_seq = *guard;
                }
            }
            latest_frame = self.drain_latest_queued_frame();
        }

        let frame = latest_frame.ok_or(CaptureError::WaitTimeout)?;
        let surface = frame.Surface().map_err(|e| CaptureError::Dxgi {
            api: "Direct3D11CaptureFrame::Surface",
            hresult: e.code().0,
        })?;
        let access: IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|e| CaptureError::Dxgi {
                api: "IDirect3DSurface::cast<IDirect3DDxgiInterfaceAccess>",
                hresult: e.code().0,
            })?;
        let tex: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(|e| {
            CaptureError::Dxgi {
                api: "IDirect3DDxgiInterfaceAccess::GetInterface<ID3D11Texture2D>",
                hresult: e.code().0,
            }
        })?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };

        let content_size = frame.ContentSize().ok();
        let copy_w = content_size
            .map(|s| (s.Width.max(1) as u32).min(desc.Width))
            .unwrap_or(desc.Width);
        let copy_h = content_size
            .map(|s| (s.Height.max(1) as u32).min(desc.Height))
            .unwrap_or(desc.Height);

        let dev = self.pooled_dev.as_ref().ok_or(CaptureError::Dxgi {
            api: "WGC::pooled_dev",
            hresult: -1,
        })?;

        let key = (desc.Width, desc.Height, desc.Format);
        if self.staging_desc != Some(key) {
            let mut staging_desc = desc;
            staging_desc.Usage = D3D11_USAGE_STAGING;
            staging_desc.BindFlags = 0;
            staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            staging_desc.MiscFlags = 0;
            staging_desc.MipLevels = 1;
            staging_desc.ArraySize = 1;

            let mut staging = None;
            unsafe {
                dev.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            }
            .map_err(|e| CaptureError::Dxgi {
                api: "WGC::CreateTexture2D(staging)",
                hresult: e.code().0,
            })?;
            self.staging = staging;
            self.staging_desc = Some(key);
        }

        let staging = self.staging.as_ref().unwrap();
        unsafe { dev.context.CopyResource(staging, &tex) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            dev.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| CaptureError::Dxgi {
            api: "WGC::ID3D11DeviceContext::Map",
            hresult: e.code().0,
        })?;

        let w = copy_w as usize;
        let h = copy_h as usize;
        let row_bytes = w * 4;
        let row_pitch = mapped.RowPitch as usize;
        let src_len = row_pitch * h;
        let src = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, src_len) };

        let mut out = vec![0u8; row_bytes * h];
        if swizzle_to_rgba {
            for y in 0..h {
                let srow = &src[y * row_pitch..y * row_pitch + row_bytes];
                let drow = &mut out[y * row_bytes..(y + 1) * row_bytes];
                for (s, d) in srow.chunks_exact(4).zip(drow.chunks_exact_mut(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
        } else if row_pitch == row_bytes {
            // Single contiguous memcpy for the entire monitor! DWM monitor surfaces already carry A=255.
            out.copy_from_slice(&src[..row_bytes * h]);
        } else {
            for y in 0..h {
                let srow = &src[y * row_pitch..y * row_pitch + row_bytes];
                let drow = &mut out[y * row_bytes..(y + 1) * row_bytes];
                drow.copy_from_slice(srow);
            }
        }

        unsafe { dev.context.Unmap(staging, 0) };
        let _ = frame.Close();

        if !swizzle_to_rgba {
            self.last_bgra = Some((copy_w, copy_h, out.clone()));
        }

        Ok((copy_w, copy_h, out))
    }

    fn drain_latest_queued_frame(
        &self,
    ) -> Option<windows::Graphics::Capture::Direct3D11CaptureFrame> {
        let mut latest = None;
        while let Ok(next) = self.frame_pool.TryGetNextFrame() {
            if let Some(prev) = latest.replace(next) {
                let _ = prev.Close();
            }
        }
        latest
    }
}

impl Drop for WgcMonitorSession {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
        self.staging = None;
        if let Some(dev) = self.pooled_dev.take() {
            checkin_wgc_device(dev);
        }
    }
}

// SAFETY: `WgcMonitorSession` uses `Direct3D11CaptureFramePool::CreateFreeThreaded` and agile
// MTA WinRT/D3D11 objects, and each session is checked out exclusively to a single thread at a time.
unsafe impl Send for WgcMonitorSession {}

static WARM_SESSION_POOL: OnceLock<Mutex<HashMap<(isize, bool), WgcMonitorSession>>> =
    OnceLock::new();

fn session_pool() -> &'static Mutex<HashMap<(isize, bool), WgcMonitorSession>> {
    WARM_SESSION_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn checkout_warm_session(hmonitor: isize, capture_cursor: bool) -> Option<WgcMonitorSession> {
    session_pool()
        .lock()
        .ok()
        .and_then(|mut map| map.remove(&(hmonitor, capture_cursor)))
}

fn checkin_warm_session(hmonitor: isize, capture_cursor: bool, session: WgcMonitorSession) {
    if let Ok(mut map) = session_pool().lock() {
        if map.len() < 8 {
            map.insert((hmonitor, capture_cursor), session);
        }
    }
}

/// Pre-warm persistent free-threaded WGC monitor sessions for all active monitors on a background
/// thread so clicking `Capture` grabs the latest VRAM frame in **`< 15ms`** (`0ms` DWM session setup,
/// `0ms` staging texture allocation)!
pub fn prewarm_wgc_monitor_sessions() {
    let Ok(topo) = snagsoup_win_platform::dpi::enumerate_monitors() else {
        return;
    };
    std::thread::scope(|s| {
        for mon in &topo.monitors {
            let hm = mon.hmonitor;
            s.spawn(move || {
                let already = session_pool()
                    .lock()
                    .ok()
                    .is_some_and(|m| m.contains_key(&(hm, false)));
                if !already {
                    if let Ok(mut sess) = WgcMonitorSession::start(hm, false) {
                        let _ = sess.acquire_frame_bgra(150);
                        checkin_warm_session(hm, false, sess);
                    }
                }
            });
        }
    });
}

/// Capture a single **BGRA8** frame of `hmonitor` using a persistent warm `WgcMonitorSession`
/// (creating & caching one if not yet warm, or recreating transparently if display mode changed).
pub fn capture_monitor_wgc_bgra_timeout(
    hmonitor: isize,
    bounds: PhysicalRect,
    capture_cursor: bool,
    timeout_ms: u32,
) -> Result<CapturedFrame, CaptureError> {
    if bounds.width() <= 0 || bounds.height() <= 0 || hmonitor == 0 {
        return Err(CaptureError::InvalidBounds);
    }
    let expected_w = bounds.width() as u32;
    let expected_h = bounds.height() as u32;
    let started = Instant::now();

    if let Some(mut warm) = checkout_warm_session(hmonitor, capture_cursor) {
        match warm.acquire_frame_bgra(18) {
            Ok((width, height, bgra)) if width == expected_w && height == expected_h => {
                checkin_warm_session(hmonitor, capture_cursor, warm);
                return Ok(CapturedFrame {
                    bounds,
                    width,
                    height,
                    rgba: bgra,
                    backend: CaptureBackend::WindowsGraphicsCapture,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                });
            }
            Err(CaptureError::WaitTimeout) => {
                // Static monitor: DWM only pushes frames when >= 1 pixel changes! Return the
                // unchanged cached frame in <2ms and keep the warm session alive in the pool.
                if let Some((cw, ch, ref cached)) = warm.last_bgra {
                    if cw == expected_w && ch == expected_h {
                        let bgra = cached.clone();
                        checkin_warm_session(hmonitor, capture_cursor, warm);
                        return Ok(CapturedFrame {
                            bounds,
                            width: cw,
                            height: ch,
                            rgba: bgra,
                            backend: CaptureBackend::WindowsGraphicsCapture,
                            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                        });
                    }
                }
            }
            _ => {}
        }
        // Display resolution changed or session invalidated — drop stale session and recreate below
        drop(warm);
    }

    let mut fresh = WgcMonitorSession::start(hmonitor, capture_cursor)?;
    let (width, height, bgra) = fresh.acquire_frame_bgra(timeout_ms)?;
    checkin_warm_session(hmonitor, capture_cursor, fresh);
    Ok(CapturedFrame {
        bounds,
        width,
        height,
        rgba: bgra,
        backend: CaptureBackend::WindowsGraphicsCapture,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Capture a single RGBA8 frame of `hmonitor` using `Windows.Graphics.Capture`.
pub fn capture_monitor_wgc(
    hmonitor: isize,
    bounds: PhysicalRect,
    capture_cursor: bool,
) -> Result<CapturedFrame, CaptureError> {
    if bounds.width() <= 0 || bounds.height() <= 0 || hmonitor == 0 {
        return Err(CaptureError::InvalidBounds);
    }
    let started = Instant::now();
    let mut session = WgcMonitorSession::start(hmonitor, capture_cursor)?;
    let (width, height, rgba) = session.acquire_frame_rgba(350)?;
    Ok(CapturedFrame {
        bounds,
        width,
        height,
        rgba,
        backend: CaptureBackend::WindowsGraphicsCapture,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}
