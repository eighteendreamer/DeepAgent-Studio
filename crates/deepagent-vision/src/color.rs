//! Pixel-level colour analysis: dominant colours, brightness histogram,
//! saturated-region detection.

use image::{DynamicImage, Rgba};
use serde::{Deserialize, Serialize};

/// Result of pixel-level colour analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorAnalysis {
    /// Top dominant colours (hex like "#FF0000") with approximate pixel counts.
    pub dominant_colors: Vec<DominantColor>,
    /// Average brightness (0-255).
    pub avg_brightness: f64,
    /// Brightness histogram: 8 buckets from dark to bright, each with pixel count.
    pub brightness_histogram: Vec<u64>,
    /// Saturated (non-grey) pixel count — pixels where max-min channel > threshold.
    pub saturated_pixel_count: u64,
    /// Percentage of saturated pixels (0-100).
    pub saturated_pixel_ratio: f64,
    /// Detected saturated colour regions (e.g. "red", "blue", "green").
    pub color_regions: Vec<ColorRegion>,
    /// Image area classification: "dark", "dim", "normal", "bright".
    pub brightness_level: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DominantColor {
    pub hex: String,
    pub count: u64,
    pub ratio: f64, // 0-1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorRegion {
    /// Colour name: "red", "blue", "green", "yellow", "orange", "purple", "cyan".
    pub name: String,
    /// Approximate pixel count.
    pub count: u64,
    /// Percentage of image (0-100).
    pub ratio: f64,
}

/// Saturation threshold for non-grey detection.
const SATURATION_THRESHOLD: u8 = 30;
/// Number of bins for colour quantisation (each channel divided into this many levels).
const QUANT_LEVELS: u8 = 4; // 4^3 = 64 colour buckets

/// Analyse the colour distribution of an image.
pub fn analyze_colors(img: &DynamicImage) -> ColorAnalysis {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let total_pixels = (width as u64) * (height as u64);

    if total_pixels == 0 {
        return ColorAnalysis {
            dominant_colors: Vec::new(),
            avg_brightness: 0.0,
            brightness_histogram: vec![0; 8],
            saturated_pixel_count: 0,
            saturated_pixel_ratio: 0.0,
            color_regions: Vec::new(),
            brightness_level: "unknown".to_string(),
        };
    }

    let mut color_buckets: std::collections::HashMap<(u8, u8, u8), u64> =
        std::collections::HashMap::with_capacity(64);
    let mut brightness_sum: u64 = 0;
    let mut brightness_hist = [0u64; 8];
    let mut saturated_count: u64 = 0;
    let mut region_counts: std::collections::HashMap<&'static str, u64> =
        std::collections::HashMap::new();

    for (_, pixel) in rgba.pixels().enumerate() {
        let Rgba([r, g, b, _a]) = *pixel;
        // Quantise for dominant-colour bucketing.
        let step = 256u32 / QUANT_LEVELS as u32;
        let qr = (r as u32 / step) as u8;
        let qg = (g as u32 / step) as u8;
        let qb = (b as u32 / step) as u8;
        *color_buckets.entry((qr, qg, qb)).or_insert(0) += 1;

        // Brightness (luminance approximation).
        let brightness = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
        brightness_sum += brightness as u64;
        let bucket = (brightness / 32).min(7) as usize;
        brightness_hist[bucket] += 1;

        // Saturated pixel detection.
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let saturation = max_c.saturating_sub(min_c);
        if saturation > SATURATION_THRESHOLD {
            saturated_count += 1;
            if let Some(name) = classify_color(r, g, b) {
                *region_counts.entry(name).or_insert(0) += 1;
            }
        }
    }

    // Dominant colours: sort buckets by count, take top 8.
    let mut sorted_buckets: Vec<_> = color_buckets.into_iter().collect();
    sorted_buckets.sort_by(|a, b| b.1.cmp(&a.1));
    let dominant_colors: Vec<DominantColor> = sorted_buckets
        .iter()
        .take(8)
        .map(|((qr, qg, qb), count)| {
            // De-quantise to representative colour (centre of bucket).
            let step = 256u32 / QUANT_LEVELS as u32;
            let half = (step / 2) as u8;
            let r = (*qr as u32 * step) as u8 + half;
            let g = (*qg as u32 * step) as u8 + half;
            let b = (*qb as u32 * step) as u8 + half;
            DominantColor {
                hex: format!("#{:02X}{:02X}{:02X}", r, g, b),
                count: *count,
                ratio: *count as f64 / total_pixels as f64,
            }
        })
        .collect();

    let avg_brightness = brightness_sum as f64 / total_pixels as f64;
    let saturated_ratio = saturated_count as f64 / total_pixels as f64;

    let color_regions: Vec<ColorRegion> = {
        let mut regions: Vec<_> = region_counts
            .iter()
            .map(|(name, count)| ColorRegion {
                name: name.to_string(),
                count: *count,
                ratio: *count as f64 * 100.0 / total_pixels as f64,
            })
            .collect();
        regions.sort_by(|a, b| b.count.cmp(&a.count));
        regions
    };

    let brightness_level = if avg_brightness < 50.0 {
        "dark"
    } else if avg_brightness < 100.0 {
        "dim"
    } else if avg_brightness < 180.0 {
        "normal"
    } else {
        "bright"
    };

    ColorAnalysis {
        dominant_colors,
        avg_brightness,
        brightness_histogram: brightness_hist.to_vec(),
        saturated_pixel_count: saturated_count,
        saturated_pixel_ratio: saturated_ratio * 100.0,
        color_regions,
        brightness_level: brightness_level.to_string(),
    }
}

/// Classify a saturated pixel into a coarse colour name.
fn classify_color(r: u8, g: u8, b: u8) -> Option<&'static str> {
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    if max_c == 0 || max_c.saturating_sub(min_c) <= SATURATION_THRESHOLD {
        return None;
    }

    // Convert to HSV for classification.
    let r_f = r as f32 / 255.0;
    let g_f = g as f32 / 255.0;
    let b_f = b as f32 / 255.0;
    let max_f = r_f.max(g_f).max(b_f);
    let min_f = r_f.min(g_f).min(b_f);
    let delta = max_f - min_f;

    let hue = if delta == 0.0 {
        0.0
    } else if max_f == r_f {
        60.0 * (((g_f - b_f) / delta) % 6.0)
    } else if max_f == g_f {
        60.0 * (((b_f - r_f) / delta) + 2.0)
    } else {
        60.0 * (((r_f - g_f) / delta) + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };

    Some(match hue as u32 {
        0..=15 | 346..=360 => "red",
        16..=45 => "orange",
        46..=70 => "yellow",
        71..=160 => "green",
        161..=200 => "cyan",
        201..=250 => "blue",
        251..=290 => "purple",
        291..=345 => "magenta",
        _ => "other",
    })
}
