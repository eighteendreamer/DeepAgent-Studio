//! Office document service (office-agent Phase 3).
//!
//! Three-tier capability resolution for office documents:
//! - **Tier R** (managed runtime present, e.g. pandoc / LibreOffice) → high
//!   fidelity / legacy formats. Resolved via [`RuntimeService`].
//! - **Tier C** (always available, pure Rust) → read via the file-preview
//!   extractor, and *generate* docx/xlsx from a structured [`DocSpec`].
//!
//! Tier C generation is the keystone of the office plan: the system LLM (via
//! [`ChatService`], wired by the caller) turns content into a [`DocSpec`], and
//! this service materializes it into a real `.docx` / `.xlsx` with **no
//! external runtime and no Python**.

use std::io::Write;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

use crate::file_preview_service::FilePreviewService;
use crate::runtime_service::RuntimeService;

/// The capability id provided by document-conversion runtimes (pandoc /
/// LibreOffice). When resolvable, Tier R is available.
const DOC_CONVERT_CAPABILITY: &str = "doc-convert";

/// A structured document the Tier C writers can materialize. Produced by the
/// LLM (from a transcript / source content) or assembled directly by callers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocSpec {
    /// Optional document title (rendered as a top heading).
    pub title: Option<String>,
    /// Ordered content blocks.
    pub blocks: Vec<DocBlock>,
}

/// One block of a [`DocSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocBlock {
    /// A heading at `level` (1..=6).
    Heading { level: u8, text: String },
    /// A body paragraph.
    Paragraph(String),
    /// A bullet list item.
    Bullet(String),
    /// A simple table (first row treated as header by the renderer).
    Table(Vec<Vec<String>>),
}

/// Office document operations. Reads delegate to the pure-Rust file-preview
/// extractor; generation uses the built-in OOXML writers.
pub struct OfficeService {
    runtime: Arc<RuntimeService>,
    preview: FilePreviewService,
}

impl OfficeService {
    /// Build over the runtime manager (for Tier R resolution).
    pub fn new(runtime: Arc<RuntimeService>) -> Self {
        Self {
            runtime,
            preview: FilePreviewService::new(),
        }
    }

    /// The tier currently used for generation/conversion: `"R"` when a
    /// conversion runtime is installed, else `"C"` (pure Rust).
    pub fn tier(&self) -> &'static str {
        if self.runtime.resolve(DOC_CONVERT_CAPABILITY).is_some() {
            "R"
        } else {
            "C"
        }
    }

    /// Extract readable text from an office/text file (Tier C). Returns the
    /// extracted text, or a sheet dump for xlsx.
    pub fn read_text(&self, path: &str) -> Result<String> {
        let r = self.preview.extract_text(path)?;
        if let Some(text) = r.text {
            return Ok(text);
        }
        if let Some(sheets) = r.sheets {
            let mut out = String::new();
            for s in sheets {
                out.push_str(&format!("# {}\n", s.name));
                for row in s.rows {
                    out.push_str(&row.join("\t"));
                    out.push('\n');
                }
                out.push('\n');
            }
            return Ok(out);
        }
        Ok(String::new())
    }

    /// Create a `.docx` from a [`DocSpec`] (Tier C, pure Rust).
    pub fn create_docx(&self, spec: &DocSpec, out_path: &str) -> Result<()> {
        let document_xml = render_document_xml(spec);
        write_docx_zip(out_path, &document_xml)
    }

    /// Create a `.docx` from a Markdown source. Prefers Tier R (pandoc, when a
    /// `doc-convert` runtime is installed) for higher fidelity; otherwise falls
    /// back to Tier C (parse → pure-Rust OOXML writer).
    pub fn create_docx_from_markdown(
        &self,
        markdown: &str,
        title: Option<&str>,
        out_path: &str,
    ) -> Result<()> {
        if let Some(dir) = self.runtime.resolve(DOC_CONVERT_CAPABILITY) {
            if let Some(pandoc) = pandoc_executable(&dir) {
                return convert_markdown_to_docx_via_pandoc(&pandoc, markdown, title, out_path);
            }
        }
        // Tier C: pure-Rust materialization.
        let mut spec = markdown_to_docspec(markdown);
        if spec.title.is_none() {
            spec.title = title.map(|s| s.to_string());
        }
        self.create_docx(&spec, out_path)
    }

    /// Create an `.xlsx` from named sheets of string rows (Tier C, pure Rust).
    pub fn create_xlsx(&self, sheets: &[(String, Vec<Vec<String>>)], out_path: &str) -> Result<()> {
        use rust_xlsxwriter::Workbook;
        let mut workbook = Workbook::new();
        for (name, rows) in sheets {
            let sheet = workbook.add_worksheet();
            if !name.is_empty() {
                sheet
                    .set_name(name.as_str())
                    .map_err(|e| CoreError::Other(format!("sheet name '{name}': {e}")))?;
            }
            for (r, row) in rows.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    sheet
                        .write_string(r as u32, c as u16, cell.as_str())
                        .map_err(|e| CoreError::Other(format!("write cell: {e}")))?;
                }
            }
        }
        workbook
            .save(out_path)
            .map_err(|e| CoreError::Other(format!("save xlsx '{out_path}': {e}")))?;
        Ok(())
    }

    /// Render PDF pages to PNG images when a pdfium runtime is installed (Tier
    /// R); otherwise degrade to extracted text (Tier C) with a note. Page PNGs
    /// are written under `out_dir`.
    pub fn render_pdf_pages(
        &self,
        pdf_path: &str,
        out_dir: &str,
        max_pages: usize,
    ) -> Result<crate::dto::PdfRenderResultDto> {
        if let Some(pdfium_dir) = self.runtime.resolve("pdf-render") {
            match render_pdf_with_pdfium(
                &pdfium_dir,
                pdf_path,
                std::path::Path::new(out_dir),
                max_pages,
            ) {
                Ok(pages) => {
                    return Ok(crate::dto::PdfRenderResultDto {
                        rendered: true,
                        pages,
                        text: None,
                        message: None,
                    })
                }
                Err(e) => {
                    return Ok(crate::dto::PdfRenderResultDto {
                        rendered: false,
                        pages: Vec::new(),
                        text: self.read_text(pdf_path).ok(),
                        message: Some(format!("pdfium render unavailable, showing text: {e}")),
                    })
                }
            }
        }
        Ok(crate::dto::PdfRenderResultDto {
            rendered: false,
            pages: Vec::new(),
            text: self.read_text(pdf_path).ok(),
            message: Some("install the pdfium runtime to render PDF pages as images".to_string()),
        })
    }

    /// Convert a document via LibreOffice (Tier R office-suite) — legacy
    /// formats (.doc/.xls/.ppt) and high-fidelity PDF export. Returns the
    /// output file path. Errors clearly when LibreOffice is not installed.
    pub fn convert_via_libreoffice(
        &self,
        input_path: &str,
        target_ext: &str,
        out_dir: &str,
    ) -> Result<String> {
        let suite = self
            .runtime
            .resolve("office-suite")
            .ok_or_else(|| CoreError::Other("LibreOffice runtime not installed".to_string()))?;
        let soffice = libreoffice_executable(&suite).ok_or_else(|| {
            CoreError::Other("soffice not found in LibreOffice runtime".to_string())
        })?;
        std::fs::create_dir_all(out_dir)
            .map_err(|e| CoreError::Other(format!("create out dir: {e}")))?;
        let status = std::process::Command::new(&soffice)
            .args([
                "--headless",
                "--convert-to",
                target_ext,
                "--outdir",
                out_dir,
                input_path,
            ])
            .status()
            .map_err(|e| CoreError::Other(format!("run soffice: {e}")))?;
        if !status.success() {
            return Err(CoreError::Other(format!(
                "LibreOffice conversion failed ({status})"
            )));
        }
        let stem = std::path::Path::new(input_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".to_string());
        Ok(std::path::Path::new(out_dir)
            .join(format!("{stem}.{target_ext}"))
            .to_string_lossy()
            .into_owned())
    }
}

/// Locate the pandoc executable inside an installed `doc-convert` runtime dir.
fn pandoc_executable(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "pandoc.exe"
    } else {
        "pandoc"
    };
    [dir.join(name), dir.join("bin").join(name)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Locate the `soffice` executable inside an installed LibreOffice runtime dir.
fn libreoffice_executable(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "soffice.exe"
    } else {
        "soffice"
    };
    [
        dir.join(name),
        dir.join("program").join(name),
        dir.join("App")
            .join("libreoffice")
            .join("program")
            .join(name),
        dir.join("opt")
            .join("libreoffice")
            .join("program")
            .join(name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// Render PDF pages to PNGs via pdfium (Tier R). Real implementation behind the
/// `pdfium` feature; otherwise reports the feature is disabled so the caller
/// degrades to text.
#[cfg(feature = "pdfium")]
fn render_pdf_with_pdfium(
    pdfium_dir: &std::path::Path,
    pdf_path: &str,
    out_dir: &std::path::Path,
    max_pages: usize,
) -> Result<Vec<String>> {
    use pdfium_render::prelude::*;

    std::fs::create_dir_all(out_dir)
        .map_err(|e| CoreError::Other(format!("create page dir: {e}")))?;

    // pdfium ships its library under lib/ or bin/ depending on the platform
    // package; probe both, then the dir root.
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
        &pdfium_dir.join("lib"),
    ))
    .or_else(|_| {
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
            &pdfium_dir.join("bin"),
        ))
    })
    .or_else(|_| Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(pdfium_dir)))
    .map_err(|e| CoreError::Other(format!("bind pdfium library: {e}")))?;

    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| CoreError::Other(format!("load pdf: {e}")))?;
    let config = PdfRenderConfig::new().set_target_width(1200);

    let mut paths = Vec::new();
    for (i, page) in document.pages().iter().enumerate() {
        if i >= max_pages {
            break;
        }
        let image = page
            .render_with_config(&config)
            .map_err(|e| CoreError::Other(format!("render page {}: {e}", i + 1)))?
            .as_image();
        let out = out_dir.join(format!("page-{}.png", i + 1));
        image
            .save(&out)
            .map_err(|e| CoreError::Other(format!("save page png: {e}")))?;
        paths.push(out.to_string_lossy().into_owned());
    }
    Ok(paths)
}

/// See the feature-gated variant. Without `pdfium`, page rendering is
/// unavailable and the caller degrades to text extraction.
#[cfg(not(feature = "pdfium"))]
fn render_pdf_with_pdfium(
    _pdfium_dir: &std::path::Path,
    _pdf_path: &str,
    _out_dir: &std::path::Path,
    _max_pages: usize,
) -> Result<Vec<String>> {
    Err(CoreError::Other(
        "pdfium rendering is not enabled in this build".to_string(),
    ))
}

/// Tier R docx generation: pipe Markdown through pandoc for high-fidelity
/// conversion. Runs only when a pandoc runtime is installed.
fn convert_markdown_to_docx_via_pandoc(
    pandoc: &std::path::Path,
    markdown: &str,
    title: Option<&str>,
    out_path: &str,
) -> Result<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // Prepend a title heading when one is given and the body lacks a top heading.
    let body = match title {
        Some(t) if !markdown.trim_start().starts_with("# ") => format!("# {t}\n\n{markdown}"),
        _ => markdown.to_string(),
    };

    let mut child = Command::new(pandoc)
        .args(["-f", "markdown", "-o", out_path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Other(format!("spawn pandoc: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .map_err(|e| CoreError::Other(format!("write to pandoc: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| CoreError::Other(format!("pandoc failed: {e}")))?;
    if !output.status.success() {
        return Err(CoreError::Other(format!(
            "pandoc exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Escape text for inclusion in XML content.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render a `<w:p>` paragraph with optional bold + half-point size on its run.
fn paragraph_xml(text: &str, bold: bool, half_pt: Option<u32>) -> String {
    let mut rpr = String::new();
    if bold || half_pt.is_some() {
        rpr.push_str("<w:rPr>");
        if bold {
            rpr.push_str("<w:b/>");
        }
        if let Some(sz) = half_pt {
            rpr.push_str(&format!("<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>"));
        }
        rpr.push_str("</w:rPr>");
    }
    format!(
        "<w:p><w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        escape_xml(text)
    )
}

/// Half-point font size for a heading level (1→32pt … smaller as level grows).
fn heading_size(level: u8) -> u32 {
    match level {
        1 => 40,
        2 => 32,
        3 => 28,
        4 => 26,
        _ => 24,
    }
}

/// Build a `<w:tbl>` from string rows with simple single-line borders.
fn table_xml(rows: &[Vec<String>]) -> String {
    let borders = "<w:tblBorders>\
<w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
<w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
<w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
<w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
<w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
<w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
</w:tblBorders>";
    let mut tbl = format!("<w:tbl><w:tblPr>{borders}</w:tblPr>");
    for row in rows {
        tbl.push_str("<w:tr>");
        for cell in row {
            tbl.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr>{}</w:tc>",
                paragraph_xml(cell, false, None)
            ));
        }
        tbl.push_str("</w:tr>");
    }
    tbl.push_str("</w:tbl>");
    tbl
}

/// Render the full `word/document.xml` for a [`DocSpec`].
fn render_document_xml(spec: &DocSpec) -> String {
    let mut body = String::new();
    if let Some(title) = &spec.title {
        body.push_str(&paragraph_xml(title, true, Some(heading_size(1))));
    }
    for block in &spec.blocks {
        match block {
            DocBlock::Heading { level, text } => {
                body.push_str(&paragraph_xml(text, true, Some(heading_size(*level))));
            }
            DocBlock::Paragraph(text) => body.push_str(&paragraph_xml(text, false, None)),
            DocBlock::Bullet(text) => {
                body.push_str(&paragraph_xml(&format!("•  {text}"), false, None))
            }
            DocBlock::Table(rows) => body.push_str(&table_xml(rows)),
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:body>{body}<w:sectPr/></w:body></w:document>"
    )
}

/// Write a minimal valid `.docx` (Content_Types + rels + document.xml).
fn write_docx_zip(out_path: &str, document_xml: &str) -> Result<()> {
    let file = std::fs::File::create(out_path)
        .map_err(|e| CoreError::Other(format!("create docx: {e}")))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
</Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

    let entries = [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels),
        ("word/document.xml", document_xml),
    ];
    for (name, data) in entries {
        zip.start_file(name, opts)
            .map_err(|e| CoreError::Other(format!("zip start '{name}': {e}")))?;
        zip.write_all(data.as_bytes())
            .map_err(|e| CoreError::Other(format!("zip write '{name}': {e}")))?;
    }
    zip.finish()
        .map_err(|e| CoreError::Other(format!("finish docx: {e}")))?;
    Ok(())
}

/// Parse a (simple) Markdown string into a [`DocSpec`]: `#`/`##`/… headings,
/// `-`/`*` bullets, blank-separated paragraphs. The first `# ` heading becomes
/// the title.
pub fn markdown_to_docspec(markdown: &str) -> DocSpec {
    let mut spec = DocSpec::default();
    for raw in markdown.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = heading_prefix(trimmed) {
            let (level, text) = rest;
            if level == 1 && spec.title.is_none() && spec.blocks.is_empty() {
                spec.title = Some(text.to_string());
            } else {
                spec.blocks.push(DocBlock::Heading {
                    level,
                    text: strip_inline(text),
                });
            }
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            spec.blocks.push(DocBlock::Bullet(strip_inline(item)));
        } else {
            spec.blocks.push(DocBlock::Paragraph(strip_inline(trimmed)));
        }
    }
    spec
}

/// Parse `#`-prefixed heading markers, returning `(level, text)`.
fn heading_prefix(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some((hashes as u8, line[hashes + 1..].trim_start()))
    } else {
        None
    }
}

/// Strip the most common inline Markdown emphasis markers for plain rendering.
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn office() -> (OfficeService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(crate::runtime_service::UnavailableDownloader),
            vec![],
        ));
        (OfficeService::new(runtime), dir)
    }

    fn read_docx_document_xml(path: &str) -> String {
        let f = std::fs::File::open(path).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut s = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    }

    #[test]
    fn tier_is_c_without_runtime() {
        let (svc, _d) = office();
        assert_eq!(svc.tier(), "C");
    }

    #[test]
    fn create_docx_writes_openable_package_with_text() {
        let (svc, dir) = office();
        let out = dir.path().join("out.docx");
        let out = out.to_string_lossy().into_owned();
        let spec = DocSpec {
            title: Some("Meeting".to_string()),
            blocks: vec![
                DocBlock::Heading {
                    level: 2,
                    text: "Decisions".to_string(),
                },
                DocBlock::Paragraph("Ship it & test <x>".to_string()),
                DocBlock::Bullet("todo one".to_string()),
                DocBlock::Table(vec![
                    vec!["a".into(), "b".into()],
                    vec!["1".into(), "2".into()],
                ]),
            ],
        };
        svc.create_docx(&spec, &out).unwrap();
        let xml = read_docx_document_xml(&out);
        assert!(xml.contains("Meeting"));
        assert!(xml.contains("Decisions"));
        // XML special chars are escaped.
        assert!(xml.contains("Ship it &amp; test &lt;x&gt;"));
        assert!(xml.contains("<w:tbl>"));
        // And our pure-Rust preview extractor can read the body back.
        let text = svc.read_text(&out).unwrap();
        assert!(text.contains("Meeting"));
        assert!(text.contains("Decisions"));
    }

    #[test]
    fn create_xlsx_roundtrips_via_calamine() {
        let (svc, dir) = office();
        let out = dir.path().join("out.xlsx");
        let out = out.to_string_lossy().into_owned();
        let sheets = vec![(
            "Data".to_string(),
            vec![
                vec!["name".to_string(), "value".to_string()],
                vec!["alpha".to_string(), "42".to_string()],
            ],
        )];
        svc.create_xlsx(&sheets, &out).unwrap();
        let text = svc.read_text(&out).unwrap();
        assert!(text.contains("Data"));
        assert!(text.contains("alpha"));
        assert!(text.contains("42"));
    }

    #[test]
    fn markdown_parses_title_headings_bullets() {
        let md = "# Title\n\n## Section\n\n- item a\n- item b\n\nA paragraph with **bold**.";
        let spec = markdown_to_docspec(md);
        assert_eq!(spec.title.as_deref(), Some("Title"));
        assert!(matches!(spec.blocks[0], DocBlock::Heading { level: 2, .. }));
        assert!(matches!(spec.blocks[1], DocBlock::Bullet(_)));
        assert!(matches!(spec.blocks[2], DocBlock::Bullet(_)));
        match &spec.blocks[3] {
            DocBlock::Paragraph(p) => assert_eq!(p, "A paragraph with bold."),
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn markdown_to_docx_end_to_end() {
        let (svc, dir) = office();
        let out = dir.path().join("minutes.docx");
        let out = out.to_string_lossy().into_owned();
        let md = "# 会议纪要\n\n## 摘要\n\n讨论了发布计划。\n\n- 决策一\n- 决策二";
        svc.create_docx_from_markdown(md, None, &out).unwrap();
        let text = svc.read_text(&out).unwrap();
        assert!(text.contains("会议纪要"));
        assert!(text.contains("决策一"));
    }
}
