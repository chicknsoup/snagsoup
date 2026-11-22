use crate::error::{CanvasError, Result};
use snagsoup_model::{ColorRgba, RectF};
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;

/// Fixed tile width and height in pixels (256x256).
pub const TILE_SIZE: u32 = 256;
/// Fixed byte size of one RGBA8 tile: 256 * 256 * 4 = 262,144 bytes (256 KiB).
pub const TILE_BYTES: usize = (TILE_SIZE as usize) * (TILE_SIZE as usize) * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileCoord {
    pub tx: u32,
    pub ty: u32,
}

impl TileCoord {
    pub const fn new(tx: u32, ty: u32) -> Self {
        Self { tx, ty }
    }
}

/// Axis-aligned integer pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub const fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub const fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    pub fn intersect(&self, other: PixelRect) -> Option<PixelRect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 > x0 && y1 > y0 {
            Some(PixelRect::new(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
    }

    pub fn union(&self, other: PixelRect) -> PixelRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        PixelRect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn to_rect_f(&self) -> RectF {
        RectF::new(
            self.x as f64,
            self.y as f64,
            self.width as f64,
            self.height as f64,
        )
    }

    pub fn from_rect_f(r: RectF, max_w: u32, max_h: u32) -> Option<PixelRect> {
        let x0 = r.x.floor().max(0.0) as u32;
        let y0 = r.y.floor().max(0.0) as u32;
        let x1 = r.right().ceil().clamp(0.0, max_w as f64) as u32;
        let y1 = r.bottom().ceil().clamp(0.0, max_h as f64) as u32;
        if x1 > x0 && y1 > y0 {
            Some(PixelRect::new(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
    }
}

/// Allocates a zeroed tile directly on the heap (avoiding 256 KiB stack frames).
fn alloc_zero_tile() -> Arc<[u8; TILE_BYTES]> {
    let boxed_slice = vec![0u8; TILE_BYTES].into_boxed_slice();
    let boxed_arr: Box<[u8; TILE_BYTES]> = boxed_slice
        .try_into()
        .expect("TILE_BYTES length matches");
    Arc::from(boxed_arr)
}

/// Allocates a solid-color tile directly on the heap.
fn alloc_filled_tile(color: ColorRgba) -> Arc<[u8; TILE_BYTES]> {
    let mut vec = vec![0u8; TILE_BYTES];
    let px = [color.r, color.g, color.b, color.a];
    for chunk in vec.chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
    let boxed_arr: Box<[u8; TILE_BYTES]> = vec
        .into_boxed_slice()
        .try_into()
        .expect("TILE_BYTES length matches");
    Arc::from(boxed_arr)
}

/// Clones a tile directly on the heap without placing 256 KiB on the stack.
fn clone_tile_heap(src: &[u8; TILE_BYTES]) -> Arc<[u8; TILE_BYTES]> {
    let mut vec = vec![0u8; TILE_BYTES];
    vec.copy_from_slice(src);
    let boxed_arr: Box<[u8; TILE_BYTES]> = vec
        .into_boxed_slice()
        .try_into()
        .expect("TILE_BYTES length matches");
    Arc::from(boxed_arr)
}

/// Copy-on-Write RGBA8 image backed by a grid of fixed 256x256 `Arc<[u8; 262_144]>` tiles.
#[derive(Debug, Clone)]
pub struct TiledBitmap {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Arc<[u8; TILE_BYTES]>>,
}

impl TiledBitmap {
    pub fn new_empty(width: u32, height: u32) -> Self {
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        let count = (tiles_x as usize) * (tiles_y as usize);
        let tiles = if count == 0 {
            Vec::new()
        } else {
            let shared = alloc_zero_tile();
            vec![shared; count]
        };
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            tiles,
        }
    }

    pub fn new_filled(width: u32, height: u32, color: ColorRgba) -> Self {
        if color == ColorRgba::TRANSPARENT {
            return Self::new_empty(width, height);
        }
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        let count = (tiles_x as usize) * (tiles_y as usize);
        let tiles = if count == 0 {
            Vec::new()
        } else {
            let shared = alloc_filled_tile(color);
            vec![shared; count]
        };
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            tiles,
        }
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn tiles_x(&self) -> u32 {
        self.tiles_x
    }

    pub const fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    pub const fn bounds(&self) -> PixelRect {
        PixelRect::new(0, 0, self.width, self.height)
    }

    pub fn tile_index(&self, tx: u32, ty: u32) -> Option<usize> {
        if tx < self.tiles_x && ty < self.tiles_y {
            Some((ty as usize) * (self.tiles_x as usize) + (tx as usize))
        } else {
            None
        }
    }

    pub fn tile_arc(&self, tx: u32, ty: u32) -> Option<&Arc<[u8; TILE_BYTES]>> {
        let idx = self.tile_index(tx, ty)?;
        self.tiles.get(idx)
    }

    /// Returns unique heap pointers for all backing tiles (useful for CoW memory accounting).
    pub fn collect_unique_tile_ptrs(&self, out: &mut HashSet<usize>) {
        for tile in &self.tiles {
            out.insert(Arc::as_ptr(tile) as usize);
        }
    }

    /// Ensures the tile at `(tx, ty)` is uniquely owned and returns a mutable slice.
    fn make_tile_mut(&mut self, tx: u32, ty: u32) -> &mut [u8; TILE_BYTES] {
        let idx = (ty as usize) * (self.tiles_x as usize) + (tx as usize);
        if Arc::strong_count(&self.tiles[idx]) > 1 {
            self.tiles[idx] = clone_tile_heap(&self.tiles[idx]);
        }
        Arc::get_mut(&mut self.tiles[idx]).expect("tile uniquely owned after CoW clone")
    }

    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Self> {
        let expected = (width as usize) * (height as usize) * 4;
        if rgba.len() != expected {
            return Err(CanvasError::BufferLengthMismatch {
                expected,
                actual: rgba.len(),
            });
        }
        let mut bmp = Self::new_empty(width, height);
        if width == 0 || height == 0 {
            return Ok(bmp);
        }

        let src_stride = (width as usize) * 4;
        let tile_stride = (TILE_SIZE as usize) * 4;

        for ty in 0..bmp.tiles_y {
            let y0 = ty * TILE_SIZE;
            let copy_h = (height - y0).min(TILE_SIZE);
            for tx in 0..bmp.tiles_x {
                let x0 = tx * TILE_SIZE;
                let copy_w = (width - x0).min(TILE_SIZE);
                let row_bytes = (copy_w as usize) * 4;
                let tile = bmp.make_tile_mut(tx, ty);
                for ly in 0..copy_h {
                    let sy = (y0 + ly) as usize;
                    let src_off = sy * src_stride + (x0 as usize) * 4;
                    let dst_off = (ly as usize) * tile_stride;
                    tile[dst_off..dst_off + row_bytes]
                        .copy_from_slice(&rgba[src_off..src_off + row_bytes]);
                }
            }
        }
        Ok(bmp)
    }

    /// Decodes a PNG byte stream (RGBA, RGB, Grayscale, Indexed) into a `TiledBitmap`.
    pub fn from_png_bytes(bytes: &[u8]) -> Result<Self> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(
            png::Transformations::EXPAND | png::Transformations::STRIP_16,
        );
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf)?;
        let raw = &buf[..info.buffer_size()];
        let w = info.width;
        let h = info.height;

        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => raw.to_vec(),
            png::ColorType::Rgb => {
                let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
                for px in raw.chunks_exact(3) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                out
            }
            png::ColorType::GrayscaleAlpha => {
                let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
                for px in raw.chunks_exact(2) {
                    out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
                out
            }
            png::ColorType::Grayscale => {
                let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
                for &g in raw {
                    out.extend_from_slice(&[g, g, g, 255]);
                }
                out
            }
            png::ColorType::Indexed => {
                // EXPAND already converts Indexed to RGB/RGBA
                raw.to_vec()
            }
        };

        Self::from_rgba(w, h, &rgba)
    }

    pub fn to_rgba(&self) -> Vec<u8> {
        self.read_region(self.bounds())
    }

    /// Encodes the bitmap to PNG bytes using fast compression (`fdeflate` via `png` crate).
    pub fn to_png_bytes(&self) -> Result<Vec<u8>> {
        encode_rgba_to_png(self.width, self.height, &self.to_rgba())
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let tx = x / TILE_SIZE;
        let ty = y / TILE_SIZE;
        let lx = (x % TILE_SIZE) as usize;
        let ly = (y % TILE_SIZE) as usize;
        let tile = self.tile_arc(tx, ty)?;
        let off = (ly * (TILE_SIZE as usize) + lx) * 4;
        Some([tile[off], tile[off + 1], tile[off + 2], tile[off + 3]])
    }

    /// Reads an arbitrary pixel rectangle into a contiguous RGBA8 buffer.
    pub fn read_region(&self, rect: PixelRect) -> Vec<u8> {
        let Some(clipped) = rect.intersect(self.bounds()) else {
            return Vec::new();
        };
        let mut out = vec![0u8; (clipped.width as usize) * (clipped.height as usize) * 4];
        let dst_stride = (clipped.width as usize) * 4;
        let tile_stride = (TILE_SIZE as usize) * 4;

        let tx_start = clipped.x / TILE_SIZE;
        let ty_start = clipped.y / TILE_SIZE;
        let tx_end = (clipped.right() - 1) / TILE_SIZE;
        let ty_end = (clipped.bottom() - 1) / TILE_SIZE;

        for ty in ty_start..=ty_end {
            let tile_y0 = ty * TILE_SIZE;
            let y0 = clipped.y.max(tile_y0);
            let y1 = clipped.bottom().min(tile_y0 + TILE_SIZE);
            for tx in tx_start..=tx_end {
                let tile_x0 = tx * TILE_SIZE;
                let x0 = clipped.x.max(tile_x0);
                let x1 = clipped.right().min(tile_x0 + TILE_SIZE);
                let row_bytes = ((x1 - x0) as usize) * 4;
                let tile = self.tile_arc(tx, ty).expect("clipped tile in bounds");

                for y in y0..y1 {
                    let ly = (y - tile_y0) as usize;
                    let lx = (x0 - tile_x0) as usize;
                    let src_off = ly * tile_stride + lx * 4;
                    let dst_off =
                        ((y - clipped.y) as usize) * dst_stride + ((x0 - clipped.x) as usize) * 4;
                    out[dst_off..dst_off + row_bytes]
                        .copy_from_slice(&tile[src_off..src_off + row_bytes]);
                }
            }
        }
        out
    }

    /// Mutates pixels inside `rect` using Copy-on-Write: ONLY tiles intersecting `rect` are cloned.
    /// Returns the list of modified `TileCoord`s.
    pub fn mutate_region<F>(&mut self, rect: PixelRect, mut f: F) -> Vec<TileCoord>
    where
        F: FnMut(u32, u32, &mut [u8; 4]),
    {
        let Some(clipped) = rect.intersect(self.bounds()) else {
            return Vec::new();
        };
        let tx_start = clipped.x / TILE_SIZE;
        let ty_start = clipped.y / TILE_SIZE;
        let tx_end = (clipped.right() - 1) / TILE_SIZE;
        let ty_end = (clipped.bottom() - 1) / TILE_SIZE;

        let mut modified = Vec::with_capacity(
            ((tx_end - tx_start + 1) * (ty_end - ty_start + 1)) as usize,
        );
        let tile_stride = (TILE_SIZE as usize) * 4;

        for ty in ty_start..=ty_end {
            let tile_y0 = ty * TILE_SIZE;
            let y0 = clipped.y.max(tile_y0);
            let y1 = clipped.bottom().min(tile_y0 + TILE_SIZE);
            for tx in tx_start..=tx_end {
                let tile_x0 = tx * TILE_SIZE;
                let x0 = clipped.x.max(tile_x0);
                let x1 = clipped.right().min(tile_x0 + TILE_SIZE);

                let tile = self.make_tile_mut(tx, ty);
                for y in y0..y1 {
                    let ly = (y - tile_y0) as usize;
                    for x in x0..x1 {
                        let lx = (x - tile_x0) as usize;
                        let off = ly * tile_stride + lx * 4;
                        let mut px = [tile[off], tile[off + 1], tile[off + 2], tile[off + 3]];
                        f(x, y, &mut px);
                        tile[off..off + 4].copy_from_slice(&px);
                    }
                }
                modified.push(TileCoord::new(tx, ty));
            }
        }

        modified
    }

    /// Crops the bitmap to `rect`, returning a new `TiledBitmap`.
    pub fn crop(&self, rect: PixelRect) -> Self {
        let Some(clipped) = rect.intersect(self.bounds()) else {
            return Self::new_empty(0, 0);
        };
        let rgba = self.read_region(clipped);
        Self::from_rgba(clipped.width, clipped.height, &rgba)
            .expect("clipped region matches dimensions")
    }

    /// Resizes canvas to `(new_width, new_height)` placing existing content at `(offset_x, offset_y)`.
    pub fn resize(
        &self,
        new_width: u32,
        new_height: u32,
        offset_x: i32,
        offset_y: i32,
        fill: ColorRgba,
    ) -> Self {
        let mut out = Self::new_filled(new_width, new_height, fill);
        if self.width == 0 || self.height == 0 || new_width == 0 || new_height == 0 {
            return out;
        }

        let dst_x0 = offset_x.max(0) as u32;
        let dst_y0 = offset_y.max(0) as u32;
        let src_x0 = (-offset_x).max(0) as u32;
        let src_y0 = (-offset_y).max(0) as u32;

        if src_x0 >= self.width || src_y0 >= self.height || dst_x0 >= new_width || dst_y0 >= new_height
        {
            return out;
        }

        let copy_w = (self.width - src_x0).min(new_width - dst_x0);
        let copy_h = (self.height - src_y0).min(new_height - dst_y0);

        let src_buf = self.read_region(PixelRect::new(src_x0, src_y0, copy_w, copy_h));
        let src_stride = (copy_w as usize) * 4;
        out.mutate_region(PixelRect::new(dst_x0, dst_y0, copy_w, copy_h), |x, y, px| {
            let sx = (x - dst_x0) as usize;
            let sy = (y - dst_y0) as usize;
            let off = sy * src_stride + sx * 4;
            px.copy_from_slice(&src_buf[off..off + 4]);
        });
        out
    }
}

impl PartialEq for TiledBitmap {
    fn eq(&self, other: &Self) -> bool {
        if self.width != other.width || self.height != other.height {
            return false;
        }
        // Fast path: identical Arc pointers compare in O(1), otherwise compare visible pixels.
        if self.tiles.len() == other.tiles.len()
            && self
                .tiles
                .iter()
                .zip(other.tiles.iter())
                .all(|(a, b)| Arc::ptr_eq(a, b) || a == b)
        {
            return true;
        }
        self.to_rgba() == other.to_rgba()
    }
}

/// Encodes a contiguous RGBA8 buffer to PNG bytes using fast compression.
pub fn encode_rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(CanvasError::InvalidDimensions { width, height });
    }
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(CanvasError::BufferLengthMismatch {
            expected,
            actual: rgba.len(),
        });
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(out)
}
