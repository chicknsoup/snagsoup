//! Webcam device enumeration (`MFEnumDeviceSources`) and real-time PiP / Fullscreen
//! Webcam compositing (`Circle`, `RoundedSquare`, `Rectangle`, or `FullscreenWebcam`)
//! for live switching mid-recording.

use crate::MediaError;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MFCreateAttributes,
    MFEnumDeviceSources,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::PWSTR;

/// Metadata for a Windows video capture device (Webcam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebcamDeviceInfo {
    pub symbolic_link: String,
    pub name: String,
}

/// Enumerate active video capture devices via Media Foundation `MFEnumDeviceSources`.
pub fn enumerate_webcams() -> Result<Vec<WebcamDeviceInfo>, MediaError> {
    let co_init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let _ = crate::ensure_mf_started();

    let result = (|| -> Result<Vec<WebcamDeviceInfo>, MediaError> {
        let mut attrs: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs, 1) }.map_err(|e| MediaError::MediaFoundation {
            api: "MFCreateAttributes(webcam)",
            hresult: e.code().0,
        })?;
        let attrs = attrs.unwrap();
        unsafe {
            attrs.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
        }
        .map_err(|e| MediaError::MediaFoundation {
            api: "SetGUID(VIDCAP)",
            hresult: e.code().0,
        })?;

        let mut sources_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;
        unsafe { MFEnumDeviceSources(&attrs, &mut sources_ptr, &mut count) }.map_err(|e| {
            MediaError::MediaFoundation {
                api: "MFEnumDeviceSources",
                hresult: e.code().0,
            }
        })?;

        let mut devices = Vec::new();
        if !sources_ptr.is_null() && count > 0 {
            let slice = unsafe { std::slice::from_raw_parts_mut(sources_ptr, count as usize) };
            for slot in slice.iter_mut() {
                if let Some(act) = slot.take() {
                    let name = get_allocated_string(&act, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                        .unwrap_or_else(|| "Webcam".to_string());
                    let symbolic_link = get_allocated_string(
                        &act,
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                    )
                    .unwrap_or_default();
                    devices.push(WebcamDeviceInfo {
                        symbolic_link,
                        name,
                    });
                }
            }
            unsafe { CoTaskMemFree(Some(sources_ptr as *const _)) };
        }

        Ok(devices)
    })();

    if co_init.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

fn get_allocated_string(act: &IMFActivate, key: &windows::core::GUID) -> Option<String> {
    let mut pw = PWSTR::null();
    let mut len = 0u32;
    unsafe { act.GetAllocatedString(key, &mut pw, &mut len).ok()? };
    if pw.is_null() {
        return None;
    }
    let s = unsafe { pw.to_string().ok() };
    unsafe { CoTaskMemFree(Some(pw.0 as *const _)) };
    s
}

/// PiP mask shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PipShape {
    Circle,
    RoundedSquare { radius_ratio: f32 },
    Rectangle,
}

/// PiP screen anchor corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Live webcam presentation mode (toggleable mid-recording!).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebcamOverlayMode {
    Hidden,
    Pip {
        corner: PipCorner,
        shape: PipShape,
        /// Fraction of base frame width (e.g. `0.22`).
        scale: f32,
        margin: u32,
    },
    /// Smoothly switches the recording output to fullscreen webcam.
    FullscreenWebcam,
}

/// Composite a webcam RGBA8 frame onto `base_rgba` according to `mode` (`Hidden`, `Pip`,
/// or `FullscreenWebcam`).
pub fn composite_webcam_pip(
    base_rgba: &mut [u8],
    base_w: u32,
    base_h: u32,
    cam_rgba: &[u8],
    cam_w: u32,
    cam_h: u32,
    mode: WebcamOverlayMode,
) {
    if base_w == 0 || base_h == 0 || cam_w == 0 || cam_h == 0 {
        return;
    }
    match mode {
        WebcamOverlayMode::Hidden => {}
        WebcamOverlayMode::FullscreenWebcam => {
            // Cover-scale webcam frame onto entire base frame.
            let bw = base_w as usize;
            let bh = base_h as usize;
            let cw = cam_w as usize;
            let ch = cam_h as usize;
            for y in 0..bh {
                let sy = (y * ch) / bh;
                for x in 0..bw {
                    let sx = (x * cw) / bw;
                    let s_idx = (sy * cw + sx) * 4;
                    let d_idx = (y * bw + x) * 4;
                    base_rgba[d_idx..d_idx + 4].copy_from_slice(&cam_rgba[s_idx..s_idx + 4]);
                }
            }
        }
        WebcamOverlayMode::Pip {
            corner,
            shape,
            scale,
            margin,
        } => {
            let pip_w = ((base_w as f32 * scale.clamp(0.08, 0.6)).round() as u32).max(16);
            let pip_h = match shape {
                PipShape::Circle | PipShape::RoundedSquare { .. } => pip_w.min(base_h),
                PipShape::Rectangle => {
                    ((pip_w as f32 * cam_h as f32 / cam_w as f32).round() as u32)
                        .clamp(16, base_h)
                }
            };
            let pip_w = pip_w.min(base_w);
            let m = margin.min(base_w.saturating_sub(pip_w)).min(base_h.saturating_sub(pip_h));

            let dst_x0 = match corner {
                PipCorner::TopLeft | PipCorner::BottomLeft => m,
                PipCorner::TopRight | PipCorner::BottomRight => base_w - pip_w - m,
            };
            let dst_y0 = match corner {
                PipCorner::TopLeft | PipCorner::TopRight => m,
                PipCorner::BottomLeft | PipCorner::BottomRight => base_h - pip_h - m,
            };

            let bw = base_w as usize;
            let cw = cam_w as usize;
            let ch = cam_h as usize;
            let pw = pip_w as f32;
            let ph = pip_h as f32;
            let cx = pw * 0.5;
            let cy = ph * 0.5;
            let radius = cx.min(cy);

            for ly in 0..pip_h {
                let sy = ((ly as usize) * ch) / (pip_h as usize);
                let dy = (dst_y0 + ly) as usize;
                for lx in 0..pip_w {
                    let alpha = pip_mask_alpha(lx as f32 + 0.5, ly as f32 + 0.5, pw, ph, cx, cy, radius, shape);
                    if alpha <= 0.0 {
                        continue;
                    }
                    let sx = ((lx as usize) * cw) / (pip_w as usize);
                    let s_idx = (sy * cw + sx) * 4;
                    let d_idx = (dy * bw + (dst_x0 + lx) as usize) * 4;
                    if alpha >= 0.999 {
                        base_rgba[d_idx..d_idx + 4].copy_from_slice(&cam_rgba[s_idx..s_idx + 4]);
                    } else {
                        let inv = 1.0 - alpha;
                        for c in 0..3 {
                            base_rgba[d_idx + c] = (cam_rgba[s_idx + c] as f32 * alpha
                                + base_rgba[d_idx + c] as f32 * inv)
                                .round() as u8;
                        }
                        base_rgba[d_idx + 3] = 255;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pip_mask_alpha(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cx: f32,
    cy: f32,
    r: f32,
    shape: PipShape,
) -> f32 {
    match shape {
        PipShape::Rectangle => 1.0,
        PipShape::Circle => {
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            (r - d).clamp(0.0, 1.0)
        }
        PipShape::RoundedSquare { radius_ratio } => {
            let rr = (w.min(h) * radius_ratio.clamp(0.0, 0.5)).max(1.0);
            let qx = (x - cx).abs() - (cx - rr);
            let qy = (y - cy).abs() - (cy - rr);
            let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rr;
            (-dist).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcam_pip_circle_and_fullscreen_toggle() {
        let mut base = vec![10u8; 200 * 200 * 4];
        for px in base.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let cam = vec![240u8; 64 * 64 * 4];

        // Circle PiP at bottom-right
        composite_webcam_pip(
            &mut base,
            200,
            200,
            &cam,
            64,
            64,
            WebcamOverlayMode::Pip {
                corner: PipCorner::BottomRight,
                shape: PipShape::Circle,
                scale: 0.25, // 50x50
                margin: 10,
            },
        );
        // Center of PiP (165, 165) should be webcam color 240
        let pip_center = (165 * 200 + 165) * 4;
        assert_eq!(base[pip_center], 240);
        // Corner of PiP bounding box (140, 140) is outside circle -> still base color 10
        let pip_corner = (140 * 200 + 140) * 4;
        assert_eq!(base[pip_corner], 10);

        // Toggle mid-recording to FullscreenWebcam
        composite_webcam_pip(
            &mut base,
            200,
            200,
            &cam,
            64,
            64,
            WebcamOverlayMode::FullscreenWebcam,
        );
        assert_eq!(base[0], 240);
        assert_eq!(base[(199 * 200 + 199) * 4], 240);
    }

    #[test]
    fn enumerate_webcams_succeeds_cleanly() {
        let cams = enumerate_webcams().expect("MFEnumDeviceSources should not error");
        for c in &cams {
            assert!(!c.name.is_empty());
        }
    }
}
