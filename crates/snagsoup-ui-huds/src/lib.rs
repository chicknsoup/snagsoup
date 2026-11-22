//! SnagSoup floating HUDs and Video-from-Images studio (`W3-04` + `W3-13`):
//! - `exclude_capture`: `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` so HUDs never appear in recordings
//! - `all_in_one_hud`: Post-selection All-in-One HUD (`W x H`, 8-handle fine-tune, Image/Video/Panoramic)
//! - `panoramic_hud`: Panoramic Live Preview HUD wired to `snagsoup_stitch::PanoramicSession`
//! - `recording_hud`: Pre-Recording, 3-2-1 Countdown, Mini Recording HUD & Webcam PiP controls
//! - `video_from_images_view`: Create Video from Images studio wired to `snagsoup_media::VideoFromImagesSession`

pub mod all_in_one_hud;
pub mod exclude_capture;
pub mod panoramic_hud;
pub mod recording_hud;
pub mod video_from_images_view;

pub use all_in_one_hud::{AllInOneAction, AllInOneHudView, ResizeHandle};
pub use exclude_capture::{
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, extract_hwnd, get_cursor_physical_pos,
    get_window_exclude_from_capture, hide_window_hwnd, is_window_visible_hwnd,
    position_window_physical, query_hwnd_physical_rect_and_dpi, restore_and_focus_hwnd,
    set_window_exclude_from_capture,
};
pub use panoramic_hud::{PanoramicHudView, ScrollSpeedStatus};
pub use recording_hud::{RecordingHudView, RecordingPhase, format_mm_ss};
pub use video_from_images_view::{PresenterTool, SlideItem, VideoFromImagesView};
