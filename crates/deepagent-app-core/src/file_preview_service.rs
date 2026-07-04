//! File preview for the desktop "File Preview" panel (office-agent Phase 1).
//!
//! Produces a previewable representation of a user-selected file using **pure
//! Rust** (Tier C — always available, no external runtime):
//! - `txt / md / json / csv` → raw text (csv is rendered as a table by the UI),
//! - `docx / pptx` → extracted body text (OOXML container via `zip`, text runs
//!   pulled from `w:t` / `a:t` elements),
//! - `xlsx` → first-N-rows-per-sheet via `calamine`,
//! - `pdf` → text extraction is added in Phase 1 task 7 (placeholder here),
//! - images → no text; the UI displays them directly from the path.
//!
//! Higher-fidelity rendering (PDF page rasterization, pptx thumbnails) is a
//! Tier R upgrade handled by later phases via the runtime manager. This
//! service never spawns external processes.

use std::io::Read;
use std::path::Path;

use deepagent_core::error::{CoreError, Result};

use crate::dto::{PreviewMetadataDto, PreviewResultDto, SheetPreviewDto};

/// Cap on extracted text returned to the UI (1 MiB). Keeps the IPC payload and
/// the webview render bounded for very large documents.
const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Max rows previewed per xlsx sheet.
const MAX_SHEET_ROWS: usize = 200;

/// Max cap on a single file we will attempt to read into memory (64 MiB).
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Max image size we will inline as a data URL (16 MiB).
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// Bytes sampled when deciding whether an unknown file is actually text.
const TEXT_SNIFF_BYTES: usize = 8 * 1024;

/// Stateless service that reads files and returns preview DTOs. Holds no
/// handles — every call is independent, mirroring the thin-service convention.
#[derive(Debug, Default, Clone)]
pub struct FilePreviewService;

impl FilePreviewService {
    /// Construct the service.
    pub fn new() -> Self {
        Self
    }

    /// Classify a file by extension into a coarse preview `kind`.
    fn classify(path: &Path, ext: &str) -> &'static str {
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match ext {
            "txt" | "md" | "markdown" | "json" | "log" | "yaml" | "yml" | "toml" | "xml" | "ts"
            | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "css" | "scss" | "sass" | "less" | "html"
            | "htm" | "vue" | "svelte" | "java" | "kt" | "kts" | "gradle" | "groovy" | "rs"
            | "go" | "py" | "rb" | "php" | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx"
            | "cs" | "swift" | "sql" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd"
            | "conf" | "cfg" | "ini" | "properties" | "env" | "lock" => "text",
            "csv" | "tsv" => "csv",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" => "image",
            "pdf" => "pdf",
            "docx" => "docx",
            "xlsx" => "xlsx",
            "pptx" => "pptx",
            _ => {
                if file_name.starts_with('.') {
                    "text"
                } else {
                    match file_name.as_str() {
                        "dockerfile" | "makefile" => "text",
                        _ => "unknown",
                    }
                }
            }
        }
    }

    /// Read metadata (name, extension, size, classified kind) for `path`.
    pub fn get_metadata(&self, path: &str) -> Result<PreviewMetadataDto> {
        let p = Path::new(path);
        let meta = std::fs::metadata(p)
            .map_err(|e| CoreError::Other(format!("cannot stat '{path}': {e}")))?;
        if !meta.is_file() {
            return Err(CoreError::Other(format!("'{path}' is not a file")));
        }
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = p
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Ok(PreviewMetadataDto {
            path: path.to_string(),
            name,
            ext: ext.clone(),
            size_bytes: meta.len(),
            kind: Self::classify(p, &ext).to_string(),
        })
    }

    /// Extract a previewable representation of `path`, dispatching on kind.
    pub fn extract_text(&self, path: &str) -> Result<PreviewResultDto> {
        let metadata = self.get_metadata(path)?;
        if metadata.size_bytes > MAX_FILE_BYTES {
            return Ok(result(
                metadata,
                None,
                None,
                false,
                Some("file is too large to preview".to_string()),
            ));
        }

        match metadata.kind.as_str() {
            "text" | "csv" => {
                let (text, truncated) = read_text_capped(path)?;
                Ok(result(metadata, Some(text), None, truncated, None))
            }
            "docx" => {
                let text = extract_docx_text(path)?;
                let (text, truncated) = cap_text(text);
                Ok(result(metadata, Some(text), None, truncated, None))
            }
            "pptx" => {
                let text = extract_pptx_text(path)?;
                let (text, truncated) = cap_text(text);
                Ok(result(metadata, Some(text), None, truncated, None))
            }
            "xlsx" => {
                let sheets = extract_xlsx_sheets(path)?;
                Ok(result(metadata, None, Some(sheets), false, None))
            }
            "image" => Ok(result(
                metadata,
                None,
                None,
                false,
                Some("image is rendered directly by the viewer".to_string()),
            )),
            "pdf" => {
                // Tier C: pure-Rust text extraction. Page rasterization is a
                // Tier R upgrade (pdfium) handled in a later phase. Extraction
                // failures degrade to a readable note rather than erroring.
                match extract_pdf_text(path) {
                    Ok(text) if !text.trim().is_empty() => {
                        let (text, truncated) = cap_text(text);
                        Ok(result(
                            metadata,
                            Some(text),
                            None,
                            truncated,
                            Some(
                                "text-only preview (install pdfium for page rendering)".to_string(),
                            ),
                        ))
                    }
                    Ok(_) => Ok(result(
                        metadata,
                        None,
                        None,
                        false,
                        Some(
                            "no extractable text (scanned PDF?) — page rendering needs pdfium"
                                .to_string(),
                        ),
                    )),
                    Err(e) => Ok(result(
                        metadata,
                        None,
                        None,
                        false,
                        Some(format!("PDF text extraction failed: {e}")),
                    )),
                }
            }
            _ => {
                let bytes = std::fs::read(path)
                    .map_err(|e| CoreError::Other(format!("read '{path}': {e}")))?;
                if is_likely_text(&bytes) {
                    let (text, truncated) = cap_bytes_to_text(&bytes);
                    Ok(result(metadata, Some(text), None, truncated, None))
                } else {
                    Ok(result(
                        metadata,
                        None,
                        None,
                        false,
                        Some("preview not supported for this file type".to_string()),
                    ))
                }
            }
        }
    }

    /// Read an image file and return it as a `data:` URL (base64), so the
    /// webview can display it without the Tauri asset protocol. Size-capped to
    /// [`MAX_IMAGE_BYTES`]; non-image kinds are rejected.
    pub fn read_data_url(&self, path: &str) -> Result<String> {
        let metadata = self.get_metadata(path)?;
        if metadata.kind != "image" {
            return Err(CoreError::Other(format!(
                "'{path}' is not a previewable image"
            )));
        }
        if metadata.size_bytes > MAX_IMAGE_BYTES {
            return Err(CoreError::Other(
                "image is too large to preview".to_string(),
            ));
        }
        let bytes =
            std::fs::read(path).map_err(|e| CoreError::Other(format!("read '{path}': {e}")))?;
        let mime = image_mime(&metadata.ext);
        Ok(format!("data:{mime};base64,{}", base64_encode(&bytes)))
    }
}

/// Build a [`PreviewResultDto`].
fn result(
    metadata: PreviewMetadataDto,
    text: Option<String>,
    sheets: Option<Vec<SheetPreviewDto>>,
    truncated: bool,
    message: Option<String>,
) -> PreviewResultDto {
    PreviewResultDto {
        metadata,
        text,
        sheets,
        truncated,
        message,
    }
}

/// Read a UTF-8(-lossy) text file, capping at [`MAX_TEXT_BYTES`].
fn read_text_capped(path: &str) -> Result<(String, bool)> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::Other(format!("read '{path}': {e}")))?;
    Ok(cap_bytes_to_text(&bytes))
}

/// Truncate already-decoded text to the byte cap (on a char boundary).
fn cap_text(text: String) -> (String, bool) {
    if text.len() <= MAX_TEXT_BYTES {
        return (text, false);
    }
    let mut end = MAX_TEXT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Decode bytes lossily and cap to [`MAX_TEXT_BYTES`].
fn cap_bytes_to_text(bytes: &[u8]) -> (String, bool) {
    let truncated = bytes.len() > MAX_TEXT_BYTES;
    let slice = if truncated {
        &bytes[..MAX_TEXT_BYTES]
    } else {
        bytes
    };
    (String::from_utf8_lossy(slice).into_owned(), truncated)
}

/// Best-effort detector for "unknown" file types that are still plain text.
fn is_likely_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }

    let sample = &bytes[..bytes.len().min(TEXT_SNIFF_BYTES)];

    if sample.starts_with(&[0xEF, 0xBB, 0xBF])
        || sample.starts_with(&[0xFE, 0xFF])
        || sample.starts_with(&[0xFF, 0xFE])
    {
        return true;
    }

    if sample.contains(&0) {
        return false;
    }

    let suspicious = sample
        .iter()
        .filter(|&&b| {
            !(b == b'\n'
                || b == b'\r'
                || b == b'\t'
                || b == 0x0C
                || b == 0x1B
                || (0x20..=0x7E).contains(&b)
                || b >= 0x80)
        })
        .count();

    suspicious * 10 <= sample.len()
}

/// Extract text from a PDF (Tier C, pure Rust via `pdf-extract`). Returns the
/// concatenated text; rasterization of pages is a Tier R upgrade (pdfium).
fn extract_pdf_text(path: &str) -> Result<String> {
    pdf_extract::extract_text(path)
        .map_err(|e| CoreError::Other(format!("extract pdf '{path}': {e}")))
}

/// Read one entry of an OOXML zip container into a String.
fn read_zip_entry(path: &str, entry: &str) -> Result<Option<String>> {
    let file =
        std::fs::File::open(path).map_err(|e| CoreError::Other(format!("open '{path}': {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Other(format!("'{path}' is not a valid zip/OOXML file: {e}")))?;
    let mut f = match zip.by_name(entry) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(CoreError::Other(format!("read '{entry}' in '{path}': {e}"))),
    };
    let mut s = String::new();
    f.read_to_string(&mut s)
        .map_err(|e| CoreError::Other(format!("read '{entry}' in '{path}': {e}")))?;
    Ok(Some(s))
}

/// Extract body text from a `.docx`: text runs (`<w:t>`) joined per paragraph
/// (`</w:p>` → newline).
fn extract_docx_text(path: &str) -> Result<String> {
    let xml = read_zip_entry(path, "word/document.xml")?
        .ok_or_else(|| CoreError::Other(format!("'{path}' has no word/document.xml")))?;
    Ok(ooxml_text(&xml, "</w:p>", "w:t"))
}

/// Extract text from a `.pptx`: every slide's `<a:t>` runs, in slide order.
fn extract_pptx_text(path: &str) -> Result<String> {
    let file =
        std::fs::File::open(path).map_err(|e| CoreError::Other(format!("open '{path}': {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Other(format!("'{path}' is not a valid pptx: {e}")))?;

    // Collect slide entry names, sorted by the numeric suffix so slides come
    // out in presentation order (slide1, slide2, … slide10).
    let mut slides: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .collect();
    slides.sort_by_key(|n| slide_number(n));

    let mut out = String::new();
    for name in slides {
        if let Some(xml) = read_zip_entry(path, &name)? {
            let text = ooxml_text(&xml, "</a:p>", "a:t");
            if !text.trim().is_empty() {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&text);
            }
        }
    }
    Ok(out)
}

/// Parse the numeric slide index from a name like `ppt/slides/slide12.xml`.
fn slide_number(name: &str) -> u32 {
    name.trim_start_matches("ppt/slides/slide")
        .trim_end_matches(".xml")
        .parse()
        .unwrap_or(u32::MAX)
}

/// Extract text from OOXML: split on the paragraph-close tag, pull text runs
/// from `text_tag` elements within each paragraph, one line per paragraph.
fn ooxml_text(xml: &str, para_close: &str, text_tag: &str) -> String {
    let mut lines = Vec::new();
    for segment in xml.split(para_close) {
        let line = extract_runs(segment, text_tag);
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Collect and concatenate the text inside every `<text_tag ...>…</text_tag>`
/// element in `segment`, decoding the basic XML entities. Self-closing tags
/// (`<w:t/>`) carry no text and are skipped.
fn extract_runs(segment: &str, text_tag: &str) -> String {
    let open = format!("<{text_tag}");
    let close = format!("</{text_tag}>");
    let mut out = String::new();
    let mut rest = segment;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else { break };
        // Skip self-closing `<w:t/>`.
        if after.as_bytes().get(gt.wrapping_sub(1)) == Some(&b'/') {
            rest = &after[gt + 1..];
            continue;
        }
        let content_start = start + gt + 1;
        let Some(close_rel) = rest[content_start..].find(&close) else {
            break;
        };
        let content = &rest[content_start..content_start + close_rel];
        out.push_str(&decode_entities(content));
        rest = &rest[content_start + close_rel + close.len()..];
    }
    out
}

/// Decode the five predefined XML entities. `&amp;` is decoded last so an
/// already-decoded `&` is not re-interpreted.
fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// MIME type for an image extension (used by [`FilePreviewService::read_data_url`]).
fn image_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Standard base64 encoder (no padding omitted) — dependency-free so the
/// kernel workspace doesn't pull a base64 crate just for image previews.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0b11) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0b1111) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[b2 & 0b111111] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Read the first [`MAX_SHEET_ROWS`] rows of every sheet in an xlsx workbook.
fn extract_xlsx_sheets(path: &str) -> Result<Vec<SheetPreviewDto>> {
    use calamine::{open_workbook_auto, Reader};

    let mut workbook = open_workbook_auto(path)
        .map_err(|e| CoreError::Other(format!("open xlsx '{path}': {e}")))?;
    let names = workbook.sheet_names().to_owned();
    let mut sheets = Vec::with_capacity(names.len());
    for name in names {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| CoreError::Other(format!("read sheet '{name}' in '{path}': {e}")))?;
        let total = range.rows().count();
        let rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_SHEET_ROWS)
            .map(|row| row.iter().map(|cell| cell.to_string()).collect())
            .collect();
        sheets.push(SheetPreviewDto {
            name,
            rows,
            truncated: total > MAX_SHEET_ROWS,
        });
    }
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        let p = path.to_string_lossy().into_owned();
        (dir, p)
    }

    #[test]
    fn classify_covers_office_and_text() {
        assert_eq!(
            FilePreviewService::classify(Path::new("a.md"), "md"),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("a.ts"), "ts"),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("index.html"), "html"),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("csv.csv"), "csv"),
            "csv"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("img.png"), "png"),
            "image"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("doc.docx"), "docx"),
            "docx"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("book.xlsx"), "xlsx"),
            "xlsx"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("deck.pptx"), "pptx"),
            "pptx"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("scan.pdf"), "pdf"),
            "pdf"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("Dockerfile"), ""),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new(".gitignore"), ""),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new(".npmrc"), ""),
            "text"
        );
        assert_eq!(
            FilePreviewService::classify(Path::new("a.zip"), "zip"),
            "unknown"
        );
    }

    #[test]
    fn metadata_reports_name_ext_kind() {
        let (_d, path) = write_temp("notes.md", b"# hello");
        let svc = FilePreviewService::new();
        let m = svc.get_metadata(&path).unwrap();
        assert_eq!(m.name, "notes.md");
        assert_eq!(m.ext, "md");
        assert_eq!(m.kind, "text");
        assert_eq!(m.size_bytes, 7);
    }

    #[test]
    fn extracts_plain_text() {
        let (_d, path) = write_temp("a.txt", b"line1\nline2");
        let svc = FilePreviewService::new();
        let r = svc.extract_text(&path).unwrap();
        assert_eq!(r.text.as_deref(), Some("line1\nline2"));
        assert!(!r.truncated);
        assert_eq!(r.metadata.kind, "text");
    }

    #[test]
    fn extracts_unknown_text_like_file() {
        let (_d, path) = write_temp("post-update.sample", b"#!/bin/sh\necho ok\n");
        let svc = FilePreviewService::new();
        let r = svc.extract_text(&path).unwrap();
        assert_eq!(r.metadata.kind, "unknown");
        assert_eq!(r.text.as_deref(), Some("#!/bin/sh\necho ok\n"));
        assert!(r.message.is_none());
    }

    #[test]
    fn ooxml_text_pulls_runs_per_paragraph() {
        let xml = r#"<w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world</w:t></w:r></w:p><w:p><w:r><w:t>Second &amp; line</w:t></w:r></w:p></w:body>"#;
        let text = ooxml_text(xml, "</w:p>", "w:t");
        assert_eq!(text, "Hello world\nSecond & line");
    }

    #[test]
    fn extract_runs_skips_self_closing() {
        let seg = r#"<w:r><w:t/></w:r><w:r><w:t>x</w:t></w:r>"#;
        assert_eq!(extract_runs(seg, "w:t"), "x");
    }

    #[test]
    fn decode_entities_handles_amp_last() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
    }

    #[test]
    fn base64_encodes_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn read_data_url_rejects_non_image() {
        let (_d, path) = write_temp("a.txt", b"hi");
        let svc = FilePreviewService::new();
        assert!(svc.read_data_url(&path).is_err());
    }

    #[test]
    fn read_data_url_builds_data_uri_for_image() {
        // 1x1 transparent GIF bytes — content doesn't matter, only the path/ext.
        let (_d, path) = write_temp("p.png", b"\x89PNG\r\n\x1a\n");
        let svc = FilePreviewService::new();
        let url = svc.read_data_url(&path).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn slide_number_parses_and_sorts() {
        assert_eq!(slide_number("ppt/slides/slide2.xml"), 2);
        assert_eq!(slide_number("ppt/slides/slide12.xml"), 12);
    }

    #[test]
    fn extracts_docx_from_minimal_zip() {
        // Build a minimal .docx (zip with word/document.xml).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.docx");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(
            br#"<w:document><w:body><w:p><w:r><w:t>Hello docx</w:t></w:r></w:p></w:body></w:document>"#,
        )
        .unwrap();
        zip.finish().unwrap();

        let svc = FilePreviewService::new();
        let r = svc.extract_text(&path.to_string_lossy()).unwrap();
        assert_eq!(r.metadata.kind, "docx");
        assert_eq!(r.text.as_deref(), Some("Hello docx"));
    }

    #[test]
    fn unsupported_type_returns_message_not_error() {
        let (_d, path) = write_temp("a.bin", b"\x00\x01\x02");
        let svc = FilePreviewService::new();
        let r = svc.extract_text(&path).unwrap();
        assert_eq!(r.metadata.kind, "unknown");
        assert!(r.text.is_none());
        assert!(r.message.is_some());
    }
}
