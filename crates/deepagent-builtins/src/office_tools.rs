//! `office_*` AI tools: read + generate office documents through a host
//! backend (office-agent Phase 3).
//!
//! Tool names use underscores (function-calling names must match
//! `[A-Za-z0-9_-]+`, so the conceptual `office.docx.create` is exposed as
//! `office_docx_create`). The host implements [`OfficeBackend`] over the
//! app-core `OfficeService`, which itself decides Tier R vs Tier C — the tools
//! are unaware of the tier.

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Backend adapter implemented by the host over `OfficeService`.
#[async_trait]
pub trait OfficeBackend: Send + Sync {
    /// Extract readable text from an office/text file (docx/xlsx/pptx/pdf/txt…).
    async fn read_text(&self, path: &str) -> Result<serde_json::Value>;
    /// Create a `.docx` from Markdown at `out_path`. `overwrite` must be true
    /// to replace an existing file.
    async fn create_docx_from_markdown(
        &self,
        markdown: &str,
        title: Option<String>,
        out_path: &str,
        overwrite: bool,
    ) -> Result<serde_json::Value>;
    /// Create an `.xlsx` from `sheets` (`[{ name, rows: [[..]] }]`) at `out_path`.
    /// `overwrite` must be true to replace an existing file.
    async fn create_xlsx(
        &self,
        sheets: serde_json::Value,
        out_path: &str,
        overwrite: bool,
    ) -> Result<serde_json::Value>;
}

/// Tool name.
pub const OFFICE_READ_TOOL_NAME: &str = "office_read";
/// Tool name.
pub const OFFICE_DOCX_CREATE_TOOL_NAME: &str = "office_docx_create";
/// Tool name.
pub const OFFICE_XLSX_CREATE_TOOL_NAME: &str = "office_xlsx_create";

/// Read an office/text document to plain text.
pub struct OfficeReadTool<B: OfficeBackend> {
    backend: B,
}

impl<B: OfficeBackend> OfficeReadTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: OfficeBackend> Tool for OfficeReadTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: OFFICE_READ_TOOL_NAME.to_string(),
            description: "Extract readable text from an office or text file (docx, xlsx, pptx, pdf, txt, md, csv). Before reading docx/xlsx/pptx/pdf files, invoke the matching skill first (`docx`, `xlsx`, `pptx`, or `pdf`) and follow its instructions.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Absolute path to the file." } },
                "required": ["path"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(path) = required_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        Ok(ToolOutput::success(self.backend.read_text(path).await?))
    }
}

/// Create a Word document from Markdown.
pub struct OfficeDocxCreateTool<B: OfficeBackend> {
    backend: B,
}

impl<B: OfficeBackend> OfficeDocxCreateTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: OfficeBackend> Tool for OfficeDocxCreateTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: OFFICE_DOCX_CREATE_TOOL_NAME.to_string(),
            description: "Create a Word (.docx) document from Markdown content. You MUST invoke `skill` with {\"id\":\"docx\"} before using this tool, then follow the docx skill's formatting and validation rules. Headings (#), bullet lists (-), and paragraphs are rendered. Writes to outPath inside the workspace.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "markdown": { "type": "string", "description": "Document body as Markdown." },
                    "title": { "type": "string", "description": "Optional document title." },
                    "outPath": { "type": "string", "description": "Absolute output path ending in .docx." },
                    "overwrite": { "type": "boolean", "description": "Set true to overwrite an existing file (default false)." }
                },
                "required": ["markdown", "outPath"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(markdown) = required_str(&args, "markdown") else {
            return Ok(ToolOutput::failure("missing 'markdown'"));
        };
        let Some(out_path) = required_str(&args, "outPath") else {
            return Ok(ToolOutput::failure("missing 'outPath'"));
        };
        let title = optional_string(&args, "title");
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(ToolOutput::success(
            self.backend
                .create_docx_from_markdown(markdown, title, out_path, overwrite)
                .await?,
        ))
    }
}

/// Create a spreadsheet from sheet data.
pub struct OfficeXlsxCreateTool<B: OfficeBackend> {
    backend: B,
}

impl<B: OfficeBackend> OfficeXlsxCreateTool<B> {
    /// Build the tool.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: OfficeBackend> Tool for OfficeXlsxCreateTool<B> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: OFFICE_XLSX_CREATE_TOOL_NAME.to_string(),
            description: "Create an Excel (.xlsx) workbook from sheet data. You MUST invoke `skill` with {\"id\":\"xlsx\"} before using this tool, then follow the xlsx skill's spreadsheet rules. Each sheet has a name and rows of string cells. Writes to outPath inside the workspace.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "sheets": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "rows": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } }
                            },
                            "required": ["name", "rows"]
                        },
                        "minItems": 1
                    },
                    "outPath": { "type": "string", "description": "Absolute output path ending in .xlsx." },
                    "overwrite": { "type": "boolean", "description": "Set true to overwrite an existing file (default false)." }
                },
                "required": ["sheets", "outPath"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(sheets) = args.get("sheets").cloned() else {
            return Ok(ToolOutput::failure("missing 'sheets'"));
        };
        if !sheets.is_array() || sheets.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            return Ok(ToolOutput::failure("'sheets' must be a non-empty array"));
        }
        let Some(out_path) = required_str(&args, "outPath") else {
            return Ok(ToolOutput::failure("missing 'outPath'"));
        };
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(ToolOutput::success(
            self.backend
                .create_xlsx(sheets, out_path, overwrite)
                .await?,
        ))
    }
}

fn required_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn optional_string(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct StubBackend {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl OfficeBackend for StubBackend {
        async fn read_text(&self, path: &str) -> Result<serde_json::Value> {
            self.calls.lock().unwrap().push(format!("read:{path}"));
            Ok(serde_json::json!({ "text": "hello" }))
        }
        async fn create_docx_from_markdown(
            &self,
            _markdown: &str,
            _title: Option<String>,
            out_path: &str,
            _overwrite: bool,
        ) -> Result<serde_json::Value> {
            self.calls.lock().unwrap().push(format!("docx:{out_path}"));
            Ok(serde_json::json!({ "path": out_path }))
        }
        async fn create_xlsx(
            &self,
            _sheets: serde_json::Value,
            out_path: &str,
            _overwrite: bool,
        ) -> Result<serde_json::Value> {
            self.calls.lock().unwrap().push(format!("xlsx:{out_path}"));
            Ok(serde_json::json!({ "path": out_path }))
        }
    }

    #[test]
    fn read_is_safe_create_is_write() {
        assert_eq!(
            OfficeReadTool::new(StubBackend::default())
                .descriptor()
                .risk,
            RiskLevel::Safe
        );
        let docx = OfficeDocxCreateTool::new(StubBackend::default()).descriptor();
        assert_eq!(docx.risk, RiskLevel::Low);
        assert!(docx
            .required_permissions
            .contains(Permission::WorkspaceWrite));
    }

    #[tokio::test]
    async fn docx_create_invokes_backend() {
        let backend = StubBackend::default();
        let calls = backend.calls.clone();
        let out = OfficeDocxCreateTool::new(backend)
            .invoke(serde_json::json!({ "markdown": "# Hi", "outPath": "/tmp/a.docx" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(calls.lock().unwrap()[0], "docx:/tmp/a.docx");
    }

    #[tokio::test]
    async fn missing_args_are_failures() {
        let out = OfficeDocxCreateTool::new(StubBackend::default())
            .invoke(serde_json::json!({ "markdown": "x" }))
            .await
            .unwrap();
        assert!(!out.ok);
        let out2 = OfficeXlsxCreateTool::new(StubBackend::default())
            .invoke(serde_json::json!({ "sheets": [], "outPath": "/tmp/a.xlsx" }))
            .await
            .unwrap();
        assert!(!out2.ok);
    }
}
