//! Local system-vision service.
//!
//! Pure-Rust image analysis — no Python, no external ML runtime required.
//! The pipeline combines:
//!   1. Image metadata (dimensions, format, colour mode, EXIF)
//!   2. Pixel-level colour analysis (dominant colours, brightness, saturated regions)
//!   3. ASCII art rendering (layout reference for the model)
//!   4. Optional Tesseract OCR (when a binary is available on PATH or installed
//!      as a managed runtime)
//!
//! The entire pipeline runs in-process; the only external dependency is the
//! optional Tesseract executable.

use std::path::PathBuf;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_vision::analyze::{analyze_image, AnalysisOptions};

use crate::dto::{VisionRecognizeRequestDto, VisionRecognizeResultDto};
use crate::runtime_service::RuntimeService;

/// Runtime capability for the Tesseract OCR engine.
const TESSERACT_CAPABILITY: &str = "vision-ocr-tesseract";

/// Model identifier reported back to the caller.
const VISION_MODEL_ID: &str = "deepagent-vision-rust";

#[derive(Clone)]
pub struct VisionService {
    runtime: Arc<RuntimeService>,
}

impl VisionService {
    pub fn new(runtime: Arc<RuntimeService>) -> Self {
        Self { runtime }
    }

    pub fn recognize_image(
        &self,
        request: VisionRecognizeRequestDto,
    ) -> Result<VisionRecognizeResultDto> {
        let image_path = PathBuf::from(&request.image_path);
        if !image_path.is_file() {
            return Err(CoreError::Other(format!(
                "image file not found: {}",
                image_path.display()
            )));
        }

        // Resolve Tesseract binary: check managed runtime first, then PATH.
        let (tesseract_path, tessdata_dir) = self.resolve_tesseract();

        let options = AnalysisOptions {
            tesseract_path: tesseract_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            tessdata_dir: tessdata_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            ocr_language: request.prompt.as_deref().map(|p| {
                // The prompt field is reused as the OCR language code.
                // Empty or "auto" falls back to the default (chi_sim+eng).
                if p.trim().is_empty() || p == "auto" {
                    "chi_sim+eng".to_string()
                } else {
                    p.to_string()
                }
            }),
            include_ascii: true,
            include_color: true,
        };

        let analysis =
            analyze_image(&image_path, &options).map_err(|e| CoreError::Other(e))?;

        // Serialize the full structured result as raw_json.
        let raw_json = serde_json::to_string(&analysis)
            .unwrap_or_else(|_| "{}".to_string());

        // The `text` field is the human-readable composite description.
        let text = analysis.text;

        // Determine the "model path" to report.
        let model_path = tesseract_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rust-in-process".to_string());

        if text.trim().is_empty() {
            return Err(CoreError::Other(
                "image analysis returned empty text".to_string(),
            ));
        }

        Ok(VisionRecognizeResultDto {
            model_id: VISION_MODEL_ID.to_string(),
            model_path,
            text,
            raw_json,
        })
    }

    /// Check whether Tesseract OCR is available (either as a managed runtime
    /// or on the system PATH). Exposed so the Tauri layer can tell the UI
    /// whether to prompt the user to download it.
    pub fn ocr_available(&self) -> bool {
        if let Some(dir) = self.runtime.resolve(TESSERACT_CAPABILITY) {
            return deepagent_vision::ocr::is_available(Some(&dir));
        }
        deepagent_vision::ocr::is_available(None)
    }

    /// Resolve the Tesseract binary path and tessdata directory.
    ///
    /// Returns `(tesseract_path, tessdata_dir)` where either may be `None`.
    fn resolve_tesseract(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        if let Some(dir) = self.runtime.resolve(TESSERACT_CAPABILITY) {
            // Managed runtime: the directory contains tesseract.exe and a
            // tessdata/ subdirectory.
            let tessdata = dir.join("tessdata");
            let tessdata_dir = if tessdata.is_dir() {
                Some(tessdata)
            } else {
                None
            };
            return (Some(dir), tessdata_dir);
        }

        // Check PATH — no tessdata override needed (system Tesseract uses
        // its own compiled data path).
        if deepagent_vision::ocr::is_available(None) {
            return (None, None);
        }

        (None, None)
    }
}
