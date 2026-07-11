//! Lightweight screenshot/layout analysis.
//!
//! This mirrors the old Reasonix Python helpers: scan pixels, find
//! non-background bounds, row/column activity bands, and coloured annotations.
//! It is factual image geometry, not semantic object recognition.

use image::{DynamicImage, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityBand {
    pub start: u32,
    pub end: u32,
    pub non_background_pixels: u64,
    pub dark_pixels: u64,
    pub red_pixels: u64,
    pub blue_pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutAnalysis {
    pub width: u32,
    pub height: u32,
    pub non_background_pixels: u64,
    pub dark_pixels: u64,
    pub red_pixels: u64,
    pub blue_pixels: u64,
    pub non_background_ratio: f64,
    pub dark_ratio: f64,
    pub content_bbox: Option<Rect>,
    pub dark_bbox: Option<Rect>,
    pub row_bands: Vec<ActivityBand>,
    pub column_bands: Vec<ActivityBand>,
    pub likely_sparse_screenshot: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct PixelClass {
    non_background: bool,
    dark: bool,
    red: bool,
    blue: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct LineCounts {
    non_background: u64,
    dark: u64,
    red: u64,
    blue: u64,
}

pub fn analyze_layout(img: &DynamicImage) -> LayoutAnalysis {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rows = vec![LineCounts::default(); height as usize];
    let mut cols = vec![LineCounts::default(); width as usize];
    let mut content_bounds = Bounds::new();
    let mut dark_bounds = Bounds::new();

    for y in 0..height {
        for x in 0..width {
            let class = classify_pixel(*rgba.get_pixel(x, y));
            if class.non_background {
                rows[y as usize].non_background += 1;
                cols[x as usize].non_background += 1;
                content_bounds.include(x, y);
            }
            if class.dark {
                rows[y as usize].dark += 1;
                cols[x as usize].dark += 1;
                dark_bounds.include(x, y);
            }
            if class.red {
                rows[y as usize].red += 1;
                cols[x as usize].red += 1;
            }
            if class.blue {
                rows[y as usize].blue += 1;
                cols[x as usize].blue += 1;
            }
        }
    }

    let non_background_pixels = rows.iter().map(|row| row.non_background).sum::<u64>();
    let dark_pixels = rows.iter().map(|row| row.dark).sum::<u64>();
    let red_pixels = rows.iter().map(|row| row.red).sum::<u64>();
    let blue_pixels = rows.iter().map(|row| row.blue).sum::<u64>();
    let total_pixels = (width as u64).saturating_mul(height as u64).max(1);
    let non_background_ratio = non_background_pixels as f64 * 100.0 / total_pixels as f64;
    let dark_ratio = dark_pixels as f64 * 100.0 / total_pixels as f64;

    let row_threshold = ((width as u64) / 250).max(3);
    let col_threshold = ((height as u64) / 250).max(3);
    let row_bands = active_bands(&rows, row_threshold, 8, 12);
    let column_bands = active_bands(&cols, col_threshold, 8, 12);

    let likely_sparse_screenshot = width >= 800
        && height >= 500
        && non_background_ratio > 0.05
        && non_background_ratio < 18.0
        && row_bands.len() >= 2
        && column_bands.len() >= 2;

    LayoutAnalysis {
        width,
        height,
        non_background_pixels,
        dark_pixels,
        red_pixels,
        blue_pixels,
        non_background_ratio,
        dark_ratio,
        content_bbox: content_bounds.rect(),
        dark_bbox: dark_bounds.rect(),
        row_bands,
        column_bands,
        likely_sparse_screenshot,
    }
}

fn classify_pixel(pixel: Rgba<u8>) -> PixelClass {
    let Rgba([r, g, b, a]) = pixel;
    if a < 16 {
        return PixelClass::default();
    }

    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    let saturation = max_c.saturating_sub(min_c);
    let non_background = brightness < 245 || saturation > 18;
    let dark = brightness < 190;
    let red = r > 180 && r.saturating_sub(g) > 70 && r.saturating_sub(b) > 70;
    let blue = b > 120 && b.saturating_sub(r) > 45 && b.saturating_sub(g) > 20;

    PixelClass {
        non_background,
        dark,
        red,
        blue,
    }
}

fn active_bands(
    counts: &[LineCounts],
    threshold: u64,
    merge_gap: u32,
    max_bands: usize,
) -> Vec<ActivityBand> {
    let mut bands = Vec::new();
    let mut current: Option<(u32, u32, LineCounts)> = None;

    for (idx, count) in counts.iter().enumerate() {
        let active = count.non_background >= threshold
            || count.dark >= threshold
            || count.red > 0
            || count.blue > 0;
        let idx = idx as u32;

        match (&mut current, active) {
            (None, true) => current = Some((idx, idx, *count)),
            (Some((_, end, total)), true) => {
                *end = idx;
                add_counts(total, *count);
            }
            (Some((_, end, _)), false) if idx.saturating_sub(*end) <= merge_gap => {
                *end = idx;
            }
            (Some(_), false) => {
                push_band(&mut bands, current.take());
            }
            (None, false) => {}
        }
    }
    push_band(&mut bands, current);

    bands.sort_by_key(|band| std::cmp::Reverse(band.non_background_pixels + band.dark_pixels));
    bands.truncate(max_bands);
    bands.sort_by_key(|band| band.start);
    bands
}

fn add_counts(target: &mut LineCounts, source: LineCounts) {
    target.non_background += source.non_background;
    target.dark += source.dark;
    target.red += source.red;
    target.blue += source.blue;
}

fn push_band(bands: &mut Vec<ActivityBand>, current: Option<(u32, u32, LineCounts)>) {
    let Some((start, end, counts)) = current else {
        return;
    };
    bands.push(ActivityBand {
        start,
        end,
        non_background_pixels: counts.non_background,
        dark_pixels: counts.dark,
        red_pixels: counts.red,
        blue_pixels: counts.blue,
    });
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    seen: bool,
}

impl Bounds {
    fn new() -> Self {
        Self {
            min_x: u32::MAX,
            min_y: u32::MAX,
            max_x: 0,
            max_y: 0,
            seen: false,
        }
    }

    fn include(&mut self, x: u32, y: u32) {
        self.seen = true;
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn rect(self) -> Option<Rect> {
        if !self.seen {
            return None;
        }
        Some(Rect {
            x: self.min_x,
            y: self.min_y,
            width: self.max_x.saturating_sub(self.min_x) + 1,
            height: self.max_y.saturating_sub(self.min_y) + 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    #[test]
    fn detects_sparse_content_on_bright_background() {
        let mut img = RgbaImage::from_pixel(200, 120, Rgba([255, 255, 255, 255]));
        for y in 40..60 {
            for x in 30..150 {
                img.put_pixel(x, y, Rgba([40, 40, 40, 255]));
            }
        }
        for y in 80..90 {
            for x in 160..180 {
                img.put_pixel(x, y, Rgba([230, 20, 20, 255]));
            }
        }

        let analysis = analyze_layout(&DynamicImage::ImageRgba8(img));

        assert!(analysis.non_background_pixels > 0);
        assert!(analysis.dark_pixels > 0);
        assert!(analysis.red_pixels > 0);
        assert!(analysis.content_bbox.is_some());
        assert!(!analysis.row_bands.is_empty());
        assert!(!analysis.column_bands.is_empty());
    }
}
