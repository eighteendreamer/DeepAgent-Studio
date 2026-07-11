//! # deepagent-vision
//!
//! Pure-Rust local image analysis — no Python, no external ML runtime.
//!
//! The analyser combines several lightweight techniques to produce a text
//! description that a chat model can reason about:
//!
//! 1. **Image metadata** — dimensions, format, colour mode (via the `image` crate).
//! 2. **Pixel-level colour analysis** — dominant colours, brightness histogram,
//!    detection of saturated (non-grey) regions that may indicate UI elements
//!    like red error badges or blue links.
//! 3. **ASCII art rendering** — a downsampled character grid that preserves
//!    coarse layout and text-region shapes so the model can "see" structure.
//! 4. **EXIF / JPEG marker parsing** — extracts embedded metadata when present.
//! 5. **OCR** (optional) — when a Tesseract binary is available on the system
//!    or installed as a managed runtime, text is extracted via subprocess call.
//!
//! The entire pipeline runs in-process; the only external dependency is the
//! optional Tesseract executable.

pub mod analyze;
pub mod ascii;
pub mod color;
pub mod layout;
pub mod metadata;
pub mod ocr;

pub use analyze::{AnalysisOptions, ImageAnalysis};
