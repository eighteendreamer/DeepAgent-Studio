//! Incremental Server-Sent Events data framing.
//!
//! The transport decodes UTF-8; this parser joins `data` fields and dispatches
//! only on a blank line. CR, LF and CRLF (including split CRLF) are supported.
//! Event names, IDs and retry hints are not projected by this data-only API.

/// Incremental SSE parser for already-decoded UTF-8 text chunks.
#[derive(Debug, Default)]
pub struct SseParser {
    line: String,
    data: String,
    skip_lf: bool,
    started: bool,
}

impl SseParser {
    /// New empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return complete event payloads, joining their `data` fields with LF.
    /// Partial lines and events remain buffered until a blank line arrives.
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        let mut out = Vec::new();
        for character in chunk.chars() {
            if !self.started {
                self.started = true;
                if character == '\u{feff}' {
                    continue;
                }
            }
            if self.skip_lf {
                self.skip_lf = false;
                if character == '\n' {
                    continue;
                }
            }
            match character {
                '\r' => {
                    self.finish_line(&mut out);
                    self.skip_lf = true;
                }
                '\n' => self.finish_line(&mut out),
                _ => self.line.push(character),
            }
        }
        out
    }

    fn finish_line(&mut self, out: &mut Vec<String>) {
        if self.line.is_empty() {
            if !self.data.is_empty() {
                // Each data field contributes one LF, including an empty field.
                self.data.pop();
                out.push(std::mem::take(&mut self.data));
            }
        } else if let Some(payload) = parse_data_line(&self.line) {
            self.data.push_str(payload);
            self.data.push('\n');
        }
        self.line.clear();
    }

    /// Finish the stream, discarding any event not terminated by a blank line.
    ///
    /// The old signature is retained for source compatibility, but EOF never
    /// dispatches a payload. This always returns `None` and resets the parser.
    pub fn flush(&mut self) -> Option<String> {
        *self = Self::default();
        None
    }
}

fn parse_data_line(line: &str) -> Option<&str> {
    let (field, value) = line.split_once(':').unwrap_or((line, ""));
    if field == "data" {
        // Field names are literal; remove at most one ASCII space after ':'.
        Some(value.strip_prefix(' ').unwrap_or(value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_sse_frames_preserve_multiline_json_and_require_terminal_boundary() {
        fn assemble(chunks: &[&str]) -> deepagent_core::error::Result<crate::Response> {
            let mut parser = SseParser::new();
            let mut accumulator = crate::ChatCompletionAccumulator::new();
            for chunk in chunks {
                for payload in parser.feed(chunk) {
                    if accumulator.push_sse_data(&payload)? {
                        return accumulator.finish();
                    }
                }
            }
            if let Some(payload) = parser.flush() {
                accumulator.push_sse_data(&payload)?;
            }
            accumulator.finish()
        }

        let single = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},",
            "\"finish_reason\":\"stop\"}]}\n\n"
        );
        let multiline = concat!(
            "data: {\"choices\":\n",
            "data: [{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n"
        );
        let cr_only = format!("{}data: [DONE]\r\r", single.replace('\n', "\r"));
        let cases: [(&str, Vec<&str>, bool); 4] = [
            (
                "single-line control",
                vec![single, "data: [DONE]\n\n"],
                true,
            ),
            ("multiline JSON", vec![multiline, "data: [DONE]\n\n"], true),
            ("CR-only lines", vec![&cr_only], true),
            (
                "unterminated terminal frame",
                vec![single, "data: [DONE]"],
                false,
            ),
        ];
        let mut mismatched = Vec::new();
        for (name, chunks, expected_success) in cases {
            let result = assemble(&chunks);
            eprintln!(
                "case={name}, expected_success={expected_success}, actual_success={}, error={:?}",
                result.is_ok(),
                result.as_ref().err()
            );
            if result.is_ok() != expected_success {
                mismatched.push(name);
            }
        }
        assert!(
            mismatched.is_empty(),
            "SSE event boundaries were not preserved: {mismatched:?}"
        );
    }

    #[test]
    fn parses_complete_events() {
        let mut p = SseParser::new();
        let out = p.feed(
            "data: {\"a\":1}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        assert_eq!(
            out,
            vec![
                "{\"a\":1}".to_string(),
                "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}"
                    .to_string()
            ]
        );
    }

    #[test]
    fn handles_split_chunks_without_loss() {
        let mut p = SseParser::new();
        // A line split across three feeds.
        assert!(p.feed("data: {\"hel").is_empty());
        assert!(p.feed("lo\": ").is_empty());
        assert!(p.feed("true}\n").is_empty());
        let out = p.feed("\n");
        assert_eq!(out, vec!["{\"hello\": true}".to_string()]);
    }

    #[test]
    fn ignores_comments_and_other_fields() {
        let mut p = SseParser::new();
        let out = p.feed(": this is a comment\nevent: message\ndata: payload\n\n");
        assert_eq!(out, vec!["payload".to_string()]);
    }

    #[test]
    fn handles_crlf() {
        let mut p = SseParser::new();
        let out = p.feed("data: x\r\n\r\ndata: y\r\n\r\n");
        assert_eq!(out, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn flush_discards_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.feed("data: tail").is_empty());
        assert_eq!(p.flush(), None);
        assert_eq!(p.flush(), None);
    }
}
