# SnagSoup v1.4.1.3

High-performance, 100% local-first multi-monitor screen capture, video recording, OCR, and annotation studio for Windows 10/11 — written in pure **Rust** on top of **GPUI DirectX 11**.

---

## Highlights

- **1:1 Physical-Pixel Multi-Monitor Screen Freeze:** Freezes virtual desktops spanning mixed-DPI monitors (`100%`, `125%`, `150%`, `200%`, negative/offset coordinates) in a single atomic pass while preserving exact Z-order window & child-pane hierarchy (titlebar, client area, sub-panes).
- **Compact Capture App & System Tray Daemon:**
  - Snagit-style compact Capture Window (`All-in-One`, `Image`, `Video`) with Presets drawer, solid blue menu bar (`File`, `Edit`, `View`, `Help > About`), and standalone 3-tab `SnagSoup Preferences` dialog (`General`, `Capture`, `Hotkeys`).
  - Hardware-polled global shortcut recorder + `WH_KEYBOARD_LL` fallback so `Print Screen` works reliably on Windows 11 even when Snipping Tool claims `VK_SNAPSHOT`.
  - True `SW_HIDE` close-to-tray behavior (`< 15 MB` idle working-set RAM in `--tray` Tier-0 mode).
- **Crisp HiDPI Annotation Editor & Recent Captures Tray:**
  - Copy-on-Write `256×256` tiled bitmap engine (`snagsoup-canvas`) with full Undo/Redo, 21 vector & raster tools (`Arrow`, `Text`, `Callout`, `Shape`, `Stamp`, `Step`, `Blur`, `Magnify`, `Cut Out`, `Simplify`, `Smart Move`, `Magic Wand`, `Crop`, etc.), and 15 live image effects.
  - `.snagx` ZIP/JSON archive compatibility + SQLite WAL + FTS5 local Library (`library.db`) + bottom Recent Captures thumbnail strip.
- **Offline Multilingual Neural OCR (`Grab Text` & `Edit Text`):**
  - Dual-stage neural pipeline (**PP-OCRv4 DBNet** detector + **EasyOCR `latin_g2`** 64px BiLSTM-CTC recognizer) executed via Windows built-in `Windows.AI.MachineLearning` (`System32\onnxruntime.dll`) with zero external DLL dependencies.
  - Supports **Vietnamese** (full stacked diacritics & tone marks), **English**, source code/URLs, small `10–14px` ClearType UI fonts, dark-mode/colored badges, region-scoped `Grab Text`, and in-place `Edit Text` with background inpainting.
- **Live MP4 Video Recording & Scrolling/Panoramic Stitching:**
  - `Windows.Graphics.Capture` + Media Foundation H.264/AAC hardware encoder + lock-free WASAPI loopback/microphone mixer + `WDA_EXCLUDEFROMCAPTURE` floating HUD.

---

## Workspace Architecture

| Crate | Responsibility |
| :--- | :--- |
| `crates/snagsoup-app` | Single-binary entry point (`snagsoup.exe`), CLI dispatcher, Tier-0 tray/hotkey daemon, Win32 PE icon/version resources |
| `crates/snagsoup-ui` | Main GPUI Capture Window, Selection Overlay, Preferences/About dialogs, Editor Shell, Video Player, Grab Text modal |
| `crates/snagsoup-ui-huds` | Floating All-in-One, Recording, Panoramic HUDs & `WDA_EXCLUDEFROMCAPTURE` Win32 window helpers |
| `crates/snagsoup-ui-library` | SQLite FTS5 Library Organizer, Template Builder, Combine Images, Share targets, XLIFF translation view |
| `crates/snagsoup-canvas` | CoW `256×256` `TiledBitmap`, Undo/Redo `DocumentSession`, `SceneCompositor`, vector/raster tools & 15 effects |
| `crates/snagsoup-capture` | Multi-monitor `Windows.Graphics.Capture` / DXGI / GDI screen freezer & cursor capture |
| `crates/snagsoup-cv-ocr` | Offline Neural OCR (`PP-OCRv4` + `EasyOCR` Vietnamese/Latin via WinML), `Edit Text`, Magic Wand, Smart Move, Auto-Simplify |
| `crates/snagsoup-media` | Live MP4 screen recorder, WASAPI audio mixer, webcam PiP, crash-safe recovery journal, MP4 trimmer & frame decoder |
| `crates/snagsoup-stitch` | Automatic vertical/horizontal Scrolling Window & live Panoramic phase-correlation stitcher |
| `crates/snagsoup-library-export` | SQLite WAL + FTS5 indexer & PNG / JPEG / PDF / SVG / Animated GIF exporters |
| `crates/snagsoup-win-platform` | Per-Monitor DPI v2, Z-order window/UIA hierarchy finder, Win32 Clipboard (`PNG` + `CF_DIBV5`), Hotkeys, Tray, Startup Registry |
| `crates/snagsoup-model` | `.snagx`, `.snagtheme`, `.snagpalette` data models, RTF codec, presets & geometry primitives |
| `crates/snagsoup-shellext` | Windows Explorer `.snagx` thumbnail & preview shell extension helpers |

---

## Vendor Components & Integration Guide

Both `/vendor/` and `/dist/` are excluded from Git via [`.gitignore`](.gitignore) to keep the repository lightweight. Below is the complete specification of every vendored component and how to set them up on a fresh clone.

### 1. Vendored GPUI Engine (`vendor/zed`)

SnagSoup renders all windows via **GPUI** (`gpui` + `gpui_windows` DirectX 11 backend) vendored from [`zed-industries/zed`](https://github.com/zed-industries/zed) at `vendor/zed`.

- **Upstream Repository:** `https://github.com/zed-industries/zed.git`
- **Pinned Base Commit:** `2f808b3c2de049ecfdbad0fdb26763cebfcaa3b4`
- **Crates Consumed by SnagSoup:**
  - `vendor/zed/crates/gpui` (referenced as `gpui = { path = "../../vendor/zed/crates/gpui" }` in UI crates)
  - `vendor/zed/crates/gpui_windows` (DirectX 11 renderer & Win32 platform windowing)
  - Supporting internal crates under `vendor/zed/crates/`: `collections`, `gpui_macros`, `gpui_shared_string`, `gpui_util`, `http_client`, `refineable`, `scheduler`, `sum_tree`, `ztracing`, `ztracing_macro`
- **Custom SnagSoup Patches ([`patches/zed-gpui-standalone.patch`](patches/zed-gpui-standalone.patch)):**
  1. **Standalone Workspace Decoupling (`Cargo.toml` manifests):** Decouples `gpui` and its 11 helper crates from Zed's 200+ editor workspace members so `snagsoup` compiles standalone in seconds without fetching unrelated Zed crates.
  2. **1:1 Physical Multi-Monitor Overlay Positioning (`crates/gpui_windows/src/window.rs`):** Preserves exact virtual-desktop physical pixel bounds across mixed-DPI monitors (including negative monitor origins) without rescaling by the primary monitor's DPI.
  3. **Custom Crosshair & 4-Way Move Cursors (`crates/gpui_windows/src/events.rs`, `util.rs`):** Prevents `WM_SETCURSOR` default arrow overrides on frozen-screen selection overlays and maps `OpenHand`/`ClosedHand` to Win32 `IDC_SIZEALL` for `Move` and `Pan` tools.

#### Step-by-Step Setup for `vendor/zed`

Run from the repository root (`SnagSoup/`):

```powershell
# 1. Clone zed into vendor/zed (shallow fetch + pinned commit)
New-Item -ItemType Directory -Force -Path vendor | Out-Null
git clone --filter=blob:none --no-checkout https://github.com/zed-industries/zed.git vendor/zed
git -C vendor/zed sparse-checkout init --cone
git -C vendor/zed sparse-checkout set `
    crates/collections `
    crates/gpui `
    crates/gpui_macros `
    crates/gpui_shared_string `
    crates/gpui_util `
    crates/gpui_windows `
    crates/http_client `
    crates/refineable `
    crates/scheduler `
    crates/sum_tree `
    crates/ztracing `
    crates/ztracing_macro
git -C vendor/zed checkout 2f808b3c2de049ecfdbad0fdb26763cebfcaa3b4

# 2. Apply SnagSoup's standalone + multi-monitor HiDPI patch
git -C vendor/zed apply ../../patches/zed-gpui-standalone.patch
```

---

### 2. Offline Multilingual Neural OCR Models (`vendor/ocr-models` & `dist/models`)

`snagsoup-cv-ocr` runs a 2-stage ONNX pipeline directly through Windows 10/11's built-in `Windows.AI.MachineLearning` (`C:\Windows\System32\onnxruntime.dll`) — requiring **no external DLLs or Python runtime**:

| File | Size | Model & Upstream Source (Apache-2.0) | Tensor Specification |
| :--- | :--- | :--- | :--- |
| `det.onnx` | `4.7 MB` | **PP-OCRv4 DBNet** (`SWHL/RapidOCR`: `PP-OCRv4/ch_PP-OCRv4_det_infer.onnx`) | Input `x`: `[1, 3, H, W]` (`f32` BGR ImageNet norm) → Output `sigmoid_0.tmp_0`: `[1, 1, H, W]` |
| `rec_latin.onnx` | `15.4 MB` | **EasyOCR `latin_g2`** (`runemic-ai/easyocr-onnx`: `rec_latin.onnx`) | Input `image`: `[1, 1, 64, W]` (`f32` grayscale `[-1, 1]`) → Output `logits`: `[1, T, 352]` (Vietnamese + English + Latin + ASCII symbols) |

> **Note:** The 352-class character vocabulary (`crates/snagsoup-cv-ocr/src/rec_latin_dict.json`) is compiled directly into `snagsoup-cv-ocr`. At runtime, `snagsoup.exe` automatically searches for `det.onnx` and `rec_latin.onnx` in:
> 1. `<exe_dir>\models\` (e.g. `dist\models\`)
> 2. `%LOCALAPPDATA%\SnagSoup\models\`
> 3. `vendor\ocr-models\` (used automatically during `cargo test` / development)
> 4. If absent, SnagSoup gracefully falls back to a 3× Catmull-Rom super-resolved `Windows.Media.Ocr` pipeline.

#### Step-by-Step Setup for OCR Models

```powershell
New-Item -ItemType Directory -Force -Path vendor\ocr-models | Out-Null

# Download PP-OCRv4 DBNet detector (Apache-2.0)
curl.exe -sSL "https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx" `
    -o "vendor\ocr-models\det.onnx"

# Download EasyOCR latin_g2 Vietnamese + English recognizer (Apache-2.0)
curl.exe -sSL "https://huggingface.co/runemic-ai/easyocr-onnx/resolve/main/rec_latin.onnx" `
    -o "vendor\ocr-models\rec_latin.onnx"
```

---

## Building, Testing & Packaging

### Prerequisites
- **OS:** Windows 10 (2004+) or Windows 11 (`x86_64`)
- **Rust Toolchain:** Stable `x86_64-pc-windows-msvc` + `rust-lld.exe` (configured automatically via [`.cargo/config.toml`](.cargo/config.toml))
- **Windows 10/11 SDK:** Provides `rc.exe` (auto-discovered by [`crates/snagsoup-app/build.rs`](crates/snagsoup-app/build.rs) to embed the multi-layer Emerald S `.ico` and Win32 `VERSIONINFO`)

### Run Unit & Integration Tests
```powershell
cargo test --workspace
```

### Build & Package Release Distribution (`dist/`)
Use the `release-fast` profile for rapid optimized builds, then copy the binary and OCR models into `dist\`:

```powershell
cargo build -p snagsoup-app --profile release-fast

New-Item -ItemType Directory -Force -Path dist\models | Out-Null
Copy-Item -Force target\release-fast\snagsoup.exe dist\snagsoup.exe
Copy-Item -Force vendor\ocr-models\det.onnx dist\models\det.onnx
Copy-Item -Force vendor\ocr-models\rec_latin.onnx dist\models\rec_latin.onnx
```

---

## CLI Usage & Default Shortcuts

```text
snagsoup [OPTIONS] [FILE.snagx]

  (default)                      Launch SnagSoup Capture App window
  --tray                         Run Tier-0 lightweight Win32 tray + hotkey daemon (< 14 MB RAM)
  --editor [FILE.snagx]          Launch GPUI Editor window directly
  --capture                      Launch 3-tab GPUI Capture window
  --capture-now                  Immediately freeze desktop & open Selection Overlay
  --oneclick                     Launch OneClick top-edge floating capture bar
  --check-mem                    Report Tier-0 Working Set / Private Usage and exit
  --light / --dark               Start in Light (default) or Dark Studio theme
  --version / --help             Print version or usage
```

### Default Global Hotkeys (Configurable in `File > Capture Preferences... > Hotkeys`)
- **Global Capture:** `Print Screen`
- **Repeat Last Capture:** `Ctrl+Shift+R`
- **Show / Hide SnagSoup:** `Ctrl+Shift+X`
- **Video Record / Pause:** `Shift+F9`
- **Video Stop:** `Shift+F10`

### Local Data Directory
All captures, preferences, and SQLite FTS5 indexes stay 100% local under `%LOCALAPPDATA%\SnagSoup\`:
- `Captures\` — Auto-saved `.snagx` captures and `.mp4` recordings
- `Exports\` — Exported `.png`, `.jpg`, `.pdf`, `.svg`, and `.gif` files
- `library.db` — SQLite WAL + FTS5 searchable capture database
- `preferences.json` — Persisted Capture, General, Startup, and Hotkey preferences

---

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
