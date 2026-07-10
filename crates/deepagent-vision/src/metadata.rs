//! Image metadata extraction — dimensions, format, colour mode, EXIF.

use image::ImageReader;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;

/// Basic metadata extracted from an image file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMetadata {
    /// Format string: "png", "jpeg", "gif", "webp", "bmp", etc.
    pub format: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour mode description: "RGB", "RGBA", "Luma", "LumaA".
    pub color_mode: String,
    /// Total pixel count.
    pub pixel_count: u64,
    /// File size in bytes.
    pub file_size: u64,
    /// EXIF fields if present (key → value strings).
    pub exif: Vec<(String, String)>,
}

/// Extract metadata from an image file on disk.
pub fn extract_metadata(path: &Path) -> Result<ImageMetadata, String> {
    let file_size = std::fs::metadata(path)
        .map_err(|e| format!("read file metadata: {e}"))?
        .len();

    let raw = std::fs::read(path).map_err(|e| format!("read image file: {e}"))?;

    extract_metadata_from_bytes(&raw, file_size)
}

/// Extract metadata from raw image bytes.
pub fn extract_metadata_from_bytes(raw: &[u8], file_size: u64) -> Result<ImageMetadata, String> {
    let reader = ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|e| format!("detect image format: {e}"))?;

    let format = reader
        .format()
        .map(|f| f.extensions_str().first().copied().unwrap_or("unknown"))
        .unwrap_or("unknown")
        .to_string();

    let (width, height, color_mode) = match reader.into_dimensions() {
        Ok((w, h)) => {
            // Re-open to get colour type.
            let img = image::load_from_memory(raw).map_err(|e| format!("decode image: {e}"))?;
            let ct = img.color();
            let mode = format!("{:?}", ct).to_lowercase();
            (w, h, mode)
        }
        Err(e) => {
            // Some formats may not support dimension probing; try full decode.
            let img = image::load_from_memory(raw).map_err(|e2| {
                format!("decode image (dimension probe failed: {e}; full decode: {e2})")
            })?;
            (img.width(), img.height(), format!("{:?}", img.color()).to_lowercase())
        }
    };

    let exif = parse_exif(raw);

    Ok(ImageMetadata {
        format,
        width,
        height,
        color_mode,
        pixel_count: width as u64 * height as u64,
        file_size,
        exif,
    })
}

/// Parse EXIF data from JPEG/TIFF bytes. Returns key-value pairs.
///
/// This is a lightweight parser that looks for JPEG APP1 (EXIF) markers.
/// It does not depend on an external EXIF crate; it extracts the most
/// commonly useful fields (camera make/model, date, GPS coordinates, etc.)
/// using a minimal TIFF IFD reader.
fn parse_exif(raw: &[u8]) -> Vec<(String, String)> {
    // Quick check: JPEG starts with 0xFFD8.
    if raw.len() < 4 || raw[0] != 0xFF || raw[1] != 0xD8 {
        return Vec::new();
    }

    // Scan for APP1 marker (0xFFE1) containing "Exif\0\0".
    let mut offset = 2;
    while offset + 4 < raw.len() {
        if raw[offset] != 0xFF {
            offset += 1;
            continue;
        }
        let marker = raw[offset + 1];
        if marker == 0xE1 {
            // APP1 segment length (big-endian).
            if offset + 10 >= raw.len() {
                break;
            }
            let seg_len = ((raw[offset + 2] as usize) << 8) | (raw[offset + 3] as usize);
            let seg_start = offset + 4;
            let seg_end = (seg_start + seg_len - 2).min(raw.len());

            // Check for "Exif\0\0" header.
            if seg_end > seg_start + 6
                && &raw[seg_start..seg_start + 6] == b"Exif\x00\x00"
            {
                let tiff_data = &raw[seg_start + 6..seg_end];
                return parse_tiff_ifd(tiff_data);
            }
        }
        // Skip this marker segment.
        if offset + 3 < raw.len() {
            let seg_len = ((raw[offset + 2] as usize) << 8) | (raw[offset + 3] as usize);
            offset += 2 + seg_len;
        } else {
            break;
        }
    }
    Vec::new()
}

/// Parse a minimal TIFF IFD0 to extract common EXIF tags.
fn parse_tiff_ifd(data: &[u8]) -> Vec<(String, String)> {
    if data.len() < 8 {
        return Vec::new();
    }

    let little_endian = data[0] == 0x49 && data[1] == 0x49; // "II"
    let big_endian = data[0] == 0x4D && data[1] == 0x4D; // "MM"
    if !little_endian && !big_endian {
        return Vec::new();
    }

    let read_u16 = |bytes: &[u8], offset: usize| -> u16 {
        if little_endian {
            u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
        } else {
            u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
        }
    };
    let read_u32 = |bytes: &[u8], offset: usize| -> u32 {
        if little_endian {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        } else {
            u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        }
    };

    let ifd0_offset = read_u32(data, 4) as usize;
    if ifd0_offset + 2 > data.len() {
        return Vec::new();
    }

    let entry_count = read_u16(data, ifd0_offset) as usize;
    let mut results = Vec::new();

    for i in 0..entry_count {
        let entry_offset = ifd0_offset + 2 + i * 12;
        if entry_offset + 12 > data.len() {
            break;
        }
        let tag = read_u16(data, entry_offset);
        let typ = read_u16(data, entry_offset + 2);
        let count = read_u32(data, entry_offset + 4) as usize;

        // Value/offset field (4 bytes).
        let value_offset = entry_offset + 8;

        let value = read_tag_value(data, tag, typ, count, value_offset, little_endian);
        if let Some(v) = value {
            let label = exif_tag_name(tag).to_string();
            results.push((label, v));
        }
    }

    results
}

fn read_tag_value(
    data: &[u8],
    tag: u16,
    typ: u16,
    count: usize,
    value_offset: usize,
    little_endian: bool,
) -> Option<String> {
    // ASCII (type 2): value is inline if count <= 4, else offset pointer.
    if typ == 2 {
        let read_u32 = |bytes: &[u8], offset: usize| -> u32 {
            if little_endian {
                u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            } else {
                u32::from_be_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            }
        };

        let str_data = if count <= 4 {
            &data[value_offset..value_offset + count]
        } else {
            let ptr = read_u32(data, value_offset) as usize;
            if ptr + count > data.len() {
                return None;
            }
            &data[ptr..ptr + count]
        };
        let s = std::str::from_utf8(str_data)
            .ok()?
            .trim_end_matches('\0')
            .trim()
            .to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else if typ == 3 {
        // SHORT (u16).
        let read_u16 = |bytes: &[u8], offset: usize| -> u16 {
            if little_endian {
                u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
            } else {
                u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
            }
        };
        if count == 1 {
            Some(read_u16(data, value_offset).to_string())
        } else {
            None
        }
    } else if typ == 4 {
        // LONG (u32).
        let read_u32 = |bytes: &[u8], offset: usize| -> u32 {
            if little_endian {
                u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            } else {
                u32::from_be_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            }
        };
        if count == 1 {
            Some(read_u32(data, value_offset).to_string())
        } else {
            None
        }
    } else if typ == 5 {
        // RATIONAL (two u32s: numerator/denominator), stored as offset.
        let read_u32 = |bytes: &[u8], offset: usize| -> u32 {
            if little_endian {
                u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            } else {
                u32::from_be_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            }
        };
        if count == 1 {
            let ptr = read_u32(data, value_offset) as usize;
            if ptr + 8 > data.len() {
                return None;
            }
            let num = read_u32(data, ptr);
            let den = read_u32(data, ptr + 4);
            if den == 0 {
                None
            } else {
                Some(format!("{num}/{den}"))
            }
        } else {
            None
        }
    } else {
        // Unknown type — skip.
        let _ = tag;
        None
    }
}

fn exif_tag_name(tag: u16) -> &'static str {
    match tag {
        0x010E => "ImageDescription",
        0x010F => "Make",
        0x0110 => "Model",
        0x0112 => "Orientation",
        0x011A => "XResolution",
        0x011B => "YResolution",
        0x0131 => "Software",
        0x0132 => "DateTime",
        0x013E => "WhitePoint",
        0x8298 => "Copyright",
        0x829A => "ExposureTime",
        0x829D => "FNumber",
        0x8822 => "ExposureProgram",
        0x8827 => "ISOSpeedRatings",
        0x9003 => "DateTimeOriginal",
        0x9004 => "DateTimeDigitized",
        0x9201 => "ShutterSpeedValue",
        0x9202 => "ApertureValue",
        0x9204 => "ExposureBiasValue",
        0x9207 => "MeteringMode",
        0x9208 => "LightSource",
        0x9209 => "Flash",
        0x920A => "FocalLength",
        0xA002 => "PixelXDimension",
        0xA003 => "PixelYDimension",
        0xA210 => "FocalPlaneResolutionUnit",
        _ => "Unknown",
    }
}
