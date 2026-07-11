//! OCR via Tesseract subprocess — the only external dependency in the vision
//! pipeline.
//!
//! Tesseract is expected to be available either:
//! - On the system `PATH` (user installed it themselves), or
//! - At a specific path provided by the caller (managed-runtime download).
//!
//! The module never bundles Tesseract itself; it discovers and invokes it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// OCR result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OcrResult {
    /// Recognised text (may be empty if no text was found).
    pub text: String,
    /// Language code used: "eng", "chi_sim", "chi_sim+eng", etc.
    pub language: String,
    /// Whether the Tesseract binary was found and ran successfully.
    pub success: bool,
    /// Error message if Tesseract failed to run or was not found.
    pub error: Option<String>,
}

/// Default language preference: Chinese + English.
const DEFAULT_LANG: &str = "chi_sim+eng";

/// Check whether a Tesseract binary is reachable.
///
/// `tesseract_path` overrides the PATH lookup when provided (used by the
/// managed-runtime installer to point at a downloaded portable Tesseract).
pub fn is_available(tesseract_path: Option<&Path>) -> bool {
    let (program, pre_args) = resolve_tesseract(tesseract_path);
    let mut cmd = Command::new(&program);
    cmd.args(&pre_args).arg("--version");
    hide_command_window(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// Run Tesseract OCR on an image file.
///
/// `tesseract_path` is the path to the Tesseract binary (or `None` to search
/// `PATH`). `tessdata_dir` optionally points at the directory containing
/// `.traineddata` language files (needed for portable Tesseract installs).
pub fn run_ocr(
    image_path: &Path,
    tesseract_path: Option<&Path>,
    tessdata_dir: Option<&Path>,
    language: Option<&str>,
) -> OcrResult {
    let lang = language.unwrap_or(DEFAULT_LANG);

    if !image_path.is_file() {
        return OcrResult {
            text: String::new(),
            language: lang.to_string(),
            success: false,
            error: Some(format!("image file not found: {}", image_path.display())),
        };
    }

    let (program, pre_args) = resolve_tesseract(tesseract_path);

    // Tesseract writes recognised text to a file; we use stdout (-).
    let mut cmd = Command::new(&program);
    for arg in &pre_args {
        cmd.arg(arg);
    }
    cmd.arg(image_path);
    cmd.arg("stdout"); // output to stdout
    cmd.arg("-l");
    cmd.arg(lang);

    if let Some(dir) = tessdata_dir {
        cmd.env("TESSDATA_PREFIX", dir);
    }
    hide_command_window(&mut cmd);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return OcrResult {
                text: String::new(),
                language: lang.to_string(),
                success: false,
                error: Some(format!(
                    "Tesseract not found or failed to launch: {e}. \
                     Install Tesseract or download it from Settings."
                )),
            };
        }
    };

    if !output.status.success() && lang != "eng" {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Failed loading language")
            || stderr.contains("Error opening data file")
            || stderr.contains("Tesseract couldn't load any languages")
        {
            return run_ocr(image_path, tesseract_path, tessdata_dir, Some("eng"));
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return OcrResult {
            text: String::new(),
            language: lang.to_string(),
            success: false,
            error: Some(format!(
                "Tesseract exited with code {:?}: {}",
                output.status.code(),
                stderr
            )),
        };
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let is_empty = text.is_empty();

    OcrResult {
        text,
        language: lang.to_string(),
        success: true,
        error: if is_empty {
            Some("no text detected in image".to_string())
        } else {
            None
        },
    }
}

fn hide_command_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Resolve the Tesseract binary path and any prefix arguments.
///
/// On Windows, if `tesseract_path` points at a directory (the managed-runtime
/// install dir), we look for `tesseract.exe` inside it. If it points at a
/// file, we use it directly.
fn resolve_tesseract(tesseract_path: Option<&Path>) -> (PathBuf, Vec<String>) {
    if let Some(path) = tesseract_path {
        if path.is_dir() {
            // Managed runtime: <dir>/tesseract.exe
            let exe = path.join(if cfg!(windows) {
                "tesseract.exe"
            } else {
                "tesseract"
            });
            return (exe, Vec::new());
        }
        return (path.to_path_buf(), Vec::new());
    }

    // Fall back to PATH.
    let program = if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    };
    (PathBuf::from(program), Vec::new())
}
