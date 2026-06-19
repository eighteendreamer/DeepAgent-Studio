//! Error text parsing for codegraph location tools.
//!
//! The parser is deliberately dependency-light and tolerant: it scans arbitrary
//! pasted text for common stack-frame shapes and returns the file/line/column
//! coordinates it can recover. Unknown lines are ignored.

use std::path::{Path, PathBuf};

/// One source reference extracted from an error stack or diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub file: String,
    pub line: u32,
    pub col: Option<u32>,
    pub symbol: Option<String>,
    pub error_code: Option<String>,
    pub is_project: bool,
}

/// Parse stack frames from arbitrary text without project classification.
pub fn parse(text: &str) -> Vec<Frame> {
    ErrorParser::default().parse(text)
}

/// Configurable parser that can classify project-local frames.
#[derive(Debug, Clone, Default)]
pub struct ErrorParser {
    project_root: Option<PathBuf>,
    known_files: Vec<String>,
}

impl ErrorParser {
    /// Create a parser that marks frames as project frames when they fall
    /// under `project_root` or match one of the POSIX relative `known_files`.
    pub fn with_project(project_root: impl Into<PathBuf>, known_files: Vec<String>) -> Self {
        Self {
            project_root: Some(project_root.into()),
            known_files,
        }
    }

    /// Parse frames from arbitrary text.
    pub fn parse(&self, text: &str) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut current_error_code: Option<String> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(code) = extract_error_code(line) {
                current_error_code = Some(code);
            }
            let parsed = parse_node_frame(line)
                .or_else(|| parse_python_file_frame(line))
                .or_else(|| parse_rust_at_frame(line))
                .or_else(|| parse_java_frame(line))
                .or_else(|| parse_go_frame(line))
                .or_else(|| parse_plain_file_line(line));
            if let Some((file, line_no, col, symbol)) = parsed {
                frames.push(Frame {
                    is_project: self.is_project_file(&file),
                    file,
                    line: line_no,
                    col,
                    symbol,
                    error_code: current_error_code.clone(),
                });
            }
        }
        dedupe(frames)
    }

    fn is_project_file(&self, file: &str) -> bool {
        let normalized = normalize_path(file);
        if self
            .known_files
            .iter()
            .any(|known| normalize_path(known) == normalized)
        {
            return true;
        }
        let Some(root) = &self.project_root else {
            return false;
        };
        let path = Path::new(file);
        if path.is_absolute() {
            return path.starts_with(root);
        }
        root.join(path).exists()
    }
}

type ParsedFrame = (String, u32, Option<u32>, Option<String>);

fn parse_node_frame(line: &str) -> Option<ParsedFrame> {
    // at symbol (/repo/src/app.ts:10:5)
    // at /repo/src/app.ts:10:5
    let rest = line.strip_prefix("at ")?;
    if let Some(open) = rest.rfind('(') {
        let close = rest.rfind(')')?;
        if close > open {
            let symbol = rest[..open].trim();
            let loc = &rest[open + 1..close];
            let (file, line_no, col) = split_file_line_col(loc)?;
            return Some((
                file,
                line_no,
                col,
                (!symbol.is_empty()).then(|| symbol.to_string()),
            ));
        }
    }
    let (file, line_no, col) = split_file_line_col(rest)?;
    Some((file, line_no, col, None))
}

fn parse_python_file_frame(line: &str) -> Option<ParsedFrame> {
    // File "/repo/app.py", line 12, in handler
    let rest = line.strip_prefix("File ")?;
    let first_quote = rest.find('"')?;
    let after_first = &rest[first_quote + 1..];
    let second_quote = after_first.find('"')?;
    let file = after_first[..second_quote].to_string();
    let after_file = &after_first[second_quote + 1..];
    let line_marker = after_file.find("line ")?;
    let after_line = &after_file[line_marker + 5..];
    let (line_no, after_digits) = take_u32_prefix(after_line)?;
    let symbol = after_digits
        .find("in ")
        .map(|idx| after_digits[idx + 3..].trim().to_string())
        .filter(|s| !s.is_empty());
    Some((file, line_no, None, symbol))
}

fn parse_rust_at_frame(line: &str) -> Option<ParsedFrame> {
    // at src/main.rs:10:5
    let loc = line.strip_prefix("at ")?;
    let (file, line_no, col) = split_file_line_col(loc)?;
    Some((file, line_no, col, None))
}

fn parse_java_frame(line: &str) -> Option<ParsedFrame> {
    // at com.example.App.main(App.java:12)
    let rest = line.strip_prefix("at ")?;
    let open = rest.rfind('(')?;
    let close = rest.rfind(')')?;
    if close <= open {
        return None;
    }
    let symbol = rest[..open].trim();
    let loc = &rest[open + 1..close];
    let (file, line_no) = loc.rsplit_once(':')?;
    Some((
        file.to_string(),
        line_no.parse().ok()?,
        None,
        (!symbol.is_empty()).then(|| symbol.to_string()),
    ))
}

fn parse_go_frame(line: &str) -> Option<ParsedFrame> {
    // /repo/main.go:12 +0x20
    let first = line.split_whitespace().next()?;
    let (file, line_no, col) = split_file_line_col(first)?;
    if !file.ends_with(".go") {
        return None;
    }
    Some((file, line_no, col, None))
}

fn parse_plain_file_line(line: &str) -> Option<ParsedFrame> {
    // src/lib.rs:10:5: error[E0425]: ...
    let line = line.trim_start_matches('-').trim_start_matches('>').trim();
    let first = line.split_whitespace().next().unwrap_or(line);
    let (file, line_no, col) = split_file_line_col(first.trim_end_matches(':'))?;
    if !looks_like_file(&file) {
        return None;
    }
    Some((file, line_no, col, None))
}

fn split_file_line_col(s: &str) -> Option<(String, u32, Option<u32>)> {
    let cleaned = s.trim().trim_end_matches(',');
    let (left, right) = cleaned.rsplit_once(':')?;
    if !right.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (file, line_no, col) = match left.rsplit_once(':') {
        Some((file, line)) if line.chars().all(|c| c.is_ascii_digit()) => (
            file,
            line.parse::<u32>().ok()?,
            Some(right.parse::<u32>().ok()?),
        ),
        _ => (left, right.parse::<u32>().ok()?, None),
    };
    if file.len() == 1 && file.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    Some((strip_file_scheme(file).to_string(), line_no, col))
}

fn strip_file_scheme(file: &str) -> &str {
    file.strip_prefix("file://").unwrap_or(file)
}

fn take_u32_prefix(s: &str) -> Option<(u32, &str)> {
    let digits_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    Some((s[..digits_len].parse().ok()?, &s[digits_len..]))
}

fn extract_error_code(line: &str) -> Option<String> {
    if let Some(start) = line.find("error[") {
        let rest = &line[start + 6..];
        let end = rest.find(']')?;
        return Some(rest[..end].to_string());
    }
    if let Some(start) = line.find("TS") {
        let code = &line[start..];
        let digits = code
            .chars()
            .skip(2)
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits > 0 {
            return Some(code[..2 + digits].to_string());
        }
    }
    if line.starts_with("panic:") {
        return Some("panic".to_string());
    }
    if line.starts_with("Traceback ") {
        return Some("Traceback".to_string());
    }
    None
}

fn looks_like_file(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.contains('/')
        || lower.contains('\\')
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".py")
        || lower.ends_with(".go")
        || lower.ends_with(".java")
}

fn normalize_path(path: &str) -> String {
    strip_file_scheme(path).replace('\\', "/")
}

fn dedupe(frames: Vec<Frame>) -> Vec<Frame> {
    let mut out = Vec::new();
    for frame in frames {
        if out.iter().any(|existing: &Frame| {
            existing.file == frame.file
                && existing.line == frame.line
                && existing.col == frame.col
                && existing.symbol == frame.symbol
        }) {
            continue;
        }
        out.push(frame);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rust_diagnostic_location_and_error_code() {
        let frames = parse("error[E0425]: cannot find value\n --> src/main.rs:10:5\n");
        assert_eq!(frames[0].file, "src/main.rs");
        assert_eq!(frames[0].line, 10);
        assert_eq!(frames[0].col, Some(5));
        assert_eq!(frames[0].error_code.as_deref(), Some("E0425"));
    }

    #[test]
    fn parses_node_stack_frame() {
        let frames = parse("TypeError: bad\n    at handler (src/app.ts:12:7)\n");
        assert_eq!(frames[0].file, "src/app.ts");
        assert_eq!(frames[0].line, 12);
        assert_eq!(frames[0].col, Some(7));
        assert_eq!(frames[0].symbol.as_deref(), Some("handler"));
    }

    #[test]
    fn parses_python_traceback_frame() {
        let frames = parse("  File \"pkg/app.py\", line 22, in run\n    main()\n");
        assert_eq!(frames[0].file, "pkg/app.py");
        assert_eq!(frames[0].line, 22);
        assert_eq!(frames[0].symbol.as_deref(), Some("run"));
    }

    #[test]
    fn parses_java_and_go_frames() {
        let java = parse("at com.acme.App.main(App.java:44)");
        assert_eq!(java[0].file, "App.java");
        assert_eq!(java[0].line, 44);
        assert_eq!(java[0].symbol.as_deref(), Some("com.acme.App.main"));

        let go = parse("/repo/main.go:17 +0x20");
        assert_eq!(go[0].file, "/repo/main.go");
        assert_eq!(go[0].line, 17);
    }

    #[test]
    fn classifies_project_and_external_frames() {
        let parser = ErrorParser::with_project(
            std::env::current_dir().unwrap(),
            vec!["src/main.rs".to_string()],
        );
        let frames = parser.parse("at src/main.rs:3:1\nat node_modules/pkg/index.js:9:1\n");
        assert!(frames[0].is_project);
        assert!(!frames[1].is_project);
    }

    #[test]
    fn ignores_text_without_frames() {
        assert!(parse("nothing actionable here").is_empty());
    }
}
