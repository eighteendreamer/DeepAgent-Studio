//! ASCII art rendering — downsample an image to a character grid that
//! preserves coarse layout and text-region shapes.
//!
//! The charset goes from dark to light, so darker pixels become denser
//! characters. This gives the model a rough "visual" of the image layout
//! that is particularly effective for screenshots and diagrams.

use image::{DynamicImage, GenericImageView, Luma};
use serde::{Deserialize, Serialize};

/// ASCII art rendering result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsciiArt {
    /// The rendered character grid.
    pub text: String,
    /// Width of the grid in characters.
    pub width: usize,
    /// Height of the grid in characters.
    pub height: usize,
}

/// Characters from darkest (high density) to lightest (low density).
const CHARSET: &[char] = &[
    '@', '#', 'S', '%', '?', '*', '+', ':', ',', '.',
];

/// Maximum grid width in characters — keeps the output manageable for the model.
const MAX_WIDTH: usize = 80;
/// Maximum grid height in characters.
const MAX_HEIGHT: usize = 40;

/// Render an image as ASCII art.
///
/// The image is converted to greyscale, downsampled to a grid where each cell
/// is roughly 10×10 source pixels, and each cell is mapped to a character
/// based on its average brightness.
pub fn render_ascii(img: &DynamicImage) -> AsciiArt {
    render_ascii_with_bounds(img, MAX_WIDTH, MAX_HEIGHT)
}

/// Render ASCII art with custom maximum grid dimensions.
pub fn render_ascii_with_bounds(img: &DynamicImage, max_w: usize, max_h: usize) -> AsciiArt {
    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return AsciiArt {
            text: String::new(),
            width: 0,
            height: 0,
        };
    }

    // Calculate grid dimensions preserving aspect ratio.
    // Character cells are roughly 2:1 (tall), so we halve the height factor.
    let aspect = orig_w as f64 / orig_h as f64;
    let mut grid_w = max_w.min(orig_w as usize);
    let mut grid_h = (grid_w as f64 / aspect / 2.0).round() as usize;
    if grid_h > max_h {
        grid_h = max_h;
        grid_w = (grid_h as f64 * aspect * 2.0).round() as usize;
    }
    if grid_w == 0 {
        grid_w = 1;
    }
    if grid_h == 0 {
        grid_h = 1;
    }

    let grey = img.to_luma8();
    let cell_w = orig_w as f64 / grid_w as f64;
    let cell_h = orig_h as f64 / grid_h as f64;

    let mut lines = Vec::with_capacity(grid_h);
    for gy in 0..grid_h {
        let mut line = String::with_capacity(grid_w);
        for gx in 0..grid_w {
            let x0 = (gx as f64 * cell_w) as u32;
            let y0 = (gy as f64 * cell_h) as u32;
            let x1 = ((gx + 1) as f64 * cell_w).ceil() as u32;
            let y1 = ((gy + 1) as f64 * cell_h).ceil() as u32;
            let avg = average_brightness(&grey, x0, y0, x1, y1);
            let char_idx = map_brightness_to_char(avg);
            line.push(CHARSET[char_idx]);
        }
        lines.push(line);
    }

    AsciiArt {
        text: lines.join("\n"),
        width: grid_w,
        height: grid_h,
    }
}

/// Average brightness of a rectangular region.
fn average_brightness(
    grey: &image::ImageBuffer<Luma<u8>, Vec<u8>>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> u8 {
    let (w, h) = grey.dimensions();
    let x0 = x0.min(w);
    let x1 = x1.min(w).max(x0 + 1);
    let y0 = y0.min(h);
    let y1 = y1.min(h).max(y0 + 1);

    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let pixel = grey.get_pixel(x, y);
            sum += pixel[0] as u64;
            count += 1;
        }
    }
    if count == 0 {
        0
    } else {
        (sum / count) as u8
    }
}

/// Map a brightness value (0-255) to an index in CHARSET (0=darkest char).
fn map_brightness_to_char(brightness: u8) -> usize {
    // Invert: dark pixel → dense char (index 0).
    let normalised = 255 - brightness;
    let idx = (normalised as usize * CHARSET.len()) / 256;
    idx.min(CHARSET.len() - 1)
}
