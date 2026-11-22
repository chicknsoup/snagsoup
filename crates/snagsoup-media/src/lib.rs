//! SnagSoup video recording, WASAPI lock-free audio mixing, webcam PiP,
//! crash-safe disaster recovery, MP4 decoding/trimming, and video-from-images crate.

pub mod audio_mixer;
pub mod live_recorder;
pub mod nv12;
pub mod player_decoder;
pub mod recorder;
pub mod recovery;
pub mod trimmer;
pub mod video_from_images;
pub mod webcam;

pub use audio_mixer::{
    AudioDeviceInfo, AudioLevelMeter, LinearResampler, LockFreeAudioMixer, WasapiCaptureStream,
    enumerate_audio_devices, soft_limit,
};
pub use live_recorder::{LiveRecordConfig, LiveScreenRecorder};
pub use nv12::{bgra_to_nv12, even_dimensions, rgba_to_nv12};
pub use player_decoder::{
    DecodedFrame, Mp4StreamDecoder, VideoMetadata, decode_video_frames_range,
    extract_video_frame_rgba, probe_video_metadata,
};
pub use recorder::{HNS_PER_SECOND, Mp4RecorderConfig, Mp4SinkWriter, TimestampOffsetter};
pub use recovery::{CrashSafeRecorder, DisasterRecoveryManager, RecoveredRecording, RecoveryJournal};
pub use trimmer::{CutInterval, map_timestamp_with_cuts, merge_cut_intervals, trim_mp4_intervals};
pub use video_from_images::VideoFromImagesSession;
pub use webcam::{
    PipCorner, PipShape, WebcamDeviceInfo, WebcamOverlayMode, composite_webcam_pip,
    enumerate_webcams,
};
use std::fmt;
use std::sync::OnceLock;
use windows::Win32::Media::MediaFoundation::{MF_VERSION, MFStartup, MFSTARTUP_NOSOCKET};

static MF_INIT: OnceLock<Result<(), i32>> = OnceLock::new();

/// Initialize Windows Media Foundation once per process (thread-safe; never calls `MFShutdown`
/// while concurrent threads may still hold `IMFSinkWriter`/`IMFSourceReader` instances).
pub(crate) fn ensure_mf_started() -> Result<(), MediaError> {
    match *MF_INIT.get_or_init(|| {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.map_err(|e| e.code().0)
    }) {
        Ok(()) => Ok(()),
        Err(hresult) => Err(MediaError::MediaFoundation {
            api: "MFStartup",
            hresult,
        }),
    }
}

/// Errors returned by media recording, encoding, decoding, and audio mixing pipelines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    MediaFoundation { api: &'static str, hresult: i32 },
    Wasapi { api: &'static str, hresult: i32 },
    InvalidConfig(String),
    Io(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MediaFoundation { api, hresult } => {
                write!(
                    f,
                    "Media Foundation `{api}` failed: 0x{:08X}",
                    *hresult as u32
                )
            }
            Self::Wasapi { api, hresult } => {
                write!(f, "WASAPI `{api}` failed: 0x{:08X}", *hresult as u32)
            }
            Self::InvalidConfig(msg) => write!(f, "invalid media config: {msg}"),
            Self::Io(msg) => write!(f, "media I/O error: {msg}"),
        }
    }
}

impl std::error::Error for MediaError {}
