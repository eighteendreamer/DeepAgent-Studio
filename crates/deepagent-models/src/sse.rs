//! Minimal Server-Sent Events framing.
//!
//! We only need the subset of the SSE spec that chat-completion providers use:
//! lines beginning with `data:` carry a payload; a blank line dispatches the
//! event. This parser is byte-oriented and incremental so it can be fed
//! arbitrary network chunks (which may split a line mid-way) without losing
//! data — directly supporting the Phase 2 "无 chunk 丢失" requirement.

/// Incremental SSE parser. Feed it raw byte chunks; it yields complete `data:`
/// payloads as they become available.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    /// New empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of UTF-8 text, returning any complete `data:` payloads that
    /// were finished by this chunk. Partial lines are retained internally.
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();

        // Process all complete lines (terminated by '\n'); keep the remainder.
        while let Some(newline) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(payload) = parse_data_line(line) {
                out.push(payload);
            }
        }
        out
    }

    /// Flush any buffered final line that was not newline-terminated (e.g. at
    /// end of stream). Returns the payload if it was a `data:` line.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let line = std::mem::take(&mut self.buffer);
        let line = line.trim_end_matches(['\r', '\n']);
        parse_data_line(line)
    }
}

/// Extract the payload from a `data:` line, or `None` for comments / other
/// fields / blank lines.
fn parse_data_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("data:")?;
    // Per spec a single leading space after the colon is stripped.
    Some(rest.strip_prefix(' ').unwrap_or(rest).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_lines() {
        let mut p = SseParser::new();
        let out = p.feed(
            "data: {\"a\":1}\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
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
        let out = p.feed("true}\n");
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
        let out = p.feed("data: x\r\ndata: y\r\n");
        assert_eq!(out, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn flush_returns_unterminated_line() {
        let mut p = SseParser::new();
        assert!(p.feed("data: tail").is_empty());
        assert_eq!(p.flush(), Some("tail".to_string()));
        assert_eq!(p.flush(), None);
    }
}
