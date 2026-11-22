//! High-speed `IDXGIOutputDuplication` capture across ALL DXGI adapters, backed by a
//! reusable `ID3D11Device` and staging `ID3D11Texture2D` per output, with HDR
//! `R16G16B16A16_FLOAT` tonemapping, `DXGI_ERROR_ACCESS_LOST` handling, and black-frame
//! / `LastPresentTime == 0` rejection so callers cleanly fall back to WGC or GDI.

use crate::tonemap::tonemap_scrgb_f16_to_rgba8;
use crate::{CaptureBackend, CaptureError, CapturedFrame, is_frame_all_black};
use snagsoup_win_platform::dpi::PhysicalRect;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
};
use windows::core::Interface;

struct OutputDuplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    hmonitor: isize,
    bounds: PhysicalRect,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    staging_desc: Option<(u32, u32, DXGI_FORMAT)>,
    last_rgba: Option<Vec<u8>>,
}

/// Reusable DXGI Desktop Duplication capturer across all outputs on all DXGI adapters.
pub struct DxgiCapturer {
    primary_device: ID3D11Device,
    outputs: Vec<OutputDuplicator>,
}

impl DxgiCapturer {
    /// Initialize `ID3D11Device` and `IDXGIOutputDuplication` for every active output across all adapters.
    pub fn new() -> Result<Self, CaptureError> {
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.map_err(|e| CaptureError::Dxgi {
                api: "CreateDXGIFactory1",
                hresult: e.code().0,
            })?;

        let feature_levels = [D3D_FEATURE_LEVEL_11_0];
        let mut outputs = Vec::new();
        let mut primary_device: Option<ID3D11Device> = None;

        let mut adapter_idx = 0u32;
        loop {
            let adapter1 = match unsafe { factory.EnumAdapters1(adapter_idx) } {
                Ok(a) => {
                    adapter_idx += 1;
                    a
                }
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };

            let Ok(adapter_base) = adapter1.cast::<IDXGIAdapter>() else {
                continue;
            };

            // Check if this adapter has at least one desktop-attached output before creating a device.
            let mut has_attached = false;
            let mut probe_idx = 0u32;
            while let Ok(out) = unsafe { adapter1.EnumOutputs(probe_idx) } {
                probe_idx += 1;
                if let Ok(desc) = unsafe { out.GetDesc() } {
                    if desc.AttachedToDesktop.as_bool() {
                        has_attached = true;
                        break;
                    }
                }
            }
            if !has_attached {
                continue;
            }

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let create_res = unsafe {
                D3D11CreateDevice(
                    &adapter_base,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            };
            if create_res.is_err() {
                continue;
            }
            let (Some(device), Some(context)) = (device, context) else {
                continue;
            };

            if primary_device.is_none() {
                primary_device = Some(device.clone());
            }

            let mut out_idx = 0u32;
            loop {
                match unsafe { adapter1.EnumOutputs(out_idx) } {
                    Ok(output) => {
                        out_idx += 1;
                        let Ok(desc) = (unsafe { output.GetDesc() }) else {
                            continue;
                        };
                        if !desc.AttachedToDesktop.as_bool() {
                            continue;
                        }
                        let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                            continue;
                        };
                        if let Ok(duplication) = unsafe { output1.DuplicateOutput(&device) } {
                            let rc = desc.DesktopCoordinates;
                            outputs.push(OutputDuplicator {
                                device: device.clone(),
                                context: context.clone(),
                                hmonitor: desc.Monitor.0 as isize,
                                bounds: PhysicalRect::new(rc.left, rc.top, rc.right, rc.bottom),
                                duplication,
                                staging: None,
                                staging_desc: None,
                                last_rgba: None,
                            });
                        }
                    }
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(_) => break,
                }
            }
        }

        if outputs.is_empty() {
            return Err(CaptureError::Dxgi {
                api: "DuplicateOutput",
                hresult: DXGI_ERROR_NOT_FOUND.0,
            });
        }

        let primary_device = primary_device.unwrap_or_else(|| outputs[0].device.clone());
        Ok(Self {
            primary_device,
            outputs,
        })
    }

    /// Borrow the primary `ID3D11Device` (e.g. for `reclaim_process_memory(Some(dev))`).
    pub fn d3d_device(&self) -> &ID3D11Device {
        &self.primary_device
    }

    /// Number of duplicated outputs across all adapters.
    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    /// Capture a single output by index.
    pub fn capture_output(
        &mut self,
        output_idx: usize,
        timeout_ms: u32,
    ) -> Result<CapturedFrame, CaptureError> {
        let started = Instant::now();
        let out = self
            .outputs
            .get_mut(output_idx)
            .ok_or(CaptureError::InvalidBounds)?;
        let rgba = Self::acquire_output_rgba(out, timeout_ms)?;
        Ok(CapturedFrame {
            bounds: out.bounds,
            width: out.bounds.width() as u32,
            height: out.bounds.height() as u32,
            rgba,
            backend: CaptureBackend::DxgiDuplication,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Capture the duplicated output matching `hmonitor` (or `bounds`), rejecting black / uninitialized frames.
    pub fn capture_by_monitor(
        &mut self,
        hmonitor: isize,
        bounds: PhysicalRect,
        timeout_ms: u32,
    ) -> Result<CapturedFrame, CaptureError> {
        let idx = self
            .outputs
            .iter()
            .position(|o| (hmonitor != 0 && o.hmonitor == hmonitor) || o.bounds == bounds)
            .ok_or(CaptureError::InvalidBounds)?;
        self.capture_output(idx, timeout_ms)
    }

    /// Capture and composite all duplicated outputs into a single virtual-desktop `CapturedFrame`.
    pub fn capture_virtual_desktop(&mut self, timeout_ms: u32) -> Result<CapturedFrame, CaptureError> {
        let started = Instant::now();
        if self.outputs.len() == 1 {
            return self.capture_output(0, timeout_ms);
        }

        let mut virtual_bounds = self.outputs[0].bounds;
        for o in &self.outputs[1..] {
            virtual_bounds = virtual_bounds.union(&o.bounds);
        }
        let vw = virtual_bounds.width() as usize;
        let vh = virtual_bounds.height() as usize;
        let mut composite = vec![0u8; vw * vh * 4];

        for i in 0..self.outputs.len() {
            let out = &mut self.outputs[i];
            let ob = out.bounds;
            let rgba = Self::acquire_output_rgba(out, timeout_ms)?;
            let ow = ob.width() as usize;
            let oh = ob.height() as usize;
            let dst_x = (ob.left - virtual_bounds.left) as usize;
            let dst_y = (ob.top - virtual_bounds.top) as usize;

            for y in 0..oh {
                let src_row = &rgba[y * ow * 4..(y + 1) * ow * 4];
                let dst_off = ((dst_y + y) * vw + dst_x) * 4;
                composite[dst_off..dst_off + ow * 4].copy_from_slice(src_row);
            }
        }

        Ok(CapturedFrame {
            bounds: virtual_bounds,
            width: vw as u32,
            height: vh as u32,
            rgba: composite,
            backend: CaptureBackend::DxgiDuplication,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn acquire_output_rgba(
        out: &mut OutputDuplicator,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, CaptureError> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut desktop_resource = None;

        let effective_timeout = if out.last_rgba.is_none() {
            timeout_ms.max(100)
        } else {
            timeout_ms
        };

        match unsafe {
            out.duplication
                .AcquireNextFrame(effective_timeout, &mut frame_info, &mut desktop_resource)
        } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                if let Some(ref cached) = out.last_rgba {
                    return Ok(cached.clone());
                }
                return Err(CaptureError::WaitTimeout);
            }
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                return Err(CaptureError::AccessLost);
            }
            Err(e) => {
                return Err(CaptureError::Dxgi {
                    api: "AcquireNextFrame",
                    hresult: e.code().0,
                });
            }
        }

        // Reject uninitialized / metadata-only frames where LastPresentTime == 0.
        if frame_info.LastPresentTime == 0 {
            let _ = unsafe { out.duplication.ReleaseFrame() };
            if let Some(ref cached) = out.last_rgba {
                return Ok(cached.clone());
            }
            return Err(CaptureError::Dxgi {
                api: "AcquireNextFrame(LastPresentTime==0)",
                hresult: -1,
            });
        }

        // Always release the DXGI frame before returning.
        let copy_res = (|| -> Result<Vec<u8>, CaptureError> {
            let res = desktop_resource.ok_or(CaptureError::Dxgi {
                api: "AcquireNextFrame(null resource)",
                hresult: -1,
            })?;
            let tex: ID3D11Texture2D = res.cast().map_err(|e| CaptureError::Dxgi {
                api: "IDXGIResource::cast<ID3D11Texture2D>",
                hresult: e.code().0,
            })?;

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { tex.GetDesc(&mut desc) };

            let key = (desc.Width, desc.Height, desc.Format);
            if out.staging_desc != Some(key) {
                let mut staging_desc = desc;
                staging_desc.Usage = D3D11_USAGE_STAGING;
                staging_desc.BindFlags = 0;
                staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                staging_desc.MiscFlags = 0;
                staging_desc.MipLevels = 1;
                staging_desc.ArraySize = 1;

                let mut staging = None;
                unsafe {
                    out.device
                        .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                }
                .map_err(|e| CaptureError::Dxgi {
                    api: "CreateTexture2D(staging)",
                    hresult: e.code().0,
                })?;
                out.staging = staging;
                out.staging_desc = Some(key);
            }

            let staging = out.staging.as_ref().unwrap();
            unsafe { out.context.CopyResource(staging, &tex) };

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                out.context
                    .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            }
            .map_err(|e| CaptureError::Dxgi {
                api: "ID3D11DeviceContext::Map",
                hresult: e.code().0,
            })?;

            let width = desc.Width;
            let height = desc.Height;
            let row_pitch = mapped.RowPitch as usize;
            let src_len = row_pitch * (height as usize);
            let src = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, src_len) };

            let rgba = match desc.Format {
                DXGI_FORMAT_B8G8R8A8_UNORM => {
                    let w = width as usize;
                    let h = height as usize;
                    let mut buf = vec![0u8; w * h * 4];
                    for y in 0..h {
                        let srow = &src[y * row_pitch..y * row_pitch + w * 4];
                        let drow = &mut buf[y * w * 4..(y + 1) * w * 4];
                        for (s, d) in srow.chunks_exact(4).zip(drow.chunks_exact_mut(4)) {
                            d[0] = s[2];
                            d[1] = s[1];
                            d[2] = s[0];
                            d[3] = 255;
                        }
                    }
                    buf
                }
                DXGI_FORMAT_R8G8B8A8_UNORM => {
                    let w = width as usize;
                    let h = height as usize;
                    let mut buf = vec![0u8; w * h * 4];
                    for y in 0..h {
                        let srow = &src[y * row_pitch..y * row_pitch + w * 4];
                        let drow = &mut buf[y * w * 4..(y + 1) * w * 4];
                        drow.copy_from_slice(srow);
                        for d in drow.chunks_exact_mut(4) {
                            d[3] = 255;
                        }
                    }
                    buf
                }
                DXGI_FORMAT_R16G16B16A16_FLOAT => {
                    tonemap_scrgb_f16_to_rgba8(src, width, height, row_pitch, 80.0)
                }
                other => {
                    unsafe { out.context.Unmap(staging, 0) };
                    return Err(CaptureError::UnsupportedFormat(other.0 as u32));
                }
            };

            unsafe { out.context.Unmap(staging, 0) };
            Ok(rgba)
        })();

        let _ = unsafe { out.duplication.ReleaseFrame() };

        let rgba = copy_res?;
        if is_frame_all_black(&rgba) {
            return Err(CaptureError::Dxgi {
                api: "AcquireNextFrame(all-black frame)",
                hresult: -1,
            });
        }
        out.last_rgba = Some(rgba.clone());
        Ok(rgba)
    }
}
