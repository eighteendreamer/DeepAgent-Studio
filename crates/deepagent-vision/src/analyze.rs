//! Combined image analysis pipeline — produces a text description suitable
//! for feeding into a text-only chat model.
//!
//! The pipeline runs all pure-Rust analysers (metadata, colour, ASCII) and
//! optionally Tesseract OCR when a binary is available.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::ascii::{render_ascii, AsciiArt};
use crate::color::{analyze_colors, ColorAnalysis};
use crate::metadata::{extract_metadata, ImageMetadata};
use crate::ocr::{run_ocr, OcrResult};

/// Options controlling the analysis pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisOptions {
    /// Path to the Tesseract binary (or its parent directory). `None` disables OCR.
    pub tesseract_path: Option<String>,
    /// Path to the tessdata directory (for portable Tesseract installs).
    pub tessdata_dir: Option<String>,
    /// OCR language code: "eng", "chi_sim", "chi_sim+eng" (default).
    pub ocr_language: Option<String>,
    /// Whether to include the ASCII art rendering in the output.
    pub include_ascii: bool,
    /// Whether to include colour analysis in the output.
    pub include_color: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            tesseract_path: None,
            tessdata_dir: None,
            ocr_language: None,
            include_ascii: true,
            include_color: true,
        }
    }
}

/// The full analysis result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageAnalysis {
    /// Human-readable text description combining all analysis results.
    pub text: String,
    /// Structured metadata.
    pub metadata: ImageMetadata,
    /// Colour analysis (if enabled).
    pub color: Option<ColorAnalysis>,
    /// ASCII art rendering (if enabled).
    pub ascii: Option<AsciiArt>,
    /// OCR result (if Tesseract was available).
    pub ocr: Option<OcrResult>,
}

/// Analyse an image file on disk.
pub fn analyze_image(path: &Path, options: &AnalysisOptions) -> Result<ImageAnalysis, String> {
    let metadata = extract_metadata(path)?;
    let img = image::open(path).map_err(|e| format!("decode image: {e}"))?;

    let color = if options.include_color {
        Some(analyze_colors(&img))
    } else {
        None
    };

    let ascii = if options.include_ascii {
        Some(render_ascii(&img))
    } else {
        None
    };

    let ocr = if let Some(tess_path) = &options.tesseract_path {
        let tess_path = PathBuf::from(tess_path);
        let tessdata = options.tessdata_dir.as_deref().map(PathBuf::from);
        let result = run_ocr(
            path,
            Some(&tess_path),
            tessdata.as_deref(),
            options.ocr_language.as_deref(),
        );
        Some(result)
    } else {
        // Try PATH-based discovery.
        if crate::ocr::is_available(None) {
            Some(run_ocr(
                path,
                None,
                options
                    .tessdata_dir
                    .as_deref()
                    .map(PathBuf::from)
                    .as_deref(),
                options.ocr_language.as_deref(),
            ))
        } else {
            None
        }
    };

    let text = compose_text(&metadata, &color, &ascii, &ocr);

    Ok(ImageAnalysis {
        text,
        metadata,
        color,
        ascii,
        ocr,
    })
}

/// Compose the final text description from all analysis components.
fn compose_text(
    metadata: &ImageMetadata,
    color: &Option<ColorAnalysis>,
    ascii: &Option<AsciiArt>,
    ocr: &Option<OcrResult>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // 1. Basic metadata
    parts.push(format!(
        "图片信息：格式 {}，尺寸 {}×{} 像素，色彩模式 {}，文件大小 {} 字节",
        metadata.format, metadata.width, metadata.height, metadata.color_mode, metadata.file_size
    ));

    // 2. EXIF metadata (if any)
    if !metadata.exif.is_empty() {
        let exif_str: Vec<String> = metadata
            .exif
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        parts.push(format!("EXIF 元数据：{}", exif_str.join("；")));
    }

    // 3. Colour analysis
    if let Some(c) = color {
        let dominant: Vec<String> = c
            .dominant_colors
            .iter()
            .take(5)
            .map(|dc| format!("{}（{:.1}%）", dc.hex, dc.ratio * 100.0))
            .collect();
        parts.push(format!(
            "色彩分析：平均亮度 {:.0}/255（{}），主色调：{}",
            c.avg_brightness,
            c.brightness_level,
            dominant.join("、")
        ));

        if c.saturated_pixel_count > 0 {
            let regions: Vec<String> = c
                .color_regions
                .iter()
                .take(5)
                .map(|r| format!("{}（{:.1}%）", r.name, r.ratio))
                .collect();
            parts.push(format!(
                "彩色区域：饱和像素占 {:.1}%，主要色彩：{}",
                c.saturated_pixel_ratio,
                regions.join("、")
            ));
        }
    }

    // 4. OCR text (most valuable)
    if let Some(o) = ocr {
        if o.success && !o.text.is_empty() {
            parts.push(format!("图片文字识别（{}）：\n{}", o.language, o.text));
        } else if let Some(err) = &o.error {
            if !o.success {
                parts.push(format!("图片文字识别：失败（{err}）"));
            } else {
                parts.push("图片文字识别：未检测到文字".to_string());
            }
        }
    }

    // 5. ASCII art (layout reference)
    if let Some(a) = ascii {
        if a.width > 0 && a.height > 0 {
            parts.push(format!(
                "图片轮廓（ASCII 降采样 {}×{}）：\n```\n{}\n```",
                a.width, a.height, a.text
            ));
        }
    }

    parts.join("\n\n")
}
