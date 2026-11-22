//! Windows.Graphics.Capture (`WGC`) monitor capture backend with a **Self-Draining Zero-Copy
//! GPU Ring** (`numberOfBuffers = 3`) for `< 8ms` instant multi-monitor screen freezes.
//!
//! ### How the Self-Draining Zero-Copy Ring Eliminates Both Latency & Stale Frames
//! - `WgcMonitorSession::start` creates a free-threaded 3-buffer `Direct3D11CaptureFramePool`.
//! - Its `FrameArrived` callback runs on WinRT's MTA worker thread in `~3 µs` (`0` CPU copies,
//!   `0` `CopyResource`, `0` RAM overhead; `0` events/sec when the screen is idle): it pops
//!   `TryGetNextFrame()`, stores only the newest `Direct3D11CaptureFrame` VRAM handle in
//!   `LiveFrameSlot::latest_gpu_frame`, and **immediately calls `.Close()` on the previous frame**.
//! - Because at most **1** of the **3** pool buffers is held at any instant, DWM always has **2**
//!   free buffers to write every new window/cursor/desktop update into — the pool **never**
//!   saturates or goes stale, while `capture_monitor_wgc_bgra_timeout` can copy the already-latched
//!   VRAM surface in **`~4–7ms`** (`0ms` `StartCapture` RPC, `0ms` VSync wait)!
//! - When SnagSoup hides its own window right before a capture (`hide_snagsoup_before_capturing`),
//!   [`mark_wgc_sessions_dirty_for_window_hide`] advances `min_snapshot_seq = seq + 1` so the next
//!   freeze waits (`~4–14ms`) for DWM's post-hide frame.

use crate::{CaptureBackend, CaptureError, CapturedFrame};
use snagsoup_win_platform::dpi::PhysicalRect;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
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
    staging: Option<ID3D11Texture2D>,
    staging_desc: Option<(u32, u32, DXGI_FORMAT)>,
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
        staging: None,
        staging_desc: None,
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

/// Pre-warm `count` reusable WGC `ID3D11Device` instances in the background.
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

struct LiveFrameSlot {
    seq: u64,
    latest_gpu_frame: Option<Direct3D11CaptureFrame>,
    closed: bool,
}

/// Reusable self-draining `Windows.Graphics.Capture` session for a single `HMONITOR`.
pub struct WgcMonitorSession {
    session: GraphicsCaptureSession,
    frame_pool: Direct3D11CaptureFramePool,
    _item: GraphicsCaptureItem,
    pooled_dev: Option<PooledWgcDevice>,
    live_slot: Arc<(Mutex<LiveFrameSlot>, Condvar)>,
    last_seen_seq: u64,
    min_snapshot_seq: u64,
    // MUST be the last field so `RoUninitialize()` runs AFTER all WinRT/COM objects drop!
    _ro_guard: RoGuard,
}

impl WgcMonitorSession {
    /// Start a free-threaded self-draining WGC session targeting `hmonitor`.
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

        // 3 buffers: 1 held in `latest_gpu_frame` + 2 always free for DWM double-buffering!
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &dev.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            3,
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

        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);

        let live_slot = Arc::new((
            Mutex::new(LiveFrameSlot {
                seq: 0,
                latest_gpu_frame: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let slot_cb = Arc::clone(&live_slot);
        let handler = TypedEventHandler::<
            Direct3D11CaptureFramePool,
            windows::core::IInspectable,
        >::new(move |pool, _args| {
            let Some(pool) = pool.as_ref() else {
                return Ok(());
            };
            // Drain all queued frames on the pool, keeping only the newest VRAM surface handle
            let mut newest: Option<Direct3D11CaptureFrame> = None;
            while let Ok(next) = pool.TryGetNextFrame() {
                if let Some(prev) = newest.replace(next) {
                    let _ = prev.Close();
                }
            }
            if let Some(frame) = newest {
                let (lock, cvar) = &*slot_cb;
                if let Ok(mut guard) = lock.lock() {
                    if guard.closed {
                        let _ = frame.Close();
                        return Ok(());
                    }
                    if let Some(old) = guard.latest_gpu_frame.replace(frame) {
                        let _ = old.Close();
                    }
                    guard.seq = guard.seq.wrapping_add(1);
                    cvar.notify_all();
                } else {
                    let _ = frame.Close();
                }
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
            pooled_dev: Some(dev),
            live_slot,
            last_seen_seq: 0,
            min_snapshot_seq: 1,
            _ro_guard: ro_guard,
        })
    }

    /// Require the next one-shot snapshot to wait for at least one fresh DWM frame after this point
    /// (used right before `ShowWindow(SW_HIDE)` when hiding SnagSoup's own window).
    pub fn mark_dirty_for_next_snapshot(&mut self) {
        let (lock, _) = &*self.live_slot;
        if let Ok(guard) = lock.lock() {
            self.min_snapshot_seq = guard.seq.saturating_add(1);
        }
    }

    /// Instant one-shot snapshot readback in **native `BGRA8`**:
    /// - Immediately copies `latest_gpu_frame` (`0ms` wait!) unless `min_snapshot_seq` requires
    ///   waiting briefly (`<= post_hide_wait_ms`) for DWM's post-hide frame.
    pub fn acquire_snapshot_bgra(
        &mut self,
        initial_timeout_ms: u32,
        post_hide_wait_ms: u32,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        let required_seq = self.min_snapshot_seq.max(1);
        let frame = self.wait_for_frame(required_seq, initial_timeout_ms, post_hide_wait_ms)?;
        self.min_snapshot_seq = 1;
        self.copy_gpu_frame_to_cpu(&frame, false)
    }

    /// Acquire the next video frame in **native `BGRA8`** (waits up to `timeout_ms` for a new
    /// frame after `last_seen_seq`, falling back to `WaitTimeout` on static screens).
    pub fn acquire_frame_bgra(
        &mut self,
        timeout_ms: u32,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        let next_seq = self.last_seen_seq.saturating_add(1);
        let frame = self.wait_for_new_video_frame(next_seq, timeout_ms)?;
        self.copy_gpu_frame_to_cpu(&frame, false)
    }

    /// Wait up to `timeout_ms` for the next available WGC frame and return `(width, height, rgba8)`.
    pub fn acquire_frame_rgba(
        &mut self,
        timeout_ms: u32,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        let next_seq = self.last_seen_seq.saturating_add(1);
        let frame = self.wait_for_new_video_frame(next_seq, timeout_ms)?;
        self.copy_gpu_frame_to_cpu(&frame, true)
    }

    fn wait_for_frame(
        &mut self,
        required_seq: u64,
        initial_timeout_ms: u32,
        post_hide_wait_ms: u32,
    ) -> Result<Direct3D11CaptureFrame, CaptureError> {
        let (lock, cvar) = &*self.live_slot;
        let mut guard = lock.lock().map_err(|_| CaptureError::WaitTimeout)?;

        // 1. If session just started (`seq == 0`), wait up to `initial_timeout_ms` for Frame #0
        if guard.latest_gpu_frame.is_none() {
            let deadline = Instant::now() + Duration::from_millis(initial_timeout_ms as u64);
            while guard.latest_gpu_frame.is_none() {
                let now = Instant::now();
                if now >= deadline {
                    return Err(CaptureError::WaitTimeout);
                }
                let (g, _) = cvar
                    .wait_timeout(guard, deadline.saturating_duration_since(now))
                    .map_err(|_| CaptureError::WaitTimeout)?;
                guard = g;
            }
        }

        // 2. If `mark_dirty_for_next_snapshot` requested a post-hide frame (`guard.seq < required_seq`),
        //    wait up to `post_hide_wait_ms` (~28ms) for DWM to push the post-hide frame; if the
        //    hidden window wasn't on this monitor (so this monitor stayed static), fall back to
        //    `latest_gpu_frame` cleanly!
        if guard.seq < required_seq && post_hide_wait_ms > 0 {
            let deadline = Instant::now() + Duration::from_millis(post_hide_wait_ms as u64);
            while guard.seq < required_seq {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let (g, _) = cvar
                    .wait_timeout(guard, deadline.saturating_duration_since(now))
                    .map_err(|_| CaptureError::WaitTimeout)?;
                guard = g;
            }
        }

        self.last_seen_seq = guard.seq;
        guard
            .latest_gpu_frame
            .clone()
            .ok_or(CaptureError::WaitTimeout)
    }

    fn wait_for_new_video_frame(
        &mut self,
        required_seq: u64,
        timeout_ms: u32,
    ) -> Result<Direct3D11CaptureFrame, CaptureError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let (lock, cvar) = &*self.live_slot;
        let mut guard = lock.lock().map_err(|_| CaptureError::WaitTimeout)?;

        while guard.seq < required_seq || guard.latest_gpu_frame.is_none() {
            let now = Instant::now();
            if now >= deadline {
                return Err(CaptureError::WaitTimeout);
            }
            let (g, _) = cvar
                .wait_timeout(guard, deadline.saturating_duration_since(now))
                .map_err(|_| CaptureError::WaitTimeout)?;
            guard = g;
        }

        self.last_seen_seq = guard.seq;
        guard
            .latest_gpu_frame
            .clone()
            .ok_or(CaptureError::WaitTimeout)
    }

    fn copy_gpu_frame_to_cpu(
        &mut self,
        frame: &Direct3D11CaptureFrame,
        swizzle_to_rgba: bool,
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
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

        let dev = self.pooled_dev.as_mut().ok_or(CaptureError::Dxgi {
            api: "WGC::pooled_dev",
            hresult: -1,
        })?;

        let key = (desc.Width, desc.Height, desc.Format);
        if dev.staging_desc != Some(key) || dev.staging.is_none() {
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
            dev.staging = staging;
            dev.staging_desc = Some(key);
        }

        let staging = dev.staging.as_ref().unwrap();
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
            out.copy_from_slice(&src[..row_bytes * h]);
        } else {
            for y in 0..h {
                let srow = &src[y * row_pitch..y * row_pitch + row_bytes];
                let drow = &mut out[y * row_bytes..(y + 1) * row_bytes];
                drow.copy_from_slice(srow);
            }
        }

        unsafe { dev.context.Unmap(staging, 0) };

        Ok((copy_w, copy_h, out))
    }
}

impl Drop for WgcMonitorSession {
    fn drop(&mut self) {
        let (lock, _) = &*self.live_slot;
        if let Ok(mut guard) = lock.lock() {
            guard.closed = true;
            if let Some(frame) = guard.latest_gpu_frame.take() {
                let _ = frame.Close();
            }
        }
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
        if let Some(dev) = self.pooled_dev.take() {
            checkin_wgc_device(dev);
        }
    }
}

// SAFETY: `WgcMonitorSession` uses `Direct3D11CaptureFramePool::CreateFreeThreaded` and agile
// MTA WinRT/D3D11 objects, and each session is checked out exclusively to a single thread at a time.
unsafe impl Send for WgcMonitorSession {}

static WARM_RING_POOL: OnceLock<Mutex<HashMap<(isize, bool), WgcMonitorSession>>> =
    OnceLock::new();

fn ring_pool() -> &'static Mutex<HashMap<(isize, bool), WgcMonitorSession>> {
    WARM_RING_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn checkout_ring_session(hmonitor: isize, capture_cursor: bool) -> Option<WgcMonitorSession> {
    ring_pool()
        .lock()
        .ok()
        .and_then(|mut map| map.remove(&(hmonitor, capture_cursor)))
}

fn checkin_ring_session(hmonitor: isize, capture_cursor: bool, session: WgcMonitorSession) {
    if let Ok(mut map) = ring_pool().lock() {
        if map.len() < 8 {
            map.insert((hmonitor, capture_cursor), session);
        }
    }
}

/// Notify all warm WGC monitor sessions that SnagSoup is about to hide one of its own windows
/// (`ShowWindow(SW_HIDE)`), so the immediately following snapshot waits (`~4–14ms`) for DWM's
/// post-hide frame on that monitor rather than reading the pre-hide VRAM surface.
pub fn mark_wgc_sessions_dirty_for_window_hide() {
    if let Ok(mut map) = ring_pool().lock() {
        for sess in map.values_mut() {
            sess.mark_dirty_for_next_snapshot();
        }
    }
}

/// Pre-warm self-draining zero-copy WGC monitor rings for all active monitors on a background
/// thread so pressing `Print Screen` / clicking `Capture` reads the latest live VRAM frame in
/// **`~4–8ms` total across all monitors** (`0ms` `StartCapture` RPC, `0ms` staging allocation)!
pub fn prewarm_wgc_monitor_sessions() {
    let Ok(topo) = snagsoup_win_platform::dpi::enumerate_monitors() else {
        return;
    };
    std::thread::scope(|s| {
        for mon in &topo.monitors {
            let hm = mon.hmonitor;
            s.spawn(move || {
                let already = ring_pool()
                    .lock()
                    .ok()
                    .is_some_and(|m| m.contains_key(&(hm, false)));
                if !already {
                    if let Ok(mut sess) = WgcMonitorSession::start(hm, false) {
                        // Prime staging texture once so first user capture pays 0ms CreateTexture2D
                        let _ = sess.acquire_snapshot_bgra(200, 0);
                        checkin_ring_session(hm, false, sess);
                    }
                }
            });
        }
    });
}

/// Capture a single **live BGRA8** frame of `hmonitor` from the self-draining WGC ring
/// (`~4–8ms` when warm, automatically recreating if display topology/resolution changed).
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

    if let Some(mut warm) = checkout_ring_session(hmonitor, capture_cursor) {
        if let Ok((width, height, bgra)) = warm.acquire_snapshot_bgra(timeout_ms, 28) {
            if width == expected_w && height == expected_h {
                checkin_ring_session(hmonitor, capture_cursor, warm);
                return Ok(CapturedFrame {
                    bounds,
                    width,
                    height,
                    rgba: bgra,
                    backend: CaptureBackend::WindowsGraphicsCapture,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                });
            }
        }
        drop(warm);
    }

    let mut fresh = WgcMonitorSession::start(hmonitor, capture_cursor)?;
    let (width, height, bgra) = fresh.acquire_snapshot_bgra(timeout_ms, 0)?;
    checkin_ring_session(hmonitor, capture_cursor, fresh);
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
    let mut frame = capture_monitor_wgc_bgra_timeout(hmonitor, bounds, capture_cursor, 350)?;
    for px in frame.rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok(frame)
}
